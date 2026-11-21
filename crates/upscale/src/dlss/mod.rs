//! DLSS (NVIDIA Deep Learning Super Sampling) through the NGX SDK (feature
//! `dlss`).
//!
//! * On Windows x86_64 the build script fetches the NGX SDK (never in the
//!   repository), generates its bindings with bindgen and links its static
//!   library; `nvngx_dlss.dll` is copied next to the binaries, where NGX
//!   looks for it. Elsewhere (Windows ARM64, Linux, macOS) nothing is fetched
//!   and DLSS is reported unavailable, so the registry falls back to FSR 1.
//! * Detection (the registry's probe): the renderer's device has the Vulkan
//!   extensions NGX needs, NGX initialises on it, and NGX reports DLSS
//!   available (an RTX GPU with a recent enough driver).
//! * Any failure falls back to FSR 1: at detection through the registry, at
//!   runtime inside the upscaler.
//!
//! Every NGX call goes through the [`ngx::Ngx`] trait, implemented over the
//! SDK in `ffi.rs`; the `unsafe` of the DLSS path lives in this module.

#[cfg(all(target_os = "windows", target_arch = "x86_64", target_env = "msvc"))]
mod ffi;
mod mapping;
mod ngx;
mod upscaler;

use std::sync::Arc;

use faset_render::GpuHandles;

use crate::{Backend, UpscaleError, Upscaler, UpscalerKind};
use ngx::{Ngx, NgxDevice};

fn unavailable(reason: String) -> UpscaleError {
    UpscaleError::Unavailable {
        kind: UpscalerKind::Dlss,
        reason,
    }
}

/// What the DLSS backend needs from the registry's context.
pub(crate) trait Host {
    /// The device, as NGX takes it.
    fn ngx_device(&self) -> NgxDevice;

    /// Builds the upscaler over NGX.
    fn build(
        &self,
        ngx: Arc<dyn Ngx>,
        device: NgxDevice,
    ) -> Result<Box<dyn Upscaler>, UpscaleError>;
}

impl Host for GpuHandles {
    fn ngx_device(&self) -> NgxDevice {
        NgxDevice {
            instance: self.instance.handle(),
            physical: self.physical,
            device: self.device.handle(),
            get_device_proc_addr: self.instance.fp_v1_0().get_device_proc_addr,
        }
    }

    fn build(
        &self,
        ngx: Arc<dyn Ngx>,
        device: NgxDevice,
    ) -> Result<Box<dyn Upscaler>, UpscaleError> {
        Ok(Box::new(upscaler::Dlss::new(self, ngx, device)?))
    }
}

type Loader = Box<dyn Fn() -> Result<Arc<dyn Ngx>, UpscaleError>>;

/// The registry entry of DLSS.
pub(crate) struct DlssBackend {
    load: Loader,
}

impl DlssBackend {
    /// The NGX SDK linked into this build (Windows x86_64 only).
    pub(crate) fn system() -> Self {
        Self::with_loader(Box::new(load_system))
    }

    pub(crate) fn with_loader(load: Loader) -> Self {
        Self { load }
    }
}

impl<C: Host> Backend<C> for DlssBackend {
    fn kind(&self) -> UpscalerKind {
        UpscalerKind::Dlss
    }

    fn probe(&self, host: &C) -> Result<(), UpscaleError> {
        upscaler::detect((self.load)()?, host.ngx_device())
    }

    fn create(&self, host: &C) -> Result<Box<dyn Upscaler>, UpscaleError> {
        host.build((self.load)()?, host.ngx_device())
    }
}

#[cfg(all(target_os = "windows", target_arch = "x86_64", target_env = "msvc"))]
fn load_system() -> Result<Arc<dyn Ngx>, UpscaleError> {
    Ok(Arc::new(ffi::SystemNgx))
}

#[cfg(not(all(target_os = "windows", target_arch = "x86_64", target_env = "msvc")))]
fn load_system() -> Result<Arc<dyn Ngx>, UpscaleError> {
    Err(unavailable(
        "DLSS is only available on Windows x86_64".into(),
    ))
}

#[cfg(test)]
mod tests {
    use std::ffi::c_void;
    use std::path::Path;
    use std::ptr::NonNull;
    use std::sync::Mutex;

    use ash::vk::{self, Handle};
    use faset_render::{FrameCtx, ImageHandle, UpscaleInputs};

    use super::ngx::{Capability, Feature, NgxResult, Param};
    use super::*;
    use crate::{Extent2D, Quality, Registry};

    /// Does nothing.
    struct Stub;

    impl Upscaler for Stub {
        fn render_extent(&self, display: Extent2D, _quality: Quality) -> Extent2D {
            display
        }
        fn jitter(&self, _frame_index: u32) -> faset_render::glam::Vec2 {
            faset_render::glam::Vec2::ZERO
        }
        fn execute(&mut self, _ctx: &mut FrameCtx<'_>, inputs: &UpscaleInputs) -> ImageHandle {
            inputs.colour
        }
    }

    unsafe extern "system" fn some_command() {}

    /// A device created with every extension DLSS needs.
    unsafe extern "system" fn every_command(
        _: vk::Device,
        _: *const std::ffi::c_char,
    ) -> vk::PFN_vkVoidFunction {
        Some(some_command)
    }

    /// A device created without them.
    unsafe extern "system" fn no_command(
        _: vk::Device,
        _: *const std::ffi::c_char,
    ) -> vk::PFN_vkVoidFunction {
        None
    }

    /// A registry context without a GPU.
    struct FakeHost {
        extensions: bool,
    }

    const WITH_EXTENSIONS: FakeHost = FakeHost { extensions: true };

    impl Host for FakeHost {
        fn ngx_device(&self) -> NgxDevice {
            NgxDevice {
                instance: vk::Instance::from_raw(0x10),
                physical: vk::PhysicalDevice::from_raw(0x20),
                device: vk::Device::from_raw(0x30),
                get_device_proc_addr: if self.extensions {
                    every_command
                } else {
                    no_command
                },
            }
        }
        fn build(
            &self,
            _ngx: Arc<dyn Ngx>,
            _device: NgxDevice,
        ) -> Result<Box<dyn Upscaler>, UpscaleError> {
            Ok(Box::new(Stub))
        }
    }

    /// FSR 1 that always works.
    struct Fsr1;

    impl Backend<FakeHost> for Fsr1 {
        fn kind(&self) -> UpscalerKind {
            UpscalerKind::Fsr1
        }
        fn probe(&self, _: &FakeHost) -> Result<(), UpscaleError> {
            Ok(())
        }
        fn create(&self, _: &FakeHost) -> Result<Box<dyn Upscaler>, UpscaleError> {
            Ok(Box::new(Stub))
        }
    }

    const RTX: Capability = Capability {
        available: true,
        needs_updated_driver: false,
        min_driver: (0, 0),
        init_result: NgxResult::SUCCESS,
    };

    /// NGX that records its calls; `init` and `capability` are scripted.
    struct Mock {
        init: NgxResult,
        capability: Result<Capability, NgxResult>,
        calls: Mutex<Vec<String>>,
    }

    impl Mock {
        fn new(init: NgxResult, capability: Result<Capability, NgxResult>) -> Arc<Self> {
            Arc::new(Self {
                init,
                capability,
                calls: Mutex::default(),
            })
        }

        fn log(&self, call: impl Into<String>) {
            self.calls.lock().unwrap().push(call.into());
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl Ngx for Mock {
        unsafe fn init(&self, device: &NgxDevice, data_path: &Path) -> NgxResult {
            assert!(data_path.is_dir(), "{}", data_path.display());
            self.log(format!("init {:#x}", device.device.as_raw()));
            self.init
        }
        unsafe fn shutdown(&self, _device: &NgxDevice) -> NgxResult {
            self.log("shutdown");
            NgxResult::SUCCESS
        }
        unsafe fn capability(&self) -> Result<Capability, NgxResult> {
            self.log("capability");
            self.capability
        }
        unsafe fn create_feature(
            &self,
            _device: &NgxDevice,
            _cmd: vk::CommandBuffer,
            _params: &[Param],
        ) -> Result<Feature, NgxResult> {
            self.log("create");
            Ok(Feature {
                handle: NonNull::<c_void>::dangling(),
                params: NonNull::<c_void>::dangling(),
            })
        }
        unsafe fn evaluate(
            &self,
            _cmd: vk::CommandBuffer,
            _feature: &Feature,
            _params: &[Param],
        ) -> NgxResult {
            self.log("evaluate");
            NgxResult::SUCCESS
        }
        unsafe fn release(&self, _feature: Feature) -> NgxResult {
            self.log("release");
            NgxResult::SUCCESS
        }
    }

    fn registry(dlss: DlssBackend) -> Registry<FakeHost> {
        let mut r = Registry::new();
        r.register(Box::new(Fsr1));
        r.register(Box::new(dlss));
        r
    }

    fn mocked(mock: &Arc<Mock>) -> DlssBackend {
        let mock = Arc::clone(mock);
        DlssBackend::with_loader(Box::new(move || Ok(Arc::clone(&mock) as Arc<dyn Ngx>)))
    }

    /// Asks for DLSS and expects FSR 1.
    fn assert_falls_back(r: &Registry<FakeHost>, host: &FakeHost) {
        assert_eq!(r.available(host), [UpscalerKind::Fsr1]);
        let made = r.create(UpscalerKind::Dlss, host).unwrap();
        assert_eq!(made.requested, UpscalerKind::Dlss);
        assert_eq!(made.kind, UpscalerKind::Fsr1);
        assert!(made.fell_back());
    }

    #[test]
    fn without_ngx_the_selection_falls_back_to_fsr1() {
        // Off Windows x86_64 the SDK is not linked at all; on it, the real
        // NGX would be called with this fake device, so the check is skipped.
        if cfg!(all(
            target_os = "windows",
            target_arch = "x86_64",
            target_env = "msvc"
        )) {
            return;
        }
        assert!(matches!(
            load_system().err(),
            Some(UpscaleError::Unavailable {
                kind: UpscalerKind::Dlss,
                ..
            })
        ));
        assert_falls_back(&registry(DlssBackend::system()), &WITH_EXTENSIONS);
    }

    #[test]
    fn a_non_rtx_gpu_falls_back_to_fsr1() {
        let not_rtx = Capability {
            available: false,
            init_result: NgxResult::FAIL_FEATURE_NOT_SUPPORTED,
            ..RTX
        };
        let mock = Mock::new(NgxResult::SUCCESS, Ok(not_rtx));
        assert_falls_back(&registry(mocked(&mock)), &WITH_EXTENSIONS);
        // Each probe (`available`, then `create`) opens and closes a session.
        let probe = ["init 0x30", "capability", "shutdown"];
        assert_eq!(mock.calls(), [probe, probe].concat());
    }

    #[test]
    fn ngx_failing_to_initialise_falls_back_to_fsr1() {
        let mock = Mock::new(NgxResult::FAIL_FEATURE_NOT_SUPPORTED, Ok(RTX));
        assert_falls_back(&registry(mocked(&mock)), &WITH_EXTENSIONS);
        // Nothing to shut down after a failed init.
        assert_eq!(mock.calls(), ["init 0x30", "init 0x30"]);
    }

    #[test]
    fn an_old_driver_or_a_failed_query_falls_back_to_fsr1() {
        let old_driver = Capability {
            needs_updated_driver: true,
            min_driver: (580, 1),
            ..RTX
        };
        for capability in [Ok(old_driver), Err(NgxResult::FAIL_OUT_OF_DATE)] {
            let mock = Mock::new(NgxResult::SUCCESS, capability);
            assert_falls_back(&registry(mocked(&mock)), &WITH_EXTENSIONS);
            assert_eq!(mock.calls().iter().filter(|c| *c == "shutdown").count(), 2);
        }
        let err = upscaler::check_capability(&old_driver).unwrap_err();
        assert!(err.to_string().contains("580.1"), "{err}");
    }

    #[test]
    fn a_device_without_the_ngx_extensions_falls_back_to_fsr1() {
        let mock = Mock::new(NgxResult::SUCCESS, Ok(RTX));
        let host = FakeHost { extensions: false };
        assert_falls_back(&registry(mocked(&mock)), &host);
        // NGX is not even initialised.
        assert!(mock.calls().is_empty());
        let missing = upscaler::missing_extensions(&host.ngx_device());
        assert_eq!(
            missing,
            [
                "VK_NVX_binary_import",
                "VK_NVX_image_view_handle",
                "VK_KHR_push_descriptor"
            ]
        );
        assert!(upscaler::missing_extensions(&WITH_EXTENSIONS.ngx_device()).is_empty());
    }

    #[test]
    fn a_usable_rtx_gpu_offers_dlss() {
        let mock = Mock::new(NgxResult::SUCCESS, Ok(RTX));
        let r = registry(mocked(&mock));
        assert_eq!(
            r.available(&WITH_EXTENSIONS),
            [UpscalerKind::Fsr1, UpscalerKind::Dlss]
        );
        let made = r.create(UpscalerKind::Dlss, &WITH_EXTENSIONS).unwrap();
        assert_eq!(made.kind, UpscalerKind::Dlss);
        assert!(!made.fell_back());
    }

    #[test]
    fn a_session_shuts_ngx_down_once() {
        let mock = Mock::new(NgxResult::SUCCESS, Ok(RTX));
        let session = upscaler::Session::open(mock.clone(), WITH_EXTENSIONS.ngx_device()).unwrap();
        assert_eq!(session.capability().unwrap(), RTX);
        drop(session);
        assert_eq!(mock.calls(), ["init 0x30", "capability", "shutdown"]);
    }
}

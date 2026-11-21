//! What DLSS needs from NGX, behind a trait in plain Rust types, so that
//! detection, fallback and the parameter mapping are tested on every platform
//! with a mock. The real implementation (`ffi.rs`) exists on Windows x86_64.

// Elsewhere, nothing reads most of these values outside the tests.
#![cfg_attr(
    not(all(target_os = "windows", target_arch = "x86_64", target_env = "msvc")),
    allow(dead_code)
)]

use std::ffi::{c_void, CStr};
use std::fmt;
use std::path::Path;
use std::ptr::NonNull;

use ash::vk;

/// The renderer's device, as NGX takes it.
#[derive(Clone, Copy)]
pub(crate) struct NgxDevice {
    pub instance: vk::Instance,
    pub physical: vk::PhysicalDevice,
    pub device: vk::Device,
    pub get_device_proc_addr: vk::PFN_vkGetDeviceProcAddr,
}

/// An `NVSDK_NGX_Result`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NgxResult(pub u32);

impl NgxResult {
    pub const SUCCESS: Self = Self(0x1);
    const FAIL: u32 = 0xBAD0_0000;
    pub const FAIL_FEATURE_NOT_SUPPORTED: Self = Self(Self::FAIL | 1);
    pub const FAIL_NOT_INITIALIZED: Self = Self(Self::FAIL | 7);
    pub const FAIL_OUT_OF_DATE: Self = Self(Self::FAIL | 12);

    /// `NVSDK_NGX_FAILED`.
    pub fn failed(self) -> bool {
        self.0 & 0xFFF0_0000 == Self::FAIL
    }

    pub fn check(self) -> Result<(), NgxResult> {
        if self.failed() {
            Err(self)
        } else {
            Ok(())
        }
    }
}

impl fmt::Display for NgxResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self.0 & !0xFFF0_0000 {
            _ if !self.failed() => "success",
            1 => "feature not supported",
            2 => "platform error",
            3 => "feature already exists",
            4 => "feature not found",
            5 => "invalid parameter",
            6 => "scratch buffer too small",
            7 => "not initialised",
            8 => "unsupported input format",
            9 => "read/write flag missing",
            10 => "missing input",
            11 => "unable to initialise the feature",
            12 => "out of date",
            13 => "out of GPU memory",
            14 => "unsupported format",
            15 => "unable to write to the app data path",
            16 => "unsupported parameter",
            17 => "denied",
            18 => "not implemented",
            _ => "unknown error",
        };
        write!(f, "{name} (NGX code {:#x})", self.0)
    }
}

/// An image handed to NGX, with what NGX needs to know about its view.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct NgxImage {
    pub image: vk::Image,
    pub view: vk::ImageView,
    pub format: vk::Format,
    pub extent: vk::Extent2D,
    pub aspect: vk::ImageAspectFlags,
    /// True for the output: written through a storage view.
    pub read_write: bool,
}

/// A value of an NGX parameter map.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Value {
    Int(i32),
    Uint(u32),
    Float(f32),
    /// A Vulkan image resource; `None` clears the slot (null pointer).
    Image(Option<NgxImage>),
}

/// A named parameter (the `NVSDK_NGX_Parameter_*` names of `nvsdk_ngx_defs.h`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Param {
    pub name: &'static CStr,
    pub value: Value,
}

/// What NGX reports about DLSS (super sampling) on this GPU and driver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Capability {
    /// `SuperSampling.Available`: false on non-RTX GPUs.
    pub available: bool,
    /// `SuperSampling.NeedsUpdatedDriver`.
    pub needs_updated_driver: bool,
    /// `SuperSampling.MinDriverVersionMajor` / `Minor`.
    pub min_driver: (u32, u32),
    /// `SuperSampling.FeatureInitResult`.
    pub init_result: NgxResult,
}

/// A created DLSS feature and the parameter map it is evaluated with.
#[derive(Debug)]
pub(crate) struct Feature {
    pub handle: NonNull<c_void>,
    pub params: NonNull<c_void>,
}

/// The NGX entry points DLSS uses (Vulkan flavour).
///
/// # Safety
/// The device handles must be live and NGX initialised on them (between
/// [`Ngx::init`] and [`Ngx::shutdown`]) for every call but `init`; command
/// buffers must be recording on a queue of that device, outside a rendering;
/// the images of the parameters live and in the layouts NGX expects.
pub(crate) trait Ngx {
    /// Initialises NGX on the device. Every success is paired with one
    /// [`Ngx::shutdown`].
    unsafe fn init(&self, device: &NgxDevice, data_path: &Path) -> NgxResult;

    unsafe fn shutdown(&self, device: &NgxDevice) -> NgxResult;

    /// Reads the DLSS capability of the GPU and driver.
    unsafe fn capability(&self) -> Result<Capability, NgxResult>;

    /// Creates a DLSS feature with `params`, recording into `cmd`.
    unsafe fn create_feature(
        &self,
        device: &NgxDevice,
        cmd: vk::CommandBuffer,
        params: &[Param],
    ) -> Result<Feature, NgxResult>;

    /// Sets `params` on the feature's map and records the evaluation.
    unsafe fn evaluate(
        &self,
        cmd: vk::CommandBuffer,
        feature: &Feature,
        params: &[Param],
    ) -> NgxResult;

    /// Releases the feature and its parameter map. The GPU must be done
    /// with it.
    unsafe fn release(&self, feature: Feature) -> NgxResult;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn results_follow_the_ngx_failure_mask() {
        assert!(!NgxResult::SUCCESS.failed());
        assert!(NgxResult::SUCCESS.check().is_ok());
        for r in [
            NgxResult::FAIL_FEATURE_NOT_SUPPORTED,
            NgxResult::FAIL_OUT_OF_DATE,
            NgxResult(0xBAD0_0000),
        ] {
            assert!(r.failed(), "{r}");
            assert_eq!(r.check(), Err(r));
        }
        assert_eq!(
            NgxResult::FAIL_OUT_OF_DATE.to_string(),
            "out of date (NGX code 0xbad0000c)"
        );
        assert_eq!(NgxResult::SUCCESS.to_string(), "success (NGX code 0x1)");
    }
}

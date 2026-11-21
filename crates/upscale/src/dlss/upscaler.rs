//! The DLSS upscaler: an NGX session on the renderer's device, the DLSS
//! feature, its display-sized output and the 1 x 1 exposure image. When DLSS
//! fails at runtime, FSR 1 takes over.

use std::cell::Cell;
use std::path::PathBuf;
use std::sync::Arc;

use ash::vk;
use faset_render::glam::Vec2;
use faset_render::{FrameCtx, GpuHandles, ImageHandle, UpscaleInputs};

use super::mapping::{self, DEVICE_EXTENSIONS};
use super::ngx::{Capability, Feature, Ngx, NgxDevice};
use super::unavailable;
use crate::gpu::{depth_transition, transition, Gpu, Image, READ, UNDEFINED};
use crate::jitter::{jitter_offset, jitter_phase_count};
use crate::{
    effective_quality, scaled_extent, Extent2D, Quality, UpscaleError, Upscaler, UpscalerKind,
};

/// Decision: NGX writes its logs under the system temporary directory, in
/// `faset-ngx/` (it needs a writable directory; the executable's may not be).
fn data_path() -> Result<PathBuf, UpscaleError> {
    let dir = std::env::temp_dir().join("faset-ngx");
    std::fs::create_dir_all(&dir)
        .map_err(|e| unavailable(format!("cannot create {}: {e}", dir.display())))?;
    Ok(dir)
}

/// The device extensions DLSS needs that the device was created without.
pub(crate) fn missing_extensions(device: &NgxDevice) -> Vec<&'static str> {
    DEVICE_EXTENSIONS
        .iter()
        .filter(|(_, command)| {
            // SAFETY: a live device and a null-terminated command name.
            unsafe { (device.get_device_proc_addr)(device.device, command.as_ptr()) }.is_none()
        })
        .map(|(name, _)| *name)
        .collect()
}

/// NGX initialised on a device, shut down on drop.
pub(crate) struct Session {
    ngx: Arc<dyn Ngx>,
    device: NgxDevice,
}

impl Session {
    pub(crate) fn open(ngx: Arc<dyn Ngx>, device: NgxDevice) -> Result<Self, UpscaleError> {
        let path = data_path()?;
        // SAFETY: the renderer's live device handles.
        let code = unsafe { ngx.init(&device, &path) };
        code.check()
            .map_err(|e| unavailable(format!("NGX failed to initialise: {e}")))?;
        Ok(Self { ngx, device })
    }

    pub(crate) fn capability(&self) -> Result<Capability, UpscaleError> {
        // SAFETY: NGX is initialised on the device.
        unsafe { self.ngx.capability() }
            .map_err(|e| unavailable(format!("NGX capability query failed: {e}")))
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // SAFETY: paired with the successful `init` of `open`.
        let code = unsafe { self.ngx.shutdown(&self.device) };
        if let Err(e) = code.check() {
            tracing::warn!("NGX shutdown failed: {e}");
        }
    }
}

/// Fails unless NGX reports DLSS usable.
pub(crate) fn check_capability(cap: &Capability) -> Result<(), UpscaleError> {
    if cap.needs_updated_driver {
        let (major, minor) = cap.min_driver;
        Err(unavailable(format!(
            "the NVIDIA driver is too old for DLSS (needs {major}.{minor} or newer)"
        )))
    } else if !cap.available {
        Err(unavailable(format!(
            "NGX reports DLSS unsupported on this GPU or driver (an RTX GPU is needed): {}",
            cap.init_result
        )))
    } else {
        Ok(())
    }
}

/// Checks that DLSS can run on the device: the extensions it needs are
/// enabled, NGX initialises and reports DLSS available. The session is shut
/// down again.
pub(crate) fn detect(ngx: Arc<dyn Ngx>, device: NgxDevice) -> Result<(), UpscaleError> {
    let missing = missing_extensions(&device);
    if !missing.is_empty() {
        return Err(unavailable(format!(
            "the renderer's Vulkan device was created without {}",
            missing.join(", ")
        )));
    }
    let session = Session::open(ngx, device)?;
    check_capability(&session.capability()?)?;
    tracing::info!("DLSS is available");
    Ok(())
}

/// What a DLSS feature was created for.
#[derive(Debug, Clone, Copy, PartialEq)]
struct FeatureKey {
    render: Extent2D,
    display: Extent2D,
    quality: Quality,
}

/// NVIDIA DLSS (temporal upscaling), through NGX.
pub(crate) struct Dlss {
    session: Session,
    handles: GpuHandles,
    feature: Option<(Feature, FeatureKey)>,
    output: Option<Image>,
    exposure: Option<Image>,
    /// The quality of the last [`Upscaler::render_extent`], for the feature.
    quality: Cell<Quality>,
    /// (render width, display width) of the last [`Upscaler::render_extent`],
    /// for the length of the jitter sequence.
    widths: Cell<(u32, u32)>,
    /// FSR 1, once DLSS failed.
    fallback: Option<Box<dyn Upscaler>>,
    /// Set after the first failure.
    failed: bool,
    // Last: dropped after `Drop for Dlss` freed the rest.
    gpu: Gpu,
}

impl Dlss {
    pub(crate) fn new(
        handles: &GpuHandles,
        ngx: Arc<dyn Ngx>,
        device: NgxDevice,
    ) -> Result<Self, UpscaleError> {
        let session = Session::open(ngx, device)?;
        let mut gpu = Gpu::new(handles)?;
        let exposure = gpu.create_image(
            "dlss-exposure",
            vk::Extent2D {
                width: 1,
                height: 1,
            },
            vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST,
        )?;
        Ok(Self {
            session,
            handles: handles.clone(),
            feature: None,
            output: None,
            exposure: Some(exposure),
            quality: Cell::new(Quality::Native),
            widths: Cell::new((1, 1)),
            fallback: None,
            failed: false,
            gpu,
        })
    }

    /// (Re)creates the feature and the output for `key`, recording the
    /// creation into `cmd`.
    fn ensure(&mut self, cmd: vk::CommandBuffer, key: FeatureKey) -> Result<(), UpscaleError> {
        if self.feature.as_ref().is_some_and(|(_, k)| *k == key) {
            return Ok(());
        }
        self.gpu.wait_idle();
        self.release();
        let usage = vk::ImageUsageFlags::STORAGE | vk::ImageUsageFlags::SAMPLED;
        self.output = Some(self.gpu.create_image("dlss-output", key.display, usage)?);
        let params = mapping::create_params(key.render, key.display, key.quality);
        // SAFETY: NGX is initialised on the device; `cmd` is recording outside
        // a rendering.
        let feature = unsafe {
            self.session
                .ngx
                .create_feature(&self.session.device, cmd, &params)
        }
        .map_err(|e| unavailable(format!("DLSS feature creation failed: {e}")))?;
        self.feature = Some((feature, key));
        tracing::info!(
            render = ?key.render,
            output = ?key.display,
            quality = ?key.quality,
            "DLSS feature created"
        );
        Ok(())
    }

    /// Frees the feature and the output. The GPU must be idle.
    fn release(&mut self) {
        if let Some((feature, _)) = self.feature.take() {
            // SAFETY: a live feature, released once; the GPU is idle.
            let code = unsafe { self.session.ngx.release(feature) };
            if let Err(e) = code.check() {
                tracing::warn!("DLSS feature release failed: {e}");
            }
        }
        if let Some(mut img) = self.output.take() {
            self.gpu.destroy_image(&mut img);
        }
    }

    fn record(
        &mut self,
        ctx: &mut FrameCtx<'_>,
        inputs: &UpscaleInputs,
    ) -> Result<ImageHandle, UpscaleError> {
        let key = FeatureKey {
            render: inputs.colour.extent,
            display: inputs.display,
            quality: self.quality.get(),
        };
        self.ensure(ctx.cmd, key)?;
        let output = self.output.as_ref().expect("created above").handle();
        let exposure = self.exposure.as_ref().expect("created in new").handle();
        let params = mapping::evaluate_params(inputs, &output, &exposure);
        let (feature, _) = self.feature.as_ref().expect("created above");
        let device = ctx.device;
        let cmd = ctx.cmd;
        let depth_read = vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL;
        let general = vk::ImageLayout::GENERAL;
        let dst = vk::ImageLayout::TRANSFER_DST_OPTIMAL;
        // SAFETY: the command buffer is recording outside a rendering; every
        // image is live; NGX reads the inputs in SHADER_READ_ONLY_OPTIMAL and
        // writes the output in GENERAL, and changes no layout.
        let code = unsafe {
            depth_transition(device, cmd, inputs.depth.image, depth_read, READ);
            transition(device, cmd, output.image, UNDEFINED, general);
            transition(device, cmd, exposure.image, UNDEFINED, dst);
            device.cmd_clear_color_image(
                cmd,
                exposure.image,
                dst,
                &vk::ClearColorValue {
                    float32: [inputs.exposure, 0.0, 0.0, 0.0],
                },
                &[crate::gpu::color_range()],
            );
            transition(device, cmd, exposure.image, dst, READ);
            let code = self.session.ngx.evaluate(cmd, feature, &params);
            depth_transition(device, cmd, inputs.depth.image, READ, depth_read);
            transition(device, cmd, output.image, general, READ);
            code
        };
        code.check()
            .map_err(|e| unavailable(format!("DLSS evaluation failed: {e}")))?;
        Ok(output)
    }
}

impl Upscaler for Dlss {
    fn render_extent(&self, display: Extent2D, quality: Quality) -> Extent2D {
        if let Some(fallback) = &self.fallback {
            return fallback.render_extent(display, quality);
        }
        let quality = effective_quality(UpscalerKind::Dlss, quality);
        let render = scaled_extent(display, quality);
        self.quality.set(quality);
        self.widths.set((render.width, display.width));
        render
    }

    fn jitter(&self, frame_index: u32) -> Vec2 {
        if let Some(fallback) = &self.fallback {
            return fallback.jitter(frame_index);
        }
        let (render, display) = self.widths.get();
        jitter_offset(frame_index, jitter_phase_count(render, display))
    }

    fn execute(&mut self, ctx: &mut FrameCtx<'_>, inputs: &UpscaleInputs) -> ImageHandle {
        if let Some(fallback) = &mut self.fallback {
            return fallback.execute(ctx, inputs);
        }
        if self.failed {
            return inputs.colour;
        }
        match self.record(ctx, inputs) {
            Ok(handle) => handle,
            Err(e) => {
                // Logged once: from now on FSR 1 runs instead.
                tracing::error!("DLSS failed, falling back to FSR 1: {e}");
                self.failed = true;
                match crate::fsr1::Fsr1::new(&self.handles) {
                    Ok(fsr1) => {
                        let fallback = self.fallback.insert(Box::new(fsr1));
                        fallback.execute(ctx, inputs)
                    }
                    Err(e) => {
                        tracing::error!("FSR 1 is unavailable too, passing the scene through: {e}");
                        inputs.colour
                    }
                }
            }
        }
    }
}

impl Drop for Dlss {
    fn drop(&mut self) {
        // The renderer waited for the device before dropping us.
        self.release();
        if let Some(mut img) = self.exposure.take() {
            self.gpu.destroy_image(&mut img);
        }
    }
}

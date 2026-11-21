//! The FSR 3 upscaler: a FidelityFX context on the renderer's device, its
//! display-sized output and the 1 x 1 exposure image.

use std::cell::Cell;
use std::sync::Arc;

use ash::vk;
use faset_render::glam::Vec2;
use faset_render::{FrameCtx, GpuHandles, ImageHandle, UpscaleInputs};

use super::api::{self, check, FfxApi};
use super::bindings as ffx;
use super::mapping::{self, CreateDescs, FfxDevice};
use super::unavailable;
use crate::gpu::{depth_transition, transition, Gpu, Image, READ, UNDEFINED};
use crate::{
    effective_quality, scaled_extent, Extent2D, Quality, UpscaleError, Upscaler, UpscalerKind,
};

/// A live FidelityFX upscaling context.
pub(crate) struct Context {
    handle: ffx::ffxContext,
    display: Extent2D,
    /// The creation descriptors, kept alive with the context as `ffx_api.h`
    /// asks for the pointers they hold.
    _descs: Box<CreateDescs>,
}

impl Context {
    /// Creates a context for `display` on `device`.
    pub(crate) fn new(
        api: &dyn FfxApi,
        device: &FfxDevice,
        display: Extent2D,
    ) -> Result<Self, UpscaleError> {
        let mut descs = Box::new(CreateDescs::new(device, display));
        let mut handle: ffx::ffxContext = std::ptr::null_mut();
        // SAFETY: `descs` is boxed, so the chain stays valid for the
        // context's life; the device handles are the renderer's, alive while
        // the upscaler is.
        let code = unsafe { api.create_context(&mut handle, descs.chain()) };
        check(code).map_err(|e| unavailable(format!("the library failed to initialise: {e}")))?;
        if handle.is_null() {
            return Err(unavailable("the library returned no context".into()));
        }
        Ok(Self {
            handle,
            display,
            _descs: descs,
        })
    }

    /// Destroys the context. The GPU must be done with its dispatches.
    pub(crate) fn destroy(mut self, api: &dyn FfxApi) {
        // SAFETY: a live context, destroyed once (`self` is consumed).
        let code = unsafe { api.destroy_context(&mut self.handle) };
        if let Err(e) = check(code) {
            tracing::warn!("FSR 3 context destruction failed: {e}");
        }
    }

    pub(crate) fn handle_mut(&mut self) -> &mut ffx::ffxContext {
        &mut self.handle
    }
}

/// FidelityFX Super Resolution 3 (temporal upscaling), through AMD's library.
pub(crate) struct Fsr3 {
    api: Arc<dyn FfxApi>,
    device: FfxDevice,
    context: Option<Context>,
    output: Option<Image>,
    exposure: Option<Image>,
    /// (render width, display width) of the last [`Upscaler::render_extent`],
    /// for the length of the jitter sequence.
    widths: Cell<(u32, u32)>,
    /// Set after the first failure; the scene then passes through.
    failed: bool,
    // Last: dropped after `Drop for Fsr3` freed the rest.
    gpu: Gpu,
}

impl Fsr3 {
    pub(crate) fn new(
        handles: &GpuHandles,
        api: Arc<dyn FfxApi>,
        device: FfxDevice,
    ) -> Result<Self, UpscaleError> {
        let mut gpu = Gpu::new(handles)?;
        let exposure = gpu.create_image(
            "fsr3-exposure",
            vk::Extent2D {
                width: 1,
                height: 1,
            },
            vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST,
        )?;
        Ok(Self {
            api,
            device,
            context: None,
            output: None,
            exposure: Some(exposure),
            widths: Cell::new((1, 1)),
            failed: false,
            gpu,
        })
    }

    /// (Re)creates the context and the output for a display size.
    fn ensure(&mut self, display: Extent2D) -> Result<(), UpscaleError> {
        if self.context.as_ref().is_some_and(|c| c.display == display) {
            return Ok(());
        }
        self.gpu.wait_idle();
        self.release();
        let usage = vk::ImageUsageFlags::STORAGE | vk::ImageUsageFlags::SAMPLED;
        self.output = Some(self.gpu.create_image("fsr3-output", display, usage)?);
        self.context = Some(Context::new(&*self.api, &self.device, display)?);
        let (width, height) = (display.width, display.height);
        tracing::info!(width, height, "FSR 3 context created");
        Ok(())
    }

    /// Frees the context and the output. The GPU must be idle.
    fn release(&mut self) {
        if let Some(c) = self.context.take() {
            c.destroy(&*self.api);
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
        self.ensure(inputs.display)?;
        let output = self.output.as_ref().expect("created above").handle();
        let exposure = self.exposure.as_ref().expect("created in new").handle();
        let desc = mapping::dispatch_desc(inputs, ctx.cmd, &output, &exposure);
        let context = self.context.as_mut().expect("created above");
        let device = ctx.device;
        let cmd = ctx.cmd;
        let depth_read = vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL;
        let general = vk::ImageLayout::GENERAL;
        let dst = vk::ImageLayout::TRANSFER_DST_OPTIMAL;
        // SAFETY: the command buffer is recording outside a rendering; every
        // image is live; the dispatch descriptor only points at values that
        // outlive the call.
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
            let code = self.api.dispatch(context.handle_mut(), &desc.header);
            // The library put every external resource back in its state.
            depth_transition(device, cmd, inputs.depth.image, READ, depth_read);
            transition(device, cmd, output.image, general, READ);
            code
        };
        check(code).map_err(|e| unavailable(format!("dispatch failed: {e}")))?;
        Ok(output)
    }
}

impl Upscaler for Fsr3 {
    fn render_extent(&self, display: Extent2D, quality: Quality) -> Extent2D {
        let render = scaled_extent(display, effective_quality(UpscalerKind::Fsr3, quality));
        self.widths.set((render.width, display.width));
        render
    }

    fn jitter(&self, frame_index: u32) -> Vec2 {
        let (render, display) = self.widths.get();
        mapping::jitter_offset(frame_index, mapping::jitter_phase_count(render, display))
    }

    fn execute(&mut self, ctx: &mut FrameCtx<'_>, inputs: &UpscaleInputs) -> ImageHandle {
        if self.failed {
            return inputs.colour;
        }
        match self.record(ctx, inputs) {
            Ok(handle) => handle,
            Err(e) => {
                // Logged once: a failing library keeps failing.
                tracing::error!("FSR 3 failed, passing the scene through from now on: {e}");
                self.failed = true;
                inputs.colour
            }
        }
    }
}

impl Drop for Fsr3 {
    fn drop(&mut self) {
        // The renderer waited for the device before dropping us.
        self.release();
        if let Some(mut img) = self.exposure.take() {
            self.gpu.destroy_image(&mut img);
        }
    }
}

/// Checks that the library initialises on the device: a small context is
/// created, its provider logged, then destroyed.
pub(crate) fn detect(api: &dyn FfxApi, device: &FfxDevice) -> Result<(), UpscaleError> {
    let mut context = Context::new(api, device, mapping::PROBE_EXTENT)?;
    match api::provider_name(api, context.handle_mut()) {
        Some(name) => tracing::info!(provider = %name, "FSR 3 is available"),
        None => tracing::info!("FSR 3 is available"),
    }
    context.destroy(api);
    Ok(())
}

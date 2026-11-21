//! The Native upscaler: the scene is drawn at the display resolution and
//! passes through untouched. At a free render scale below 100 % the image is
//! stretched to the display with a bilinear filter, with no other processing.

use ash::vk;
use faset_render::glam::Vec2;
use faset_render::{FrameCtx, GpuHandles, ImageHandle, UpscaleInputs};

use crate::gpu::{
    transition, FramePools, Gpu, Image, Pass, PassLayout, ATTACHMENT, READ, UNDEFINED,
};
use crate::{
    effective_quality, scaled_extent, Extent2D, Quality, UpscaleError, Upscaler, UpscalerKind,
};

/// Native resolution, with an optional free render scale.
pub struct Native {
    layout: PassLayout,
    pipeline: vk::Pipeline,
    pools: Option<FramePools>,
    output: Option<Image>,
    // Last: dropped after `Drop for Native` freed the rest.
    gpu: Gpu,
}

impl Native {
    pub fn new(handles: &GpuHandles) -> Result<Self, UpscaleError> {
        let gpu = Gpu::new(handles)?;
        let layout = PassLayout::new(&gpu, 1)?;
        let pipeline = layout.pipeline(
            &gpu.device,
            faset_shaders::BILINEAR_VS,
            faset_shaders::BILINEAR_FS,
            c"fs_main",
            1,
        );
        let pipeline = match pipeline {
            Ok(p) => p,
            Err(e) => {
                layout.destroy(&gpu.device);
                return Err(e);
            }
        };
        Ok(Self {
            layout,
            pipeline,
            pools: None,
            output: None,
            gpu,
        })
    }

    fn ensure_output(&mut self, extent: vk::Extent2D) -> Result<(), UpscaleError> {
        if self.output.as_ref().is_some_and(|o| o.extent == extent) {
            return Ok(());
        }
        self.gpu.wait_idle();
        self.release_output();
        let usage = vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::SAMPLED;
        self.output = Some(self.gpu.create_image("native-stretch", extent, usage)?);
        Ok(())
    }

    fn release_output(&mut self) {
        if let Some(mut o) = self.output.take() {
            self.gpu.destroy_image(&mut o);
        }
    }

    fn record(
        &mut self,
        ctx: &mut FrameCtx<'_>,
        inputs: &UpscaleInputs,
    ) -> Result<ImageHandle, UpscaleError> {
        self.ensure_output(inputs.display)?;
        if self.pools.is_none() {
            self.pools = Some(FramePools::new(
                &self.gpu.device,
                ctx.frames_in_flight,
                self.layout.images,
                1,
            )?);
        }
        let output = self.output.as_ref().expect("created above");
        let pools = self.pools.as_ref().expect("created above");
        let (device, cmd) = (ctx.device, ctx.cmd);
        // SAFETY: the slot's fence was waited on by the renderer; every
        // image is live; the command buffer is recording outside a rendering.
        unsafe {
            pools.reset(device, ctx.slot)?;
            let set = pools.set(
                device,
                &self.layout,
                ctx.slot,
                &[(inputs.colour.view, READ)],
            )?;
            transition(device, cmd, output.image, UNDEFINED, ATTACHMENT);
            Pass {
                pipeline: self.pipeline,
                layout: &self.layout,
                set,
                targets: &[output.view],
                extent: inputs.display,
                push: &[],
            }
            .record(&self.gpu, cmd);
            transition(device, cmd, output.image, ATTACHMENT, READ);
        }
        Ok(output.handle())
    }
}

impl Upscaler for Native {
    fn render_extent(&self, display: Extent2D, quality: Quality) -> Extent2D {
        scaled_extent(display, effective_quality(UpscalerKind::Native, quality))
    }

    fn jitter(&self, _frame_index: u32) -> Vec2 {
        Vec2::ZERO
    }

    fn execute(&mut self, ctx: &mut FrameCtx<'_>, inputs: &UpscaleInputs) -> ImageHandle {
        if inputs.colour.extent == inputs.display {
            return inputs.colour;
        }
        match self.record(ctx, inputs) {
            Ok(handle) => handle,
            Err(e) => {
                tracing::error!("Native stretch failed: {e}");
                inputs.colour
            }
        }
    }
}

impl Drop for Native {
    fn drop(&mut self) {
        self.release_output();
        if let Some(mut pools) = self.pools.take() {
            pools.destroy(&self.gpu.device);
        }
        // SAFETY: the renderer waited for the device before dropping us.
        unsafe { self.gpu.device.destroy_pipeline(self.pipeline, None) };
        self.layout.destroy(&self.gpu.device);
    }
}

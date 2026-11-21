//! FidelityFX Super Resolution 1: EASU upsampling then RCAS sharpening, both
//! purely spatial (`fsr1.wgsl`), so it needs no motion vectors and runs on
//! every GPU the renderer does.

use ash::vk;
use faset_render::glam::Vec2;
use faset_render::{FrameCtx, GpuHandles, ImageHandle, UpscaleInputs};

use crate::gpu::{
    transition, FramePools, Gpu, Image, Pass, PassLayout, ATTACHMENT, READ, UNDEFINED,
};
use crate::{
    effective_quality, scaled_extent, Extent2D, Quality, UpscaleError, Upscaler, UpscalerKind,
};

/// RCAS sharpness in stops of attenuation: 0 is the sharpest, 2 the
/// softest. AMD's default is 0.2.
pub const DEFAULT_SHARPNESS: f32 = 0.2;

/// The EASU push constants: the input to output scale of each axis and the
/// offset that centres the sample (`p0` of `fsr1.wgsl`).
pub fn easu_constants(input: Extent2D, output: Extent2D) -> [f32; 4] {
    let sx = input.width as f32 / output.width.max(1) as f32;
    let sy = input.height as f32 / output.height.max(1) as f32;
    [sx, sy, 0.5 * sx - 0.5, 0.5 * sy - 0.5]
}

/// The RCAS attenuation constant for a sharpness in stops.
pub fn rcas_constant(sharpness: f32) -> f32 {
    (-sharpness.clamp(0.0, 2.0)).exp2()
}

/// FSR 1.
pub struct Fsr1 {
    layout: PassLayout,
    easu: vk::Pipeline,
    rcas: vk::Pipeline,
    pools: Option<FramePools>,
    targets: Option<Targets>,
    sharpness: f32,
    // Last: dropped after `Drop for Fsr1` freed the rest.
    gpu: Gpu,
}

struct Targets {
    extent: vk::Extent2D,
    /// EASU output, compressed colour.
    upscaled: Image,
    /// RCAS output: the result.
    output: Image,
}

impl Fsr1 {
    pub fn new(handles: &GpuHandles) -> Result<Self, UpscaleError> {
        let gpu = Gpu::new(handles)?;
        let layout = PassLayout::new(&gpu, 1)?;
        let build = |fs: &[u32], entry: &std::ffi::CStr| {
            layout.pipeline(&gpu.device, faset_shaders::FSR1_VS, fs, entry, 1)
        };
        let easu = match build(faset_shaders::FSR1_FS_EASU, c"fs_easu") {
            Ok(p) => p,
            Err(e) => {
                layout.destroy(&gpu.device);
                return Err(e);
            }
        };
        let rcas = match build(faset_shaders::FSR1_FS_RCAS, c"fs_rcas") {
            Ok(p) => p,
            Err(e) => {
                // SAFETY: unused pipeline.
                unsafe { gpu.device.destroy_pipeline(easu, None) };
                layout.destroy(&gpu.device);
                return Err(e);
            }
        };
        Ok(Self {
            layout,
            easu,
            rcas,
            pools: None,
            targets: None,
            sharpness: DEFAULT_SHARPNESS,
            gpu,
        })
    }

    /// Sets the RCAS sharpness in stops (0 sharpest, 2 softest).
    pub fn set_sharpness(&mut self, sharpness: f32) {
        self.sharpness = sharpness.clamp(0.0, 2.0);
    }

    fn ensure_targets(&mut self, extent: vk::Extent2D) -> Result<(), UpscaleError> {
        if self.targets.as_ref().is_some_and(|t| t.extent == extent) {
            return Ok(());
        }
        self.gpu.wait_idle();
        self.release_targets();
        let usage = vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::SAMPLED;
        let mut upscaled = self.gpu.create_image("fsr1-easu", extent, usage)?;
        match self.gpu.create_image("fsr1-rcas", extent, usage) {
            Ok(output) => {
                self.targets = Some(Targets {
                    extent,
                    upscaled,
                    output,
                });
                Ok(())
            }
            Err(e) => {
                self.gpu.destroy_image(&mut upscaled);
                Err(e)
            }
        }
    }

    fn release_targets(&mut self) {
        if let Some(mut t) = self.targets.take() {
            self.gpu.destroy_image(&mut t.upscaled);
            self.gpu.destroy_image(&mut t.output);
        }
    }

    fn record(
        &mut self,
        ctx: &mut FrameCtx<'_>,
        inputs: &UpscaleInputs,
    ) -> Result<ImageHandle, UpscaleError> {
        self.ensure_targets(inputs.display)?;
        if self.pools.is_none() {
            self.pools = Some(FramePools::new(
                &self.gpu.device,
                ctx.frames_in_flight,
                self.layout.images,
                2,
            )?);
        }
        let targets = self.targets.as_ref().expect("created above");
        let pools = self.pools.as_ref().expect("created above");
        let (device, cmd) = (ctx.device, ctx.cmd);
        let easu_push = easu_constants(inputs.colour.extent, inputs.display);
        let rcas_push = [rcas_constant(self.sharpness), 0.0, 0.0, 0.0];
        // SAFETY: the slot's fence was waited on by the renderer; every
        // image is live; the command buffer is recording outside a rendering.
        unsafe {
            pools.reset(device, ctx.slot)?;
            let source = pools.set(
                device,
                &self.layout,
                ctx.slot,
                &[(inputs.colour.view, READ)],
            )?;
            let middle = pools.set(
                device,
                &self.layout,
                ctx.slot,
                &[(targets.upscaled.view, READ)],
            )?;
            transition(device, cmd, targets.upscaled.image, UNDEFINED, ATTACHMENT);
            Pass {
                pipeline: self.easu,
                layout: &self.layout,
                set: source,
                targets: &[targets.upscaled.view],
                extent: inputs.display,
                push: &easu_push,
            }
            .record(&self.gpu, cmd);
            transition(device, cmd, targets.upscaled.image, ATTACHMENT, READ);
            transition(device, cmd, targets.output.image, UNDEFINED, ATTACHMENT);
            Pass {
                pipeline: self.rcas,
                layout: &self.layout,
                set: middle,
                targets: &[targets.output.view],
                extent: inputs.display,
                push: &rcas_push,
            }
            .record(&self.gpu, cmd);
            transition(device, cmd, targets.output.image, ATTACHMENT, READ);
        }
        Ok(targets.output.handle())
    }
}

impl Upscaler for Fsr1 {
    fn render_extent(&self, display: Extent2D, quality: Quality) -> Extent2D {
        scaled_extent(display, effective_quality(UpscalerKind::Fsr1, quality))
    }

    fn jitter(&self, _frame_index: u32) -> Vec2 {
        // Purely spatial: a jitter would only shimmer.
        Vec2::ZERO
    }

    fn execute(&mut self, ctx: &mut FrameCtx<'_>, inputs: &UpscaleInputs) -> ImageHandle {
        match self.record(ctx, inputs) {
            Ok(handle) => handle,
            Err(e) => {
                tracing::error!("FSR 1 failed, passing the scene through: {e}");
                inputs.colour
            }
        }
    }
}

impl Drop for Fsr1 {
    fn drop(&mut self) {
        self.release_targets();
        if let Some(mut pools) = self.pools.take() {
            pools.destroy(&self.gpu.device);
        }
        // SAFETY: the renderer waited for the device before dropping us.
        unsafe {
            self.gpu.device.destroy_pipeline(self.easu, None);
            self.gpu.device.destroy_pipeline(self.rcas, None);
        }
        self.layout.destroy(&self.gpu.device);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ext(width: u32, height: u32) -> Extent2D {
        Extent2D { width, height }
    }

    #[test]
    fn easu_constants_map_output_pixels_to_input_texels() {
        let c = easu_constants(ext(960, 540), ext(1920, 1080));
        assert_eq!(c, [0.5, 0.5, -0.25, -0.25]);
        // 1:1 has no offset.
        assert_eq!(
            easu_constants(ext(64, 64), ext(64, 64)),
            [1.0, 1.0, 0.0, 0.0]
        );
        // Output pixel 0 has its centre at 0.25 input texels: texel centre
        // coordinates are (x + 0.5) * scale - 0.5.
        let x = (0.0 + 0.5) * c[0] - 0.5;
        assert_eq!(x, c[2]);
    }

    #[test]
    fn rcas_sharpness_is_in_stops() {
        assert_eq!(rcas_constant(0.0), 1.0);
        assert_eq!(rcas_constant(1.0), 0.5);
        assert_eq!(rcas_constant(2.0), 0.25);
        // Clamped to the range.
        assert_eq!(rcas_constant(-3.0), 1.0);
        assert_eq!(rcas_constant(9.0), 0.25);
    }
}

//! Temporal anti-aliasing.
//!
//! The scene is rendered at the display resolution with a sub-pixel jitter
//! (Halton 2, 3 over 16 frames, from the renderer). Each frame the history
//! is reprojected with the motion vectors, clamped to the 3 x 3
//! neighbourhood bounding box of the current frame in YCoCg, and the current
//! sample is blended in at 10 %. The history is dropped when the reset flag
//! is raised, when the size changes and on the first frame.
//!
//! The per-pixel maths lives in `taa.wgsl`; the functions here are its CPU
//! reference, used by the tests, and [`TaaState`] is the bookkeeping of the
//! history.

use ash::vk;
use faset_render::glam::Vec2;
use faset_render::{FrameCtx, GpuHandles, ImageHandle, UpscaleInputs};

use crate::gpu::{
    transition, FramePools, Gpu, Image, Pass, PassLayout, ATTACHMENT, READ, UNDEFINED,
};
use crate::{scaled_extent, Extent2D, Quality, UpscaleError, Upscaler};

/// Weight of the current frame in the blend (10 %).
pub const CURRENT_WEIGHT: f32 = 0.1;

/// Largest finite half float, so a stray infinity cannot spread.
const HDR_MAX: f32 = 65000.0;

/// Maps linear HDR colour to [0, 1): `c / (1 + max(c))`. The blend works on
/// these values so a bright highlight cannot flicker.
pub fn compress(c: [f32; 3]) -> [f32; 3] {
    let v = c.map(|x| x.clamp(0.0, HDR_MAX));
    let m = v[0].max(v[1]).max(v[2]);
    v.map(|x| x / (1.0 + m))
}

/// The inverse of [`compress`].
pub fn decompress(c: [f32; 3]) -> [f32; 3] {
    let m = c[0].max(c[1]).max(c[2]).min(0.99998);
    c.map(|x| x.max(0.0) / (1.0 - m))
}

pub fn rgb_to_ycocg(c: [f32; 3]) -> [f32; 3] {
    [
        0.25 * c[0] + 0.5 * c[1] + 0.25 * c[2],
        0.5 * c[0] - 0.5 * c[2],
        -0.25 * c[0] + 0.5 * c[1] - 0.25 * c[2],
    ]
}

pub fn ycocg_to_rgb(c: [f32; 3]) -> [f32; 3] {
    [c[0] + c[1] - c[2], c[0] + c[2], c[0] - c[1] - c[2]]
}

/// Componentwise bounding box, in YCoCg, of compressed neighbour colours.
pub fn neighbourhood_box(neighbours: &[[f32; 3]]) -> ([f32; 3], [f32; 3]) {
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for &n in neighbours {
        let y = rgb_to_ycocg(n);
        for i in 0..3 {
            lo[i] = lo[i].min(y[i]);
            hi[i] = hi[i].max(y[i]);
        }
    }
    (lo, hi)
}

/// Clamps a compressed history colour to the box, in YCoCg.
pub fn clamp_history(history: [f32; 3], lo: [f32; 3], hi: [f32; 3]) -> [f32; 3] {
    let y = rgb_to_ycocg(history);
    ycocg_to_rgb([
        y[0].clamp(lo[0], hi[0]),
        y[1].clamp(lo[1], hi[1]),
        y[2].clamp(lo[2], hi[2]),
    ])
}

/// The weight of the current sample: [`CURRENT_WEIGHT`], raised to the
/// reactive mask, and 1 when the history cannot be used (`reset`, first
/// frame, size change, or the reprojected position is off screen).
pub fn current_weight(history_valid: bool, on_screen: bool, reactive: f32) -> f32 {
    if !history_valid || !on_screen {
        return 1.0;
    }
    CURRENT_WEIGHT.max(reactive.clamp(0.0, 1.0))
}

/// One pixel of the resolve, in linear HDR: `current` is the centre of
/// `neighbours` (the 3 x 3 block including it), `history` the reprojected
/// previous result. The returned history and output colours are
/// `(compressed, linear)`.
pub fn resolve_pixel(
    current: [f32; 3],
    neighbours: &[[f32; 3]],
    history: [f32; 3],
    weight: f32,
) -> ([f32; 3], [f32; 3]) {
    let compressed: Vec<[f32; 3]> = neighbours.iter().map(|&n| compress(n)).collect();
    let (lo, hi) = neighbourhood_box(&compressed);
    let h = clamp_history(history, lo, hi);
    let c = compress(current);
    let mixed = [
        h[0] + (c[0] - h[0]) * weight,
        h[1] + (c[1] - h[1]) * weight,
        h[2] + (c[2] - h[2]) * weight,
    ];
    (mixed, decompress(mixed))
}

/// What one frame does with the history images.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaaPlan {
    /// The history holds a valid previous result.
    pub use_history: bool,
    /// Index (0 or 1) of the history image read this frame.
    pub read: usize,
    /// Index of the one written this frame.
    pub write: usize,
}

/// Which history image is current and whether it is valid.
#[derive(Debug, Default)]
pub struct TaaState {
    extent: Option<(u32, u32)>,
    valid: bool,
    read: usize,
}

impl TaaState {
    /// Plans the next frame at `extent`; `reset` (respawn, teleport) and a
    /// size change discard the history.
    pub fn begin_frame(&mut self, extent: (u32, u32), reset: bool) -> TaaPlan {
        if reset || self.extent != Some(extent) {
            self.valid = false;
        }
        self.extent = Some(extent);
        let plan = TaaPlan {
            use_history: self.valid,
            read: self.read,
            write: 1 - self.read,
        };
        // This frame's result becomes the next frame's history.
        self.read = plan.write;
        self.valid = true;
        plan
    }

    /// Forgets everything (the history images are gone).
    pub fn invalidate(&mut self) {
        self.extent = None;
        self.valid = false;
    }
}

/// Temporal anti-aliasing as an [`Upscaler`]: always at the display
/// resolution, whatever the quality.
pub struct Taa {
    layout: PassLayout,
    pipeline: vk::Pipeline,
    pools: Option<FramePools>,
    targets: Option<Targets>,
    state: TaaState,
    // Last: dropped after `Drop for Taa` freed the rest.
    gpu: Gpu,
}

struct Targets {
    extent: vk::Extent2D,
    output: Image,
    history: [Image; 2],
}

impl Taa {
    pub fn new(handles: &GpuHandles) -> Result<Self, UpscaleError> {
        let gpu = Gpu::new(handles)?;
        let layout = PassLayout::new(&gpu, 5)?;
        let pipeline = layout.pipeline(
            &gpu.device,
            faset_shaders::TAA_VS,
            faset_shaders::TAA_FS,
            c"fs_main",
            2,
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
            targets: None,
            state: TaaState::default(),
            gpu,
        })
    }

    fn ensure_targets(&mut self, extent: vk::Extent2D) -> Result<(), UpscaleError> {
        if self.targets.as_ref().is_some_and(|t| t.extent == extent) {
            return Ok(());
        }
        self.gpu.wait_idle();
        self.release_targets();
        self.state.invalidate();
        let usage = vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::SAMPLED;
        let mut made: Vec<Image> = Vec::new();
        for name in ["taa-output", "taa-history-0", "taa-history-1"] {
            match self.gpu.create_image(name, extent, usage) {
                Ok(img) => made.push(img),
                Err(e) => {
                    for mut img in made {
                        self.gpu.destroy_image(&mut img);
                    }
                    return Err(e);
                }
            }
        }
        let mut it = made.into_iter();
        let output = it.next().expect("three images");
        let history = [
            it.next().expect("three images"),
            it.next().expect("three images"),
        ];
        self.targets = Some(Targets {
            extent,
            output,
            history,
        });
        Ok(())
    }

    fn release_targets(&mut self) {
        if let Some(mut t) = self.targets.take() {
            self.gpu.destroy_image(&mut t.output);
            for h in &mut t.history {
                self.gpu.destroy_image(h);
            }
        }
    }

    fn record(
        &mut self,
        ctx: &mut FrameCtx<'_>,
        inputs: &UpscaleInputs,
    ) -> Result<ImageHandle, UpscaleError> {
        let extent = inputs.colour.extent;
        self.ensure_targets(extent)?;
        if self.pools.is_none() {
            self.pools = Some(FramePools::new(
                &self.gpu.device,
                ctx.frames_in_flight,
                self.layout.images,
                1,
            )?);
        }
        let plan = self
            .state
            .begin_frame((extent.width, extent.height), inputs.reset);
        let targets = self.targets.as_ref().expect("created above");
        let pools = self.pools.as_ref().expect("created above");
        let device = ctx.device;
        let cmd = ctx.cmd;
        let read = &targets.history[plan.read];
        let write = &targets.history[plan.write];
        let reactive = inputs.reactive.unwrap_or(inputs.motion);
        let views = [
            (inputs.colour.view, READ),
            (read.view, READ),
            (
                inputs.depth.view,
                vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL,
            ),
            (inputs.motion.view, READ),
            (reactive.view, READ),
        ];
        let push = [
            1.0 / extent.width as f32,
            1.0 / extent.height as f32,
            f32::from(u8::from(plan.use_history)),
            CURRENT_WEIGHT,
            f32::from(u8::from(inputs.reactive.is_some())),
            0.0,
            0.0,
            0.0,
        ];
        // SAFETY: the slot's fence was waited on by the renderer; every
        // image is live; the command buffer is recording outside a rendering.
        unsafe {
            pools.reset(device, ctx.slot)?;
            let set = pools.set(device, &self.layout, ctx.slot, &views)?;
            // The history read is a valid image only after a frame wrote it.
            let old = if plan.use_history { READ } else { UNDEFINED };
            transition(device, cmd, read.image, old, READ);
            transition(device, cmd, write.image, UNDEFINED, ATTACHMENT);
            transition(device, cmd, targets.output.image, UNDEFINED, ATTACHMENT);
            Pass {
                pipeline: self.pipeline,
                layout: &self.layout,
                set,
                targets: &[targets.output.view, write.view],
                extent,
                push: &push,
            }
            .record(&self.gpu, cmd);
            transition(device, cmd, write.image, ATTACHMENT, READ);
            transition(device, cmd, targets.output.image, ATTACHMENT, READ);
        }
        Ok(targets.output.handle())
    }
}

impl Upscaler for Taa {
    fn render_extent(&self, display: Extent2D, _quality: Quality) -> Extent2D {
        scaled_extent(display, Quality::Native)
    }

    fn jitter(&self, frame_index: u32) -> Vec2 {
        faset_render::jitter(frame_index)
    }

    fn execute(&mut self, ctx: &mut FrameCtx<'_>, inputs: &UpscaleInputs) -> ImageHandle {
        match self.record(ctx, inputs) {
            Ok(handle) => handle,
            Err(e) => {
                tracing::error!("TAA failed, passing the scene through: {e}");
                inputs.colour
            }
        }
    }
}

impl Drop for Taa {
    fn drop(&mut self) {
        self.release_targets();
        if let Some(mut pools) = self.pools.take() {
            pools.destroy(&self.gpu.device);
        }
        // SAFETY: the renderer waited for the device before dropping us.
        unsafe { self.gpu.device.destroy_pipeline(self.pipeline, None) };
        self.layout.destroy(&self.gpu.device);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter()
            .zip(b)
            .all(|(x, y)| (x - y).abs() < 1e-4 * (1.0 + y.abs()))
    }

    #[test]
    fn compression_round_trips() {
        for c in [
            [0.0; 3],
            [0.2, 0.5, 0.9],
            [4.0, 0.1, 0.0],
            [300.0, 2.0, 70.0],
        ] {
            let z = compress(c);
            assert!(z.iter().all(|&v| (0.0..1.0).contains(&v)), "{z:?}");
            assert!(close(decompress(z), c), "{c:?} -> {z:?}");
        }
        // Non-finite and absurd values stay finite.
        let z = compress([f32::INFINITY, -1.0, 1.0e30]);
        assert!(z.iter().all(|v| v.is_finite() && *v >= 0.0));
    }

    #[test]
    fn ycocg_round_trips() {
        for c in [[0.1, 0.5, 0.9], [1.0, 0.0, 0.0], [0.3; 3]] {
            assert!(close(ycocg_to_rgb(rgb_to_ycocg(c)), c));
        }
        // Grey has no chroma.
        let y = rgb_to_ycocg([0.4; 3]);
        assert!(y[1].abs() < 1e-6 && y[2].abs() < 1e-6);
    }

    #[test]
    fn history_outside_the_neighbourhood_is_clamped_into_the_box() {
        let dark = [0.1, 0.1, 0.1];
        let neighbours = [dark; 9];
        // A bright ghost where the neighbourhood is dark.
        let ghost = compress([8.0, 8.0, 8.0]);
        let (history, out) = resolve_pixel(dark, &neighbours, ghost, CURRENT_WEIGHT);
        // The clamped history equals the dark neighbourhood: no trail.
        assert!(close(decompress(history), dark), "{history:?}");
        assert!(close(out, dark), "{out:?}");
        // A history inside the box is kept, then blended at 10 %.
        let a = compress([0.1; 3]);
        let b = compress([0.5; 3]);
        let mixed = [a, b, a, b, a, b, a, b, a];
        let inside = compress([0.3; 3]);
        let (h, _) = resolve_pixel([0.1; 3], &mixed, inside, CURRENT_WEIGHT);
        let expected = inside[0] + (a[0] - inside[0]) * CURRENT_WEIGHT;
        assert!((h[0] - expected).abs() < 1e-5);
    }

    #[test]
    fn the_current_frame_blends_in_at_ten_percent() {
        assert_eq!(CURRENT_WEIGHT, 0.1);
        assert_eq!(current_weight(true, true, 0.0), CURRENT_WEIGHT);
        // The reactive mask only raises it.
        assert_eq!(current_weight(true, true, 0.05), CURRENT_WEIGHT);
        assert_eq!(current_weight(true, true, 0.7), 0.7);
        assert_eq!(current_weight(true, true, 5.0), 1.0);
    }

    #[test]
    fn an_unusable_history_gives_the_current_frame() {
        assert_eq!(current_weight(false, true, 0.0), 1.0);
        assert_eq!(current_weight(true, false, 0.0), 1.0);
        let cur = [0.4, 0.2, 0.6];
        let (hist, out) = resolve_pixel(cur, &[cur; 9], compress([9.0; 3]), 1.0);
        assert!(close(out, cur), "{out:?}");
        assert!(close(decompress(hist), cur));
    }

    #[test]
    fn the_history_is_discarded_when_the_reset_flag_is_set() {
        let mut state = TaaState::default();
        let size = (1280, 720);
        // First frame: nothing to reuse.
        let first = state.begin_frame(size, false);
        assert!(!first.use_history);
        // Then the history is used, ping-ponging between the two images.
        let second = state.begin_frame(size, false);
        assert!(second.use_history);
        assert_eq!(second.read, first.write);
        assert_ne!(second.read, second.write);
        let third = state.begin_frame(size, false);
        assert!(third.use_history);
        assert_eq!(third.read, second.write);
        // Reset: discarded for that frame only.
        let reset = state.begin_frame(size, true);
        assert!(!reset.use_history);
        assert!(state.begin_frame(size, false).use_history);
    }

    #[test]
    fn a_new_size_discards_the_history_too() {
        let mut state = TaaState::default();
        state.begin_frame((640, 360), false);
        assert!(state.begin_frame((640, 360), false).use_history);
        assert!(!state.begin_frame((1280, 720), false).use_history);
        assert!(state.begin_frame((1280, 720), false).use_history);
        state.invalidate();
        assert!(!state.begin_frame((1280, 720), false).use_history);
    }

    #[test]
    fn the_reset_flag_forces_full_weight_on_the_current_frame() {
        let mut state = TaaState::default();
        state.begin_frame((8, 8), false);
        let plan = state.begin_frame((8, 8), true);
        assert_eq!(current_weight(plan.use_history, true, 0.0), 1.0);
        let plan = state.begin_frame((8, 8), false);
        assert_eq!(current_weight(plan.use_history, true, 0.0), CURRENT_WEIGHT);
    }
}

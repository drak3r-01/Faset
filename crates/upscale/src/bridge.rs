//! Plugs an [`Upscaler`] into the renderer's pass 6.

use faset_render::glam::Vec2;
use faset_render::{FrameCtx, ImageHandle, UpscaleInputs, UpscalePass};

use crate::{Extent2D, Quality, Upscaler, UpscalerKind};

/// An [`Upscaler`] with the quality it runs at, as the renderer's
/// [`UpscalePass`]. Build one with [`crate::Registry::create_pass`].
pub struct UpscaleBridge {
    kind: UpscalerKind,
    quality: Quality,
    upscaler: Box<dyn Upscaler>,
}

impl UpscaleBridge {
    pub fn new(kind: UpscalerKind, upscaler: Box<dyn Upscaler>, quality: Quality) -> Self {
        Self {
            kind,
            quality,
            upscaler,
        }
    }

    pub fn kind(&self) -> UpscalerKind {
        self.kind
    }

    pub fn quality(&self) -> Quality {
        self.quality
    }
}

impl UpscalePass for UpscaleBridge {
    fn render_extent(&self, display: Extent2D) -> Extent2D {
        self.upscaler.render_extent(display, self.quality)
    }

    fn jitter(&self, frame_index: u32) -> Vec2 {
        self.upscaler.jitter(frame_index)
    }

    fn is_temporal(&self) -> bool {
        self.kind.is_temporal()
    }

    fn execute(&mut self, ctx: &mut FrameCtx<'_>, inputs: &UpscaleInputs) -> ImageHandle {
        self.upscaler.execute(ctx, inputs)
    }
}

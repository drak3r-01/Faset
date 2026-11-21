//! Upscalers of the renderer: the [`Upscaler`] trait, the render scale,
//! temporal anti-aliasing and FSR 1.
//!
//! The scene is drawn at an internal resolution
//! ([`Upscaler::render_extent`]) and an upscaler turns the HDR colour, before
//! tonemapping, into an image at the display resolution (pass 6, between the
//! contours and the bloom, tonemapping, grading and screen effects of the
//! post chain). The interface is drawn afterwards at display resolution.
//!
//! * [`Native`]: 100 %, nothing happens (a free render scale below that is a
//!   bilinear stretch).
//! * [`Taa`]: temporal anti-aliasing at the display resolution; the history
//!   is reprojected with the motion vectors, clamped to the 3 x 3
//!   neighbourhood in YCoCg and blended at 10 %.
//! * [`Fsr1`]: FidelityFX Super Resolution 1, a WGSL port of EASU and RCAS.
//! * [`UpscalerKind::Fsr3`] (feature `fsr3`, on by default): FidelityFX Super
//!   Resolution 3.1 through AMD's official library, loaded at runtime on
//!   Windows x86_64 when `amd_fidelityfx_vk.dll` sits next to the
//!   executable; unavailable elsewhere.
//! * [`UpscalerKind::Dlss`] (feature `dlss`, off by default): NVIDIA DLSS
//!   through the NGX SDK, which the build script fetches and links on Windows
//!   x86_64; offered only on RTX GPUs where NGX runs DLSS, unavailable
//!   elsewhere.
//!
//! [`Registry`] lists the upscalers a GPU can run and falls back to FSR 1
//! when one fails. Hand the result to [`faset_render::Renderer::set_upscaler`]
//! and put [`faset_render::Renderer::upscaler_jitter`] in the camera.
//!
//! ```no_run
//! # fn demo(renderer: &mut faset_render::Renderer) -> Result<(), Box<dyn std::error::Error>> {
//! use faset_upscale::{Quality, Registry, UpscalerKind};
//!
//! let registry = Registry::builtin();
//! let (kind, pass) = registry.create_pass(
//!     UpscalerKind::Fsr1,
//!     Quality::Balanced,
//!     &renderer.gpu_handles(),
//! )?;
//! renderer.set_upscaler(Some(pass))?;
//! # let _ = kind;
//! # Ok(()) }
//! ```

mod bridge;
#[cfg(feature = "dlss")]
mod dlss;
mod error;
#[cfg(feature = "fsr3")]
mod ffx;
mod fsr1;
mod gpu;
#[cfg(any(feature = "fsr3", feature = "dlss"))]
mod jitter;
mod native;
mod quality;
mod registry;
pub mod taa;

pub use bridge::UpscaleBridge;
pub use error::UpscaleError;
pub use faset_render::{FrameCtx, GpuHandles, ImageHandle, UpscaleInputs, UpscalePass};
pub use fsr1::{easu_constants, rcas_constant, Fsr1, DEFAULT_SHARPNESS};
pub use native::Native;
pub use quality::{
    clamp_free_scale, effective_quality, scaled_extent, Extent2D, Quality, FREE_SCALE_MAX,
    FREE_SCALE_MIN,
};
pub use registry::{fallback_order, Backend, Created, Registry, UpscalerKind};
pub use taa::{Taa, TaaPlan, TaaState};

use faset_render::glam::Vec2;

/// A pass that turns the scene, drawn at [`Upscaler::render_extent`], into an
/// image at the display resolution.
pub trait Upscaler {
    /// The internal resolution for a display size at `quality`.
    fn render_extent(&self, display: Extent2D, quality: Quality) -> Extent2D;

    /// Sub-pixel jitter of a frame, in internal pixels (zero for spatial
    /// upscalers); set it as the camera's `jitter_px`.
    fn jitter(&self, frame_index: u32) -> Vec2;

    /// Records the pass and returns the image to feed to the rest of the
    /// frame, in `SHADER_READ_ONLY_OPTIMAL`.
    fn execute(&mut self, ctx: &mut FrameCtx<'_>, inputs: &UpscaleInputs) -> ImageHandle;
}

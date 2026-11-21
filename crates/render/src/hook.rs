//! Frame-graph pass 6: the hook through which an upscaler (lot 2.10) takes the
//! HDR colour of the scene, rendered at an internal resolution, and returns
//! an image at the display resolution.
//!
//! The renderer owns the Vulkan context; the upscaler crate depends on this
//! one, so the pieces it needs are plain data and one trait defined here:
//! [`FrameCtx`] (the command buffer being recorded), [`ImageHandle`] (a
//! borrowed image), [`UpscaleInputs`] and [`UpscalePass`].

use ash::vk;
use faset_core::glam::Vec2;

/// A borrowed image: the renderer or the upscaler owns the memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageHandle {
    pub image: vk::Image,
    pub view: vk::ImageView,
    pub extent: vk::Extent2D,
    pub format: vk::Format,
}

/// The frame being recorded.
#[derive(Clone, Copy)]
pub struct FrameCtx<'a> {
    pub device: &'a ash::Device,
    /// Primary command buffer in the recording state, outside a rendering.
    pub cmd: vk::CommandBuffer,
    /// Frame in flight the command buffer belongs to, in `0..frames_in_flight`.
    /// Per-frame resources indexed by it are free to reuse: the GPU is done
    /// with the previous use of the slot.
    pub slot: usize,
    pub frames_in_flight: usize,
    /// Frames submitted so far (the index of this one).
    pub frame_index: u64,
}

/// Everything an upscaler reads for one frame.
///
/// Layouts on entry: `colour`, `motion` and `reactive` are in
/// `SHADER_READ_ONLY_OPTIMAL`, `depth` in `DEPTH_STENCIL_READ_ONLY_OPTIMAL`.
/// All four have the internal (render) extent.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UpscaleInputs {
    /// Scene colour before tonemapping (RGBA16F, linear HDR, jittered). When
    /// the contour pass is on it already carries the outlines.
    pub colour: ImageHandle,
    /// Reverse-Z depth (`D32_SFLOAT`): near = 1, infinity = 0.
    pub depth: ImageHandle,
    /// Motion vectors (RG16F): `0.5 * (clip.xy / clip.w - prev_clip.xy /
    /// prev_clip.w)` from the unjittered matrices, in UV units per frame.
    pub motion: ImageHandle,
    /// Opacity of the weather particles (R8), the reactive mask.
    pub reactive: Option<ImageHandle>,
    /// Size of the image the upscaler must return.
    pub display: vk::Extent2D,
    /// Sub-pixel jitter applied to this frame, in internal pixels (x right,
    /// y down).
    pub jitter: Vec2,
    /// Exposure multiplier the tonemapping will apply.
    pub exposure: f32,
    /// Seconds since the previous frame.
    pub frame_time: f32,
    /// Near plane in metres.
    pub near: f32,
    /// Far plane in metres: infinite (the projection has none).
    pub far: f32,
    /// Vertical field of view in radians.
    pub fov_y: f32,
    /// Discard the temporal history (respawn, teleport, camera cut).
    pub reset: bool,
}

/// A pass between the scene and the post chain that turns the internal
/// resolution into the display resolution. `faset-upscale` implements it.
pub trait UpscalePass {
    /// Internal resolution the scene is rendered at for this display size.
    /// Clamped to `1..=display` by the renderer.
    fn render_extent(&self, display: vk::Extent2D) -> vk::Extent2D;

    /// Sub-pixel jitter, in internal pixels, for the given frame. Put it in
    /// `Camera::jitter_px`; `Vec2::ZERO` for spatial upscalers.
    fn jitter(&self, frame_index: u32) -> Vec2;

    /// True when the pass resolves anti-aliasing over time: the FXAA
    /// fallback of the post chain is then skipped.
    fn is_temporal(&self) -> bool;

    /// Records the pass and returns the image to feed to the post chain, in
    /// `SHADER_READ_ONLY_OPTIMAL`. It may be `inputs.colour` itself.
    fn execute(&mut self, ctx: &mut FrameCtx<'_>, inputs: &UpscaleInputs) -> ImageHandle;
}

/// Vulkan objects an upscaler needs to create its own resources. All are
/// cheap clones of the renderer's handles; the renderer outlives them.
#[derive(Clone)]
pub struct GpuHandles {
    pub instance: ash::Instance,
    pub device: ash::Device,
    pub physical: vk::PhysicalDevice,
    pub queue_family: u32,
}

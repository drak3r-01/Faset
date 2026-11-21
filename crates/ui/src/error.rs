use ash::vk;

/// Why the UI could not be built.
#[derive(Debug, thiserror::Error)]
pub enum UiError {
    #[error("Vulkan call failed: {0}")]
    Vulkan(#[from] vk::Result),
    #[error("GPU memory allocation failed: {0}")]
    Allocation(#[from] gpu_allocator::AllocationError),
    #[error("renderer error: {0}")]
    Render(#[from] faset_render::RenderError),
    #[error("unknown key name {0:?} for the overlay toggle")]
    UnknownKey(String),
}

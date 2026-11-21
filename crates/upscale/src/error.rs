use ash::vk;

use crate::UpscalerKind;

/// Why an upscaler could not be built or is unavailable.
#[derive(Debug, thiserror::Error)]
pub enum UpscaleError {
    #[error("Vulkan call failed: {0}")]
    Vulkan(#[from] vk::Result),
    #[error("GPU memory allocation failed: {0}")]
    Allocation(#[from] gpu_allocator::AllocationError),
    #[error("the GPU lacks {0}")]
    Unsupported(String),
    #[error("{} is unavailable: {reason}", kind.name())]
    Unavailable { kind: UpscalerKind, reason: String },
    #[error("no upscaler can run on this GPU")]
    NoUpscaler,
}

//! Per-frame resources and small command helpers.

use ash::vk;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme};
use gpu_allocator::MemoryLocation;

use crate::context::Context;
use crate::deferred::RingCursor;
use crate::gpu::GpuBuffer;
use crate::RenderError;

/// Size of each frame's uniform ring buffer.
pub(crate) const UNIFORM_RING_SIZE: u64 = 64 * 1024;

/// Host-visible uniform buffer sub-allocated per frame.
pub(crate) struct UniformRing {
    pub buffer: vk::Buffer,
    pub allocation: Option<Allocation>,
    pub cursor: RingCursor,
}

/// Everything one frame in flight owns.
pub(crate) struct FrameData {
    pub pool: vk::CommandPool,
    pub cmd: vk::CommandBuffer,
    /// Signalled by swapchain acquire (unused when headless).
    pub image_available: vk::Semaphore,
    /// Signalled when this frame's submission completes; created signalled.
    pub fence: vk::Fence,
    pub uniforms: UniformRing,
    /// Per-instance data of this frame (host-visible), grown on demand.
    pub instances: Option<GpuBuffer>,
}

impl FrameData {
    pub(crate) fn new(ctx: &mut Context, slot: usize) -> Result<Self, RenderError> {
        let device = &ctx.device;
        // SAFETY: valid device; every handle created here is destroyed in
        // `destroy` (or below on error).
        unsafe {
            let pool = device.create_command_pool(
                &vk::CommandPoolCreateInfo::default()
                    .flags(vk::CommandPoolCreateFlags::TRANSIENT)
                    .queue_family_index(ctx.queue_family),
                None,
            )?;
            let cmd = device.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1),
            )?[0];
            let image_available =
                device.create_semaphore(&vk::SemaphoreCreateInfo::default(), None)?;
            let fence = device.create_fence(
                &vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED),
                None,
            )?;
            let buffer = device.create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(UNIFORM_RING_SIZE)
                    .usage(vk::BufferUsageFlags::UNIFORM_BUFFER),
                None,
            )?;
            let mut frame = Self {
                pool,
                cmd,
                image_available,
                fence,
                uniforms: UniformRing {
                    buffer,
                    allocation: None,
                    cursor: RingCursor::new(UNIFORM_RING_SIZE),
                },
                instances: None,
            };
            let requirements = device.get_buffer_memory_requirements(buffer);
            let allocation = ctx.allocator.allocate(&AllocationCreateDesc {
                name: &format!("frame{slot}-uniform-ring"),
                requirements,
                location: MemoryLocation::CpuToGpu,
                linear: true,
                allocation_scheme: AllocationScheme::GpuAllocatorManaged,
            });
            let allocation = match allocation {
                Ok(a) => a,
                Err(e) => {
                    frame.destroy(ctx);
                    return Err(e.into());
                }
            };
            let bound = device.bind_buffer_memory(buffer, allocation.memory(), allocation.offset());
            frame.uniforms.allocation = Some(allocation);
            if let Err(e) = bound {
                frame.destroy(ctx);
                return Err(e.into());
            }
            Ok(frame)
        }
    }

    /// Destroys every handle. The GPU must be done with this frame.
    pub(crate) fn destroy(&mut self, ctx: &mut Context) {
        if let Some(mut b) = self.instances.take() {
            b.destroy(ctx);
        }
        if let Some(a) = self.uniforms.allocation.take() {
            if let Err(e) = ctx.allocator.free(a) {
                tracing::warn!("failed to free uniform ring: {e}");
            }
        }
        // SAFETY: the caller waited for the device to be idle.
        unsafe {
            ctx.device.destroy_buffer(self.uniforms.buffer, None);
            ctx.device.destroy_fence(self.fence, None);
            ctx.device.destroy_semaphore(self.image_available, None);
            ctx.device.destroy_command_pool(self.pool, None);
        }
    }
}

/// A colour image with its view and memory.
pub(crate) struct OwnedImage {
    pub image: vk::Image,
    pub view: vk::ImageView,
    pub allocation: Option<Allocation>,
}

impl OwnedImage {
    /// Destroys the image; handles become null so a second call is a no-op.
    pub(crate) fn destroy(&mut self, ctx: &mut Context) {
        // SAFETY: the deferred queue or an idle device guarantees no GPU use;
        // destroying null handles is a no-op.
        unsafe {
            ctx.device.destroy_image_view(self.view, None);
            ctx.device.destroy_image(self.image, None);
        }
        self.view = vk::ImageView::null();
        self.image = vk::Image::null();
        if let Some(a) = self.allocation.take() {
            if let Err(e) = ctx.allocator.free(a) {
                tracing::warn!("failed to free image memory: {e}");
            }
        }
    }
}

/// Resources waiting in the deferred destruction queue.
pub(crate) enum Garbage {
    Image(OwnedImage),
}

impl Garbage {
    pub(crate) fn destroy(self, ctx: &mut Context) {
        match self {
            Self::Image(mut image) => image.destroy(ctx),
        }
    }
}

/// Full colour subresource of a single-mip, single-layer image.
pub(crate) const COLOR_RANGE: vk::ImageSubresourceRange = vk::ImageSubresourceRange {
    aspect_mask: vk::ImageAspectFlags::COLOR,
    base_mip_level: 0,
    level_count: 1,
    base_array_layer: 0,
    layer_count: 1,
};

/// Creates a 2D colour view of `image`.
pub(crate) fn create_view(
    device: &ash::Device,
    image: vk::Image,
    format: vk::Format,
) -> Result<vk::ImageView, RenderError> {
    let info = vk::ImageViewCreateInfo::default()
        .image(image)
        .view_type(vk::ImageViewType::TYPE_2D)
        .format(format)
        .subresource_range(COLOR_RANGE);
    // SAFETY: `image` is a live image created with a compatible format.
    Ok(unsafe { device.create_image_view(&info, None) }?)
}

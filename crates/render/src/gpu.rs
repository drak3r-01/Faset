//! Small Vulkan helpers: buffers with memory, images of any aspect, and
//! batched image barriers.

use ash::vk;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme};
use gpu_allocator::MemoryLocation;

use crate::context::Context;
use crate::frame::OwnedImage;
use crate::RenderError;

/// A buffer with its memory.
pub(crate) struct GpuBuffer {
    pub buffer: vk::Buffer,
    pub allocation: Option<Allocation>,
    pub size: u64,
}

impl GpuBuffer {
    pub(crate) fn new(
        ctx: &mut Context,
        name: &str,
        size: u64,
        usage: vk::BufferUsageFlags,
        location: MemoryLocation,
    ) -> Result<Self, RenderError> {
        let info = vk::BufferCreateInfo::default()
            .size(size.max(4))
            .usage(usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        // SAFETY: valid device and create info.
        let buffer = unsafe { ctx.device.create_buffer(&info, None) }?;
        let mut out = Self {
            buffer,
            allocation: None,
            size,
        };
        // SAFETY: live buffer.
        let requirements = unsafe { ctx.device.get_buffer_memory_requirements(buffer) };
        let result = (|| -> Result<(), RenderError> {
            let allocation = ctx.allocator.allocate(&AllocationCreateDesc {
                name,
                requirements,
                location,
                linear: true,
                allocation_scheme: AllocationScheme::GpuAllocatorManaged,
            })?;
            // SAFETY: the memory satisfies `requirements`; the buffer is unbound.
            let bound = unsafe {
                ctx.device
                    .bind_buffer_memory(buffer, allocation.memory(), allocation.offset())
            };
            out.allocation = Some(allocation);
            Ok(bound?)
        })();
        if let Err(e) = result {
            out.destroy(ctx);
            return Err(e);
        }
        Ok(out)
    }

    /// Host-visible bytes of the buffer (CpuToGpu / GpuToCpu memory only).
    pub(crate) fn mapped(&mut self) -> &mut [u8] {
        self.allocation
            .as_mut()
            .and_then(|a| a.mapped_slice_mut())
            .expect("host-visible buffer")
    }

    /// Destroys the buffer. The GPU must be done with it.
    pub(crate) fn destroy(&mut self, ctx: &mut Context) {
        // SAFETY: the caller guarantees no GPU use; null is a no-op.
        unsafe { ctx.device.destroy_buffer(self.buffer, None) };
        self.buffer = vk::Buffer::null();
        if let Some(a) = self.allocation.take() {
            if let Err(e) = ctx.allocator.free(a) {
                tracing::warn!("failed to free buffer memory: {e}");
            }
        }
    }
}

/// Full subresource of a single-mip, single-layer image.
pub(crate) fn full_range(aspect: vk::ImageAspectFlags) -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange {
        aspect_mask: aspect,
        base_mip_level: 0,
        level_count: 1,
        base_array_layer: 0,
        layer_count: 1,
    }
}

/// Creates a 2D GPU-only image with a view.
pub(crate) fn create_image(
    ctx: &mut Context,
    name: &str,
    extent: vk::Extent2D,
    format: vk::Format,
    usage: vk::ImageUsageFlags,
    aspect: vk::ImageAspectFlags,
) -> Result<OwnedImage, RenderError> {
    let info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(format)
        .extent(extent.into())
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::OPTIMAL)
        .usage(usage)
        .sharing_mode(vk::SharingMode::EXCLUSIVE)
        .initial_layout(vk::ImageLayout::UNDEFINED);
    // SAFETY: valid device and create info.
    let image = unsafe { ctx.device.create_image(&info, None) }?;
    let mut owned = OwnedImage {
        image,
        view: vk::ImageView::null(),
        allocation: None,
    };
    // SAFETY: live image.
    let requirements = unsafe { ctx.device.get_image_memory_requirements(image) };
    let result = (|| -> Result<(), RenderError> {
        let allocation = ctx.allocator.allocate(&AllocationCreateDesc {
            name,
            requirements,
            location: MemoryLocation::GpuOnly,
            linear: false,
            allocation_scheme: AllocationScheme::GpuAllocatorManaged,
        })?;
        // SAFETY: memory satisfies `requirements`; the image is unbound.
        let bound = unsafe {
            ctx.device
                .bind_image_memory(image, allocation.memory(), allocation.offset())
        };
        owned.allocation = Some(allocation);
        bound?;
        let view_info = vk::ImageViewCreateInfo::default()
            .image(image)
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(format)
            .subresource_range(full_range(aspect));
        // SAFETY: live, bound image with a matching format.
        owned.view = unsafe { ctx.device.create_image_view(&view_info, None) }?;
        Ok(())
    })();
    if let Err(e) = result {
        owned.destroy(ctx);
        return Err(e);
    }
    Ok(owned)
}

/// One image layout change, recorded with others in a single barrier call.
pub(crate) struct ImageBarrier {
    pub image: vk::Image,
    pub aspect: vk::ImageAspectFlags,
    pub old: vk::ImageLayout,
    pub new: vk::ImageLayout,
    pub src_access: vk::AccessFlags,
    pub dst_access: vk::AccessFlags,
}

/// Records `barriers` between `src` and `dst` stages.
///
/// # Safety
/// `cmd` must be recording and every image must be live.
pub(crate) unsafe fn image_barriers(
    device: &ash::Device,
    cmd: vk::CommandBuffer,
    src: vk::PipelineStageFlags,
    dst: vk::PipelineStageFlags,
    barriers: &[ImageBarrier],
) {
    let vk_barriers: Vec<vk::ImageMemoryBarrier> = barriers
        .iter()
        .map(|b| {
            vk::ImageMemoryBarrier::default()
                .src_access_mask(b.src_access)
                .dst_access_mask(b.dst_access)
                .old_layout(b.old)
                .new_layout(b.new)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(b.image)
                .subresource_range(full_range(b.aspect))
        })
        .collect();
    // SAFETY: forwarded from the caller.
    unsafe {
        device.cmd_pipeline_barrier(
            cmd,
            src,
            dst,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &vk_barriers,
        );
    }
}

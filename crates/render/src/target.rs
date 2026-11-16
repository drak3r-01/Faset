//! Render targets: the window swapchain and the headless offscreen image.

use ash::{khr, vk};
use gpu_allocator::vulkan::{AllocationCreateDesc, AllocationScheme};
use gpu_allocator::MemoryLocation;

use crate::context::Context;
use crate::frame::{create_view, OwnedImage};
use crate::select;
use crate::RenderError;

/// Colour format of the offscreen target (8-bit sRGB, like the swapchain).
pub(crate) const OFFSCREEN_FORMAT: vk::Format = vk::Format::R8G8B8A8_SRGB;

pub(crate) struct Swapchain {
    pub handle: vk::SwapchainKHR,
    pub format: vk::Format,
    pub extent: vk::Extent2D,
    pub images: Vec<vk::Image>,
    pub views: Vec<vk::ImageView>,
    /// One per swapchain image: a semaphore may only be re-signalled once the
    /// presentation that waited on it has consumed it.
    pub render_finished: Vec<vk::Semaphore>,
}

impl Swapchain {
    /// Creates a swapchain for `size`, or `Ok(None)` when the surface has a
    /// zero extent (minimised). `old` is retired by the caller afterwards.
    pub(crate) fn new(
        ctx: &Context,
        loader: &khr::swapchain::Device,
        size: (u32, u32),
        vsync: bool,
        old: vk::SwapchainKHR,
    ) -> Result<Option<Self>, RenderError> {
        let surface = ctx
            .surface
            .as_ref()
            .expect("swapchain needs a windowed context");
        // SAFETY: surface and physical device belong to this context.
        let (caps, formats, modes) = unsafe {
            (
                surface
                    .loader
                    .get_physical_device_surface_capabilities(ctx.physical, surface.handle)?,
                surface
                    .loader
                    .get_physical_device_surface_formats(ctx.physical, surface.handle)?,
                surface
                    .loader
                    .get_physical_device_surface_present_modes(ctx.physical, surface.handle)?,
            )
        };
        let extent = select::swapchain_extent(&caps, size);
        if select::is_zero_area((extent.width, extent.height)) {
            return Ok(None);
        }
        let format = select::choose_surface_format(&formats).ok_or(RenderError::NoSrgbFormat)?;
        let present_mode = select::choose_present_mode(vsync, &modes);
        // The opaque pass result reaches the swapchain through a blit.
        if !caps
            .supported_usage_flags
            .contains(vk::ImageUsageFlags::TRANSFER_DST)
        {
            return Err(RenderError::SwapchainUsage);
        }
        let info = vk::SwapchainCreateInfoKHR::default()
            .surface(surface.handle)
            .min_image_count(select::swapchain_image_count(&caps))
            .image_format(format.format)
            .image_color_space(format.color_space)
            .image_extent(extent)
            .image_array_layers(1)
            .image_usage(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_DST)
            .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
            .pre_transform(caps.current_transform)
            .composite_alpha(vk::CompositeAlphaFlagsKHR::OPAQUE)
            .present_mode(present_mode)
            .clipped(true)
            .old_swapchain(old);
        // SAFETY: parameters were validated against the surface capabilities.
        let handle = unsafe { loader.create_swapchain(&info, None) }?;
        let mut swapchain = Self {
            handle,
            format: format.format,
            extent,
            images: Vec::new(),
            views: Vec::new(),
            render_finished: Vec::new(),
        };
        if let Err(e) = swapchain.create_image_resources(ctx, loader) {
            swapchain.destroy(ctx, loader);
            return Err(e);
        }
        tracing::debug!(
            width = extent.width,
            height = extent.height,
            ?present_mode,
            format = ?format.format,
            images = swapchain.images.len(),
            "swapchain created"
        );
        Ok(Some(swapchain))
    }

    fn create_image_resources(
        &mut self,
        ctx: &Context,
        loader: &khr::swapchain::Device,
    ) -> Result<(), RenderError> {
        // SAFETY: live swapchain.
        self.images = unsafe { loader.get_swapchain_images(self.handle) }?;
        for &image in &self.images {
            self.views
                .push(create_view(&ctx.device, image, self.format)?);
            // SAFETY: valid device.
            let semaphore = unsafe {
                ctx.device
                    .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)
            }?;
            self.render_finished.push(semaphore);
        }
        Ok(())
    }

    /// Destroys views, semaphores and the swapchain. The device must be idle.
    pub(crate) fn destroy(&mut self, ctx: &Context, loader: &khr::swapchain::Device) {
        // SAFETY: the caller waited for the device to be idle.
        unsafe {
            for view in self.views.drain(..) {
                ctx.device.destroy_image_view(view, None);
            }
            for semaphore in self.render_finished.drain(..) {
                ctx.device.destroy_semaphore(semaphore, None);
            }
            loader.destroy_swapchain(self.handle, None);
        }
    }
}

/// Headless colour target, copied back to the host on demand.
pub(crate) struct Offscreen {
    pub image: OwnedImage,
    pub extent: vk::Extent2D,
    /// A frame was rendered into this image since it was created.
    pub rendered: bool,
}

impl Offscreen {
    pub(crate) fn new(ctx: &mut Context, (width, height): (u32, u32)) -> Result<Self, RenderError> {
        let extent = vk::Extent2D { width, height };
        let info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(OFFSCREEN_FORMAT)
            .extent(extent.into())
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(
                vk::ImageUsageFlags::COLOR_ATTACHMENT
                    | vk::ImageUsageFlags::TRANSFER_SRC
                    | vk::ImageUsageFlags::TRANSFER_DST,
            )
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
                name: "offscreen-color",
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
            owned.view = create_view(&ctx.device, image, OFFSCREEN_FORMAT)?;
            Ok(())
        })();
        if let Err(e) = result {
            owned.destroy(ctx);
            return Err(e);
        }
        Ok(Self {
            image: owned,
            extent,
            rendered: false,
        })
    }
}

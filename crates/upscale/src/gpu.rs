//! Vulkan plumbing shared by the upscalers: an allocator, images, full-screen
//! passes over dynamic rendering, and per-frame descriptor sets.

use std::ffi::CStr;
use std::mem::ManuallyDrop;

use ash::{khr, vk};
use faset_render::GpuHandles;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme, Allocator};
use gpu_allocator::MemoryLocation;

use crate::UpscaleError;

/// Format of the scene colour and of every image the upscalers produce.
pub(crate) const HDR_FORMAT: vk::Format = vk::Format::R16G16B16A16_SFLOAT;

/// Bytes of the push constant block of every pass (four `vec4`).
pub(crate) const PUSH_BYTES: u32 = 64;

/// Checks the format features every upscaler relies on.
pub(crate) fn check_support(gpu: &GpuHandles) -> Result<(), UpscaleError> {
    // SAFETY: the physical device was enumerated from this instance.
    let props = unsafe {
        gpu.instance
            .get_physical_device_format_properties(gpu.physical, HDR_FORMAT)
    };
    let needed = vk::FormatFeatureFlags::COLOR_ATTACHMENT
        | vk::FormatFeatureFlags::SAMPLED_IMAGE
        | vk::FormatFeatureFlags::SAMPLED_IMAGE_FILTER_LINEAR;
    if props.optimal_tiling_features.contains(needed) {
        Ok(())
    } else {
        Err(UpscaleError::Unsupported(
            "RGBA16F colour attachments that can be sampled with a linear filter".into(),
        ))
    }
}

/// An image with its view and memory.
pub(crate) struct Image {
    pub image: vk::Image,
    pub view: vk::ImageView,
    pub extent: vk::Extent2D,
    allocation: Option<Allocation>,
}

impl Image {
    pub(crate) fn handle(&self) -> faset_render::ImageHandle {
        faset_render::ImageHandle {
            image: self.image,
            view: self.view,
            extent: self.extent,
            format: HDR_FORMAT,
        }
    }
}

/// The device, an allocator of its own and a linear clamp sampler.
///
/// The renderer drops its upscaler before it destroys the device, so every
/// object created here is freed while the device is alive.
pub(crate) struct Gpu {
    pub device: ash::Device,
    pub dr: khr::dynamic_rendering::Device,
    pub sampler: vk::Sampler,
    allocator: ManuallyDrop<Allocator>,
}

impl Gpu {
    pub(crate) fn new(h: &GpuHandles) -> Result<Self, UpscaleError> {
        check_support(h)?;
        let allocator = Allocator::new(&gpu_allocator::vulkan::AllocatorCreateDesc {
            instance: h.instance.clone(),
            device: h.device.clone(),
            physical_device: h.physical,
            debug_settings: Default::default(),
            buffer_device_address: false,
            allocation_sizes: Default::default(),
        })?;
        let dr = khr::dynamic_rendering::Device::new(&h.instance, &h.device);
        // SAFETY: valid device and create info.
        let sampler = unsafe {
            h.device.create_sampler(
                &vk::SamplerCreateInfo::default()
                    .mag_filter(vk::Filter::LINEAR)
                    .min_filter(vk::Filter::LINEAR)
                    .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
                    .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE),
                None,
            )
        };
        let sampler = match sampler {
            Ok(s) => s,
            Err(e) => {
                drop(allocator);
                return Err(e.into());
            }
        };
        Ok(Self {
            device: h.device.clone(),
            dr,
            sampler,
            allocator: ManuallyDrop::new(allocator),
        })
    }

    /// A 2D GPU-only colour image of [`HDR_FORMAT`] (or `format`) with a view.
    pub(crate) fn create_image(
        &mut self,
        name: &str,
        extent: vk::Extent2D,
        usage: vk::ImageUsageFlags,
    ) -> Result<Image, UpscaleError> {
        let info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(HDR_FORMAT)
            .extent(extent.into())
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            // The renderer blits the result to the target.
            .usage(usage | vk::ImageUsageFlags::TRANSFER_SRC)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        // SAFETY: valid device and create info.
        let image = unsafe { self.device.create_image(&info, None) }?;
        let mut out = Image {
            image,
            view: vk::ImageView::null(),
            extent,
            allocation: None,
        };
        let result = (|| -> Result<(), UpscaleError> {
            // SAFETY: live image.
            let requirements = unsafe { self.device.get_image_memory_requirements(image) };
            let allocation = self.allocator.allocate(&AllocationCreateDesc {
                name,
                requirements,
                location: MemoryLocation::GpuOnly,
                linear: false,
                allocation_scheme: AllocationScheme::GpuAllocatorManaged,
            })?;
            // SAFETY: the memory satisfies `requirements`; the image is unbound.
            let bound = unsafe {
                self.device
                    .bind_image_memory(image, allocation.memory(), allocation.offset())
            };
            out.allocation = Some(allocation);
            bound?;
            let view = vk::ImageViewCreateInfo::default()
                .image(image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(HDR_FORMAT)
                .subresource_range(color_range());
            // SAFETY: live, bound image with a matching format.
            out.view = unsafe { self.device.create_image_view(&view, None) }?;
            Ok(())
        })();
        if let Err(e) = result {
            self.destroy_image(&mut out);
            return Err(e);
        }
        Ok(out)
    }

    /// Destroys an image; a second call is a no-op. The GPU must be done
    /// with it.
    pub(crate) fn destroy_image(&mut self, image: &mut Image) {
        // SAFETY: the caller guarantees no GPU use; null handles are a no-op.
        unsafe {
            self.device.destroy_image_view(image.view, None);
            self.device.destroy_image(image.image, None);
        }
        image.view = vk::ImageView::null();
        image.image = vk::Image::null();
        if let Some(a) = image.allocation.take() {
            if let Err(e) = self.allocator.free(a) {
                tracing::warn!("failed to free image memory: {e}");
            }
        }
    }

    /// Waits for the device, so images and descriptor sets can be replaced.
    pub(crate) fn wait_idle(&self) {
        // SAFETY: valid device.
        if let Err(e) = unsafe { self.device.device_wait_idle() } {
            tracing::warn!("device_wait_idle failed: {e}");
        }
    }
}

impl Drop for Gpu {
    fn drop(&mut self) {
        // SAFETY: the owner destroyed its images first; nothing uses the
        // sampler any more; the allocator is dropped exactly once, here.
        unsafe {
            self.device.destroy_sampler(self.sampler, None);
            ManuallyDrop::drop(&mut self.allocator);
        }
    }
}

pub(crate) fn color_range() -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange {
        aspect_mask: vk::ImageAspectFlags::COLOR,
        base_mip_level: 0,
        level_count: 1,
        base_array_layer: 0,
        layer_count: 1,
    }
}

/// Moves a colour image between layouts, with conservative stages.
///
/// # Safety
/// `cmd` must be recording on a queue of `device`.
pub(crate) unsafe fn transition(
    device: &ash::Device,
    cmd: vk::CommandBuffer,
    image: vk::Image,
    old: vk::ImageLayout,
    new: vk::ImageLayout,
) {
    let barrier = vk::ImageMemoryBarrier::default()
        .src_access_mask(vk::AccessFlags::MEMORY_WRITE)
        .dst_access_mask(vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE)
        .old_layout(old)
        .new_layout(new)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(image)
        .subresource_range(color_range());
    // SAFETY: forwarded.
    unsafe {
        device.cmd_pipeline_barrier(
            cmd,
            vk::PipelineStageFlags::ALL_COMMANDS,
            vk::PipelineStageFlags::ALL_COMMANDS,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[barrier],
        );
    }
}

/// Descriptor set and pipeline layout of passes that read `images` sampled
/// images (bindings `0..images`) and a sampler (binding `images`), and take
/// [`PUSH_BYTES`] of fragment push constants.
pub(crate) struct PassLayout {
    pub set_layout: vk::DescriptorSetLayout,
    pub layout: vk::PipelineLayout,
    pub images: u32,
}

impl PassLayout {
    pub(crate) fn new(gpu: &Gpu, images: u32) -> Result<Self, UpscaleError> {
        let samplers = [gpu.sampler];
        let mut bindings: Vec<vk::DescriptorSetLayoutBinding> = (0..images)
            .map(|binding| {
                vk::DescriptorSetLayoutBinding::default()
                    .binding(binding)
                    .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::FRAGMENT)
            })
            .collect();
        bindings.push(
            vk::DescriptorSetLayoutBinding::default()
                .binding(images)
                .descriptor_type(vk::DescriptorType::SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT)
                .immutable_samplers(&samplers),
        );
        // SAFETY: valid device and create info.
        let set_layout = unsafe {
            gpu.device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                None,
            )
        }?;
        let set_layouts = [set_layout];
        let ranges = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::FRAGMENT)
            .offset(0)
            .size(PUSH_BYTES)];
        // SAFETY: valid device and create info.
        let layout = unsafe {
            gpu.device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .set_layouts(&set_layouts)
                    .push_constant_ranges(&ranges),
                None,
            )
        };
        match layout {
            Ok(layout) => Ok(Self {
                set_layout,
                layout,
                images,
            }),
            Err(e) => {
                // SAFETY: unused layout.
                unsafe { gpu.device.destroy_descriptor_set_layout(set_layout, None) };
                Err(e.into())
            }
        }
    }

    pub(crate) fn destroy(&self, device: &ash::Device) {
        // SAFETY: the device is idle.
        unsafe {
            device.destroy_pipeline_layout(self.layout, None);
            device.destroy_descriptor_set_layout(self.set_layout, None);
        }
    }

    /// A full-screen pipeline writing `attachments` colour targets of
    /// [`HDR_FORMAT`].
    pub(crate) fn pipeline(
        &self,
        device: &ash::Device,
        vs: &[u32],
        fs: &[u32],
        fs_entry: &CStr,
        attachments: usize,
    ) -> Result<vk::Pipeline, UpscaleError> {
        let module = |words: &[u32]| {
            // SAFETY: SPIR-V produced and validated by naga.
            unsafe {
                device
                    .create_shader_module(&vk::ShaderModuleCreateInfo::default().code(words), None)
            }
        };
        let vs_module = module(vs)?;
        let fs_module = match module(fs) {
            Ok(m) => m,
            Err(e) => {
                // SAFETY: unused module.
                unsafe { device.destroy_shader_module(vs_module, None) };
                return Err(e.into());
            }
        };
        let stages = [
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::VERTEX)
                .module(vs_module)
                .name(c"vs_main"),
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .module(fs_module)
                .name(fs_entry),
        ];
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let viewport = vk::PipelineViewportStateCreateInfo::default()
            .viewport_count(1)
            .scissor_count(1);
        let raster = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::NONE)
            .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let blend_attachments = vec![
            vk::PipelineColorBlendAttachmentState::default()
                .color_write_mask(vk::ColorComponentFlags::RGBA);
            attachments
        ];
        let blend =
            vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachments);
        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
        let formats = vec![HDR_FORMAT; attachments];
        let mut rendering =
            vk::PipelineRenderingCreateInfo::default().color_attachment_formats(&formats);
        let info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&input_assembly)
            .viewport_state(&viewport)
            .rasterization_state(&raster)
            .multisample_state(&multisample)
            .color_blend_state(&blend)
            .dynamic_state(&dynamic)
            .layout(self.layout)
            .push_next(&mut rendering);
        // SAFETY: every pointer in `info` outlives the call.
        let created =
            unsafe { device.create_graphics_pipelines(vk::PipelineCache::null(), &[info], None) };
        // SAFETY: modules may be destroyed once pipelines are created.
        unsafe {
            device.destroy_shader_module(vs_module, None);
            device.destroy_shader_module(fs_module, None);
        }
        Ok(created.map_err(|(_, e)| e)?[0])
    }
}

/// One descriptor pool per frame in flight, reset and refilled each frame:
/// the images a pass reads change from frame to frame.
pub(crate) struct FramePools {
    pools: Vec<vk::DescriptorPool>,
}

impl FramePools {
    pub(crate) fn new(
        device: &ash::Device,
        frames: usize,
        images: u32,
        sets: u32,
    ) -> Result<Self, UpscaleError> {
        let mut out = Self { pools: Vec::new() };
        let sizes = [
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::SAMPLED_IMAGE)
                .descriptor_count(images * sets),
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::SAMPLER)
                .descriptor_count(sets),
        ];
        for _ in 0..frames.max(1) {
            // SAFETY: valid device and create info.
            let pool = unsafe {
                device.create_descriptor_pool(
                    &vk::DescriptorPoolCreateInfo::default()
                        .max_sets(sets)
                        .pool_sizes(&sizes),
                    None,
                )
            };
            match pool {
                Ok(p) => out.pools.push(p),
                Err(e) => {
                    out.destroy(device);
                    return Err(e.into());
                }
            }
        }
        Ok(out)
    }

    /// Frees every set of `slot`.
    ///
    /// # Safety
    /// The GPU must be done with the previous use of the slot.
    pub(crate) unsafe fn reset(
        &self,
        device: &ash::Device,
        slot: usize,
    ) -> Result<(), UpscaleError> {
        let pool = self.pools[slot % self.pools.len()];
        // SAFETY: forwarded.
        unsafe { device.reset_descriptor_pool(pool, vk::DescriptorPoolResetFlags::empty()) }?;
        Ok(())
    }

    /// A set of `layout` over `views` (view, layout), for `slot`.
    ///
    /// # Safety
    /// The views must be live; the slot was [`reset`](Self::reset) this frame.
    pub(crate) unsafe fn set(
        &self,
        device: &ash::Device,
        layout: &PassLayout,
        slot: usize,
        views: &[(vk::ImageView, vk::ImageLayout)],
    ) -> Result<vk::DescriptorSet, UpscaleError> {
        debug_assert_eq!(views.len() as u32, layout.images);
        let pool = self.pools[slot % self.pools.len()];
        let layouts = [layout.set_layout];
        // SAFETY: the pool holds enough sets for a frame.
        let set = unsafe {
            device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(pool)
                    .set_layouts(&layouts),
            )
        }?[0];
        let infos: Vec<vk::DescriptorImageInfo> = views
            .iter()
            .map(|&(view, layout)| {
                vk::DescriptorImageInfo::default()
                    .image_view(view)
                    .image_layout(layout)
            })
            .collect();
        let writes: Vec<vk::WriteDescriptorSet> = infos
            .iter()
            .enumerate()
            .map(|(binding, info)| {
                vk::WriteDescriptorSet::default()
                    .dst_set(set)
                    .dst_binding(binding as u32)
                    .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                    .image_info(std::slice::from_ref(info))
            })
            .collect();
        // SAFETY: live set and views; `infos` outlives the call.
        unsafe { device.update_descriptor_sets(&writes, &[]) };
        Ok(set)
    }

    pub(crate) fn destroy(&mut self, device: &ash::Device) {
        for pool in self.pools.drain(..) {
            // SAFETY: the device is idle; destroying a pool frees its sets.
            unsafe { device.destroy_descriptor_pool(pool, None) };
        }
    }
}

/// One full-screen pass.
pub(crate) struct Pass<'a> {
    pub pipeline: vk::Pipeline,
    pub layout: &'a PassLayout,
    pub set: vk::DescriptorSet,
    /// Colour targets, in `COLOR_ATTACHMENT_OPTIMAL`.
    pub targets: &'a [vk::ImageView],
    pub extent: vk::Extent2D,
    pub push: &'a [f32],
}

impl Pass<'_> {
    /// Records the pass: begin rendering, one triangle, end rendering.
    ///
    /// # Safety
    /// `cmd` must be recording outside a rendering; every object must be
    /// live and the images in the layouts the pass expects.
    pub(crate) unsafe fn record(&self, gpu: &Gpu, cmd: vk::CommandBuffer) {
        let area = vk::Rect2D {
            offset: vk::Offset2D::default(),
            extent: self.extent,
        };
        let attachments: Vec<vk::RenderingAttachmentInfo> = self
            .targets
            .iter()
            .map(|&view| {
                vk::RenderingAttachmentInfo::default()
                    .image_view(view)
                    .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                    .load_op(vk::AttachmentLoadOp::DONT_CARE)
                    .store_op(vk::AttachmentStoreOp::STORE)
            })
            .collect();
        let info = vk::RenderingInfo::default()
            .render_area(area)
            .layer_count(1)
            .color_attachments(&attachments);
        let viewport = vk::Viewport {
            x: 0.0,
            y: 0.0,
            width: self.extent.width as f32,
            height: self.extent.height as f32,
            min_depth: 0.0,
            max_depth: 1.0,
        };
        let mut push = [0.0f32; (PUSH_BYTES / 4) as usize];
        let n = self.push.len().min(push.len());
        push[..n].copy_from_slice(&self.push[..n]);
        let device = &gpu.device;
        // SAFETY: forwarded from the caller.
        unsafe {
            gpu.dr.cmd_begin_rendering(cmd, &info);
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            device.cmd_set_viewport(cmd, 0, &[viewport]);
            device.cmd_set_scissor(cmd, 0, &[area]);
            device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.layout.layout,
                0,
                &[self.set],
                &[],
            );
            device.cmd_push_constants(
                cmd,
                self.layout.layout,
                vk::ShaderStageFlags::FRAGMENT,
                0,
                bytemuck::cast_slice(&push),
            );
            device.cmd_draw(cmd, 3, 1, 0, 0);
            gpu.dr.cmd_end_rendering(cmd);
        }
    }
}

/// Layout of an image the upscalers read.
pub(crate) const READ: vk::ImageLayout = vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL;
pub(crate) const ATTACHMENT: vk::ImageLayout = vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL;
pub(crate) const UNDEFINED: vk::ImageLayout = vk::ImageLayout::UNDEFINED;

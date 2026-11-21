//! The Vulkan paint backend of egui: pass 9 of the frame graph.
//!
//! [`crate::Ui::draw`] tessellates the frame on the CPU and hands the result
//! to [`EguiPass`] through a [`Shared`] slot; the renderer then calls
//! [`UiPass::execute`] while it records the frame. The pass uploads the
//! texture changes, copies the geometry into per-frame-slot buffers and draws
//! one indexed draw per clipped mesh.

use std::collections::HashMap;
use std::mem::ManuallyDrop;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use ash::{khr, vk};
use egui::epaint::{ImageDelta, Primitive};
use egui::{ClippedPrimitive, ImageData, TextureFilter, TextureId, TexturesDelta};
use faset_render::{FrameCtx, GpuHandles, ImageHandle, UiPass};
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme, Allocator};
use gpu_allocator::MemoryLocation;

use crate::UiError;

/// Texture sets the descriptor pool can hold.
const MAX_TEXTURES: u32 = 256;
/// Bytes of one egui vertex: position, uv (two `f32` each), colour (4 bytes).
const VERTEX_BYTES: usize = 20;

/// A tessellated frame waiting for the renderer.
pub(crate) struct PaintData {
    pub primitives: Vec<ClippedPrimitive>,
    pub textures: TexturesDelta,
    pub pixels_per_point: f32,
}

impl Drop for PaintData {
    fn drop(&mut self) {
        // egui asserts that texture changes are applied or cleared.
        self.textures.clear();
    }
}

/// Counters the pass publishes after each execution.
#[derive(Default)]
pub(crate) struct PassStats {
    pub draw_calls: AtomicU32,
    pub vertices: AtomicU32,
}

/// What [`crate::Ui`] and [`EguiPass`] share.
#[derive(Default)]
pub(crate) struct Shared {
    pub pending: Mutex<Option<PaintData>>,
    pub stats: PassStats,
}

impl Shared {
    /// Queues a frame. If the previous one was never drawn, its texture
    /// changes are kept so the atlas stays consistent.
    pub(crate) fn submit(&self, mut data: PaintData) {
        let mut slot = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(mut old) = slot.take() {
            let mut merged = std::mem::take(&mut old.textures);
            merged.append(std::mem::take(&mut data.textures));
            data.textures = merged;
        }
        *slot = Some(data);
    }
}

/// A buffer with its memory.
struct Buffer {
    buffer: vk::Buffer,
    allocation: Option<Allocation>,
    size: u64,
}

/// Geometry and staging buffers of one frame in flight.
#[derive(Default)]
struct SlotBuffers {
    vertices: Option<Buffer>,
    indices: Option<Buffer>,
    staging: Option<Buffer>,
}

struct Texture {
    image: vk::Image,
    view: vk::ImageView,
    allocation: Option<Allocation>,
    set: vk::DescriptorSet,
    filter: TextureFilter,
}

/// pass 9: draws the tessellated egui frame onto the display target.
pub struct EguiPass {
    shared: Arc<Shared>,
    device: ash::Device,
    dr: khr::dynamic_rendering::Device,
    allocator: ManuallyDrop<Allocator>,
    set_layout: vk::DescriptorSetLayout,
    layout: vk::PipelineLayout,
    pool: vk::DescriptorPool,
    sampler_linear: vk::Sampler,
    sampler_nearest: vk::Sampler,
    /// Pipeline and the target format it was built for.
    pipeline: Option<(vk::Pipeline, vk::Format)>,
    textures: HashMap<TextureId, Texture>,
    /// Textures freed by egui, destroyed once no frame in flight uses them.
    retired: Vec<(u64, Texture)>,
    slots: Vec<SlotBuffers>,
    warned_callback: bool,
}

impl EguiPass {
    pub(crate) fn new(handles: &GpuHandles, shared: Arc<Shared>) -> Result<Self, UiError> {
        let device = handles.device.clone();
        let allocator = Allocator::new(&gpu_allocator::vulkan::AllocatorCreateDesc {
            instance: handles.instance.clone(),
            device: device.clone(),
            physical_device: handles.physical,
            debug_settings: Default::default(),
            buffer_device_address: false,
            allocation_sizes: Default::default(),
        })?;
        let dr = khr::dynamic_rendering::Device::new(&handles.instance, &device);
        let mut pass = Self {
            shared,
            device,
            dr,
            allocator: ManuallyDrop::new(allocator),
            set_layout: vk::DescriptorSetLayout::null(),
            layout: vk::PipelineLayout::null(),
            pool: vk::DescriptorPool::null(),
            sampler_linear: vk::Sampler::null(),
            sampler_nearest: vk::Sampler::null(),
            pipeline: None,
            textures: HashMap::new(),
            retired: Vec::new(),
            slots: Vec::new(),
            warned_callback: false,
        };
        // On error `Drop` frees whatever was created (null handles are skipped).
        pass.create_objects()?;
        Ok(pass)
    }

    fn create_objects(&mut self) -> Result<(), UiError> {
        let device = &self.device;
        let sampler = |filter| {
            let info = vk::SamplerCreateInfo::default()
                .mag_filter(filter)
                .min_filter(filter)
                .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
                .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE);
            // SAFETY: valid device and create info.
            unsafe { device.create_sampler(&info, None) }
        };
        self.sampler_linear = sampler(vk::Filter::LINEAR)?;
        self.sampler_nearest = sampler(vk::Filter::NEAREST)?;
        let bindings = [
            vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
            vk::DescriptorSetLayoutBinding::default()
                .binding(1)
                .descriptor_type(vk::DescriptorType::SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
        ];
        // SAFETY: valid device and create infos; handles are destroyed in `Drop`.
        unsafe {
            self.set_layout = device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                None,
            )?;
            let layouts = [self.set_layout];
            let push = [vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::VERTEX)
                .offset(0)
                .size(8)];
            self.layout = device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .set_layouts(&layouts)
                    .push_constant_ranges(&push),
                None,
            )?;
            let sizes = [
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::SAMPLED_IMAGE)
                    .descriptor_count(MAX_TEXTURES),
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::SAMPLER)
                    .descriptor_count(MAX_TEXTURES),
            ];
            self.pool = device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .flags(vk::DescriptorPoolCreateFlags::FREE_DESCRIPTOR_SET)
                    .max_sets(MAX_TEXTURES)
                    .pool_sizes(&sizes),
                None,
            )?;
        }
        Ok(())
    }

    fn create_pipeline(&self, format: vk::Format) -> Result<vk::Pipeline, UiError> {
        let device = &self.device;
        let module = |words: &[u32]| {
            // SAFETY: SPIR-V produced and validated by naga.
            unsafe {
                device
                    .create_shader_module(&vk::ShaderModuleCreateInfo::default().code(words), None)
            }
        };
        let vs = module(faset_shaders::EGUI_VS)?;
        let fs = match module(faset_shaders::EGUI_FS) {
            Ok(m) => m,
            Err(e) => {
                // SAFETY: unused module.
                unsafe { device.destroy_shader_module(vs, None) };
                return Err(e.into());
            }
        };
        let stages = [
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::VERTEX)
                .module(vs)
                .name(c"vs_main"),
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .module(fs)
                .name(c"fs_main"),
        ];
        let bindings = [vk::VertexInputBindingDescription::default()
            .binding(0)
            .stride(VERTEX_BYTES as u32)
            .input_rate(vk::VertexInputRate::VERTEX)];
        let attributes = [
            vk::VertexInputAttributeDescription::default()
                .location(0)
                .format(vk::Format::R32G32_SFLOAT)
                .offset(0),
            vk::VertexInputAttributeDescription::default()
                .location(1)
                .format(vk::Format::R32G32_SFLOAT)
                .offset(8),
            vk::VertexInputAttributeDescription::default()
                .location(2)
                .format(vk::Format::R8G8B8A8_UNORM)
                .offset(16),
        ];
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(&bindings)
            .vertex_attribute_descriptions(&attributes);
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
        // Premultiplied alpha.
        let blend_attachments = [vk::PipelineColorBlendAttachmentState::default()
            .blend_enable(true)
            .src_color_blend_factor(vk::BlendFactor::ONE)
            .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .color_blend_op(vk::BlendOp::ADD)
            .src_alpha_blend_factor(vk::BlendFactor::ONE)
            .dst_alpha_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .alpha_blend_op(vk::BlendOp::ADD)
            .color_write_mask(vk::ColorComponentFlags::RGBA)];
        let blend =
            vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachments);
        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
        let formats = [format];
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
            device.destroy_shader_module(vs, None);
            device.destroy_shader_module(fs, None);
        }
        Ok(created.map_err(|(_, e)| e)?[0])
    }

    /// A buffer of at least `size` bytes in `location`.
    fn create_buffer(
        &mut self,
        name: &str,
        size: u64,
        usage: vk::BufferUsageFlags,
        location: MemoryLocation,
    ) -> Result<Buffer, UiError> {
        // SAFETY: valid device and create info.
        let buffer = unsafe {
            self.device.create_buffer(
                &vk::BufferCreateInfo::default().size(size).usage(usage),
                None,
            )?
        };
        // SAFETY: live buffer.
        let requirements = unsafe { self.device.get_buffer_memory_requirements(buffer) };
        let allocation = self.allocator.allocate(&AllocationCreateDesc {
            name,
            requirements,
            location,
            linear: true,
            allocation_scheme: AllocationScheme::GpuAllocatorManaged,
        });
        let allocation = match allocation {
            Ok(a) => a,
            Err(e) => {
                // SAFETY: unused buffer.
                unsafe { self.device.destroy_buffer(buffer, None) };
                return Err(e.into());
            }
        };
        // SAFETY: memory satisfies the requirements; the buffer is unbound.
        let bound = unsafe {
            self.device
                .bind_buffer_memory(buffer, allocation.memory(), allocation.offset())
        };
        let mut out = Buffer {
            buffer,
            allocation: Some(allocation),
            size,
        };
        if let Err(e) = bound {
            self.destroy_buffer(&mut out);
            return Err(e.into());
        }
        Ok(out)
    }

    fn destroy_buffer(&mut self, buffer: &mut Buffer) {
        if let Some(a) = buffer.allocation.take() {
            if let Err(e) = self.allocator.free(a) {
                tracing::warn!("failed to free a UI buffer: {e}");
            }
        }
        // SAFETY: the caller guarantees the GPU is done with the buffer.
        unsafe { self.device.destroy_buffer(buffer.buffer, None) };
        buffer.buffer = vk::Buffer::null();
    }

    /// Makes `slot`'s buffer at least `needed` bytes (grown by half, at least
    /// 64 KiB). The slot's previous frame is finished, so the old one is freed
    /// right away.
    fn ensure_buffer(
        &mut self,
        slot: usize,
        which: fn(&mut SlotBuffers) -> &mut Option<Buffer>,
        name: &str,
        needed: u64,
        usage: vk::BufferUsageFlags,
    ) -> Result<(), UiError> {
        let current = which(&mut self.slots[slot]).as_ref().map_or(0, |b| b.size);
        if current >= needed {
            return Ok(());
        }
        let size = needed.max(current + current / 2).max(64 * 1024);
        let fresh = self.create_buffer(name, size, usage, MemoryLocation::CpuToGpu)?;
        if let Some(mut old) = which(&mut self.slots[slot]).replace(fresh) {
            self.destroy_buffer(&mut old);
        }
        Ok(())
    }

    fn destroy_texture(&mut self, mut texture: Texture) {
        // SAFETY: the texture is no longer used by any frame in flight.
        unsafe {
            let _ = self.device.free_descriptor_sets(self.pool, &[texture.set]);
            self.device.destroy_image_view(texture.view, None);
            self.device.destroy_image(texture.image, None);
        }
        if let Some(a) = texture.allocation.take() {
            if let Err(e) = self.allocator.free(a) {
                tracing::warn!("failed to free a UI texture: {e}");
            }
        }
    }

    fn create_texture(
        &mut self,
        extent: [u32; 2],
        filter: TextureFilter,
    ) -> Result<Texture, UiError> {
        let info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(vk::Format::R8G8B8A8_UNORM)
            .extent(vk::Extent3D {
                width: extent[0],
                height: extent[1],
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        // SAFETY: valid device and create info.
        let image = unsafe { self.device.create_image(&info, None)? };
        let mut texture = Texture {
            image,
            view: vk::ImageView::null(),
            allocation: None,
            set: vk::DescriptorSet::null(),
            filter,
        };
        let result = (|| -> Result<(), UiError> {
            // SAFETY: live image.
            let requirements = unsafe { self.device.get_image_memory_requirements(image) };
            let allocation = self.allocator.allocate(&AllocationCreateDesc {
                name: "egui-texture",
                requirements,
                location: MemoryLocation::GpuOnly,
                linear: false,
                allocation_scheme: AllocationScheme::GpuAllocatorManaged,
            })?;
            // SAFETY: memory satisfies the requirements; the image is unbound.
            let bound = unsafe {
                self.device
                    .bind_image_memory(image, allocation.memory(), allocation.offset())
            };
            texture.allocation = Some(allocation);
            bound?;
            let view_info = vk::ImageViewCreateInfo::default()
                .image(image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(vk::Format::R8G8B8A8_UNORM)
                .subresource_range(COLOR_RANGE);
            // SAFETY: live image of a compatible format.
            texture.view = unsafe { self.device.create_image_view(&view_info, None)? };
            let layouts = [self.set_layout];
            // SAFETY: the pool has room for `MAX_TEXTURES` sets.
            texture.set = unsafe {
                self.device.allocate_descriptor_sets(
                    &vk::DescriptorSetAllocateInfo::default()
                        .descriptor_pool(self.pool)
                        .set_layouts(&layouts),
                )?[0]
            };
            let images = [vk::DescriptorImageInfo::default()
                .image_view(texture.view)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
            let sampler = if filter == TextureFilter::Nearest {
                self.sampler_nearest
            } else {
                self.sampler_linear
            };
            let samplers = [vk::DescriptorImageInfo::default().sampler(sampler)];
            let writes = [
                vk::WriteDescriptorSet::default()
                    .dst_set(texture.set)
                    .dst_binding(0)
                    .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                    .image_info(&images),
                vk::WriteDescriptorSet::default()
                    .dst_set(texture.set)
                    .dst_binding(1)
                    .descriptor_type(vk::DescriptorType::SAMPLER)
                    .image_info(&samplers),
            ];
            // SAFETY: the set is not in use yet.
            unsafe { self.device.update_descriptor_sets(&writes, &[]) };
            Ok(())
        })();
        match result {
            Ok(()) => Ok(texture),
            Err(e) => {
                if texture.set == vk::DescriptorSet::null() {
                    // `free_descriptor_sets` on a null set is invalid.
                    // SAFETY: never used.
                    unsafe {
                        self.device.destroy_image_view(texture.view, None);
                        self.device.destroy_image(texture.image, None);
                    }
                    if let Some(a) = texture.allocation.take() {
                        let _ = self.allocator.free(a);
                    }
                } else {
                    self.destroy_texture(texture);
                }
                Err(e)
            }
        }
    }

    /// Records the uploads of `delta` (new textures, patches) into `cmd`.
    fn upload_textures(
        &mut self,
        ctx: &FrameCtx<'_>,
        delta: &TexturesDelta,
    ) -> Result<(), UiError> {
        if delta.set.is_empty() {
            return Ok(());
        }
        // Bytes of every patch, in staging order.
        let mut patches: Vec<(TextureId, &ImageDelta, usize)> = Vec::new();
        let mut total = 0usize;
        for (id, deltas) in &delta.set {
            for d in deltas {
                let ImageData::Color(image) = &d.image;
                patches.push((*id, d, total));
                total += image.pixels.len() * 4;
            }
        }
        self.ensure_buffer(
            ctx.slot,
            |s| &mut s.staging,
            "egui-staging",
            total as u64,
            vk::BufferUsageFlags::TRANSFER_SRC,
        )?;
        let staging = self.slots[ctx.slot].staging.as_ref().expect("ensured");
        let (staging_buffer, ptr) = (
            staging.buffer,
            staging
                .allocation
                .as_ref()
                .and_then(Allocation::mapped_ptr)
                .expect("host-visible staging memory is mapped")
                .as_ptr()
                .cast::<u8>(),
        );
        for (_, d, offset) in &patches {
            let ImageData::Color(image) = &d.image;
            for (i, px) in image.pixels.iter().enumerate() {
                // SAFETY: `offset + 4 * i + 4 <= total <= buffer size`.
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        px.to_array().as_ptr(),
                        ptr.add(offset + 4 * i),
                        4,
                    );
                }
            }
        }
        for (id, d, offset) in patches {
            let ImageData::Color(image) = &d.image;
            let size = [image.size[0] as u32, image.size[1] as u32];
            let filter = d.options.magnification;
            let origin = match d.pos {
                Some([x, y]) => {
                    let Some(t) = self.textures.get(&id) else {
                        tracing::warn!("egui patched a texture it never created: {id:?}");
                        continue;
                    };
                    if t.filter != filter {
                        // Sampler changes are rare; the patch keeps the old one.
                        tracing::debug!("egui texture {id:?} changed its filter");
                    }
                    [x as i32, y as i32]
                }
                None => {
                    let texture = self.create_texture(size, filter)?;
                    if let Some(old) = self.textures.insert(id, texture) {
                        self.retired.push((ctx.frame_index, old));
                    }
                    [0, 0]
                }
            };
            let texture = &self.textures[&id];
            let old = if d.pos.is_some() {
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL
            } else {
                vk::ImageLayout::UNDEFINED
            };
            // SAFETY: `cmd` is recording outside a rendering; the image and
            // the staging buffer are live and sized for the copy.
            unsafe {
                transition(
                    ctx.device,
                    ctx.cmd,
                    texture.image,
                    old,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    (
                        vk::PipelineStageFlags::FRAGMENT_SHADER,
                        vk::AccessFlags::SHADER_READ,
                    ),
                    (
                        vk::PipelineStageFlags::TRANSFER,
                        vk::AccessFlags::TRANSFER_WRITE,
                    ),
                );
                let region = vk::BufferImageCopy::default()
                    .buffer_offset(offset as u64)
                    .image_subresource(vk::ImageSubresourceLayers {
                        aspect_mask: vk::ImageAspectFlags::COLOR,
                        mip_level: 0,
                        base_array_layer: 0,
                        layer_count: 1,
                    })
                    .image_offset(vk::Offset3D {
                        x: origin[0],
                        y: origin[1],
                        z: 0,
                    })
                    .image_extent(vk::Extent3D {
                        width: size[0],
                        height: size[1],
                        depth: 1,
                    });
                ctx.device.cmd_copy_buffer_to_image(
                    ctx.cmd,
                    staging_buffer,
                    texture.image,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    &[region],
                );
                transition(
                    ctx.device,
                    ctx.cmd,
                    texture.image,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                    (
                        vk::PipelineStageFlags::TRANSFER,
                        vk::AccessFlags::TRANSFER_WRITE,
                    ),
                    (
                        vk::PipelineStageFlags::FRAGMENT_SHADER,
                        vk::AccessFlags::SHADER_READ,
                    ),
                );
            }
        }
        Ok(())
    }

    /// Copies the geometry into the slot's buffers; returns the number of
    /// vertices and indices.
    fn upload_geometry(
        &mut self,
        slot: usize,
        primitives: &[ClippedPrimitive],
    ) -> Result<(usize, usize), UiError> {
        let (mut vertices, mut indices) = (0usize, 0usize);
        for p in primitives {
            if let Primitive::Mesh(m) = &p.primitive {
                vertices += m.vertices.len();
                indices += m.indices.len();
            }
        }
        if vertices == 0 || indices == 0 {
            return Ok((0, 0));
        }
        self.ensure_buffer(
            slot,
            |s| &mut s.vertices,
            "egui-vertices",
            (vertices * VERTEX_BYTES) as u64,
            vk::BufferUsageFlags::VERTEX_BUFFER,
        )?;
        self.ensure_buffer(
            slot,
            |s| &mut s.indices,
            "egui-indices",
            (indices * 4) as u64,
            vk::BufferUsageFlags::INDEX_BUFFER,
        )?;
        let mapped = |b: &Option<Buffer>| {
            b.as_ref()
                .and_then(|b| b.allocation.as_ref())
                .and_then(Allocation::mapped_ptr)
                .expect("host-visible buffer memory is mapped")
                .as_ptr()
                .cast::<u8>()
        };
        let vertex_ptr = mapped(&self.slots[slot].vertices);
        let index_ptr = mapped(&self.slots[slot].indices);
        let (mut v, mut i) = (0usize, 0usize);
        for p in primitives {
            let Primitive::Mesh(m) = &p.primitive else {
                continue;
            };
            for vertex in &m.vertices {
                let mut bytes = [0u8; VERTEX_BYTES];
                bytes[0..4].copy_from_slice(&vertex.pos.x.to_ne_bytes());
                bytes[4..8].copy_from_slice(&vertex.pos.y.to_ne_bytes());
                bytes[8..12].copy_from_slice(&vertex.uv.x.to_ne_bytes());
                bytes[12..16].copy_from_slice(&vertex.uv.y.to_ne_bytes());
                bytes[16..20].copy_from_slice(&vertex.color.to_array());
                // SAFETY: `v < vertices` and the buffer holds `vertices`
                // vertices.
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        bytes.as_ptr(),
                        vertex_ptr.add(v * VERTEX_BYTES),
                        VERTEX_BYTES,
                    );
                }
                v += 1;
            }
            for index in &m.indices {
                // SAFETY: `i < indices` and the buffer holds `indices` u32.
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        index.to_ne_bytes().as_ptr(),
                        index_ptr.add(i * 4),
                        4,
                    );
                }
                i += 1;
            }
        }
        Ok((vertices, indices))
    }

    fn record(
        &mut self,
        ctx: &mut FrameCtx<'_>,
        target: &ImageHandle,
        data: PaintData,
    ) -> Result<(), UiError> {
        if self.slots.len() < ctx.frames_in_flight {
            self.slots
                .resize_with(ctx.frames_in_flight, SlotBuffers::default);
        }
        // Free what no frame in flight can still read.
        let frames = ctx.frames_in_flight as u64;
        let (done, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut self.retired)
            .into_iter()
            .partition(|(at, _)| at + frames <= ctx.frame_index);
        self.retired = keep;
        for (_, texture) in done {
            self.destroy_texture(texture);
        }

        self.upload_textures(ctx, &data.textures)?;
        let (vertices, _) = self.upload_geometry(ctx.slot, &data.primitives)?;

        let stats = &self.shared.stats;
        stats.vertices.store(vertices as u32, Ordering::Relaxed);
        let mut draws = 0u32;
        if vertices > 0 {
            if self.pipeline.is_none_or(|(_, f)| f != target.format) {
                if let Some((old, _)) = self.pipeline.take() {
                    // SAFETY: the renderer waits for idle before the format
                    // of the target can change.
                    unsafe { self.device.destroy_pipeline(old, None) };
                }
                self.pipeline = Some((self.create_pipeline(target.format)?, target.format));
            }
            let (pipeline, _) = self.pipeline.expect("created above");
            let device = ctx.device;
            let slot = &self.slots[ctx.slot];
            let vertex_buffer = slot.vertices.as_ref().expect("uploaded").buffer;
            let index_buffer = slot.indices.as_ref().expect("uploaded").buffer;
            let ppp = data.pixels_per_point;
            let extent = target.extent;
            let colour = [vk::RenderingAttachmentInfo::default()
                .image_view(target.view)
                .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                .load_op(vk::AttachmentLoadOp::LOAD)
                .store_op(vk::AttachmentStoreOp::STORE)];
            let rendering = vk::RenderingInfo::default()
                .render_area(vk::Rect2D {
                    offset: vk::Offset2D::default(),
                    extent,
                })
                .layer_count(1)
                .color_attachments(&colour);
            // SAFETY: `cmd` is recording outside a rendering; the pipeline,
            // buffers and descriptor sets are live and outlive the frame.
            unsafe {
                self.dr.cmd_begin_rendering(ctx.cmd, &rendering);
                device.cmd_bind_pipeline(ctx.cmd, vk::PipelineBindPoint::GRAPHICS, pipeline);
                device.cmd_set_viewport(
                    ctx.cmd,
                    0,
                    &[vk::Viewport {
                        x: 0.0,
                        y: 0.0,
                        width: extent.width as f32,
                        height: extent.height as f32,
                        min_depth: 0.0,
                        max_depth: 1.0,
                    }],
                );
                let screen = [extent.width as f32 / ppp, extent.height as f32 / ppp];
                let mut bytes = [0u8; 8];
                bytes[0..4].copy_from_slice(&screen[0].to_ne_bytes());
                bytes[4..8].copy_from_slice(&screen[1].to_ne_bytes());
                device.cmd_push_constants(
                    ctx.cmd,
                    self.layout,
                    vk::ShaderStageFlags::VERTEX,
                    0,
                    &bytes,
                );
                device.cmd_bind_vertex_buffers(ctx.cmd, 0, &[vertex_buffer], &[0]);
                device.cmd_bind_index_buffer(ctx.cmd, index_buffer, 0, vk::IndexType::UINT32);
                let (mut v, mut i) = (0i32, 0u32);
                for p in &data.primitives {
                    let Primitive::Mesh(m) = &p.primitive else {
                        if !self.warned_callback {
                            self.warned_callback = true;
                            tracing::warn!("egui paint callbacks are not supported");
                        }
                        continue;
                    };
                    let count = m.indices.len() as u32;
                    let first = (i, v);
                    v += m.vertices.len() as i32;
                    i += count;
                    let Some(texture) = self.textures.get(&m.texture_id) else {
                        continue;
                    };
                    let scissor = scissor(p.clip_rect, ppp, extent);
                    if scissor.extent.width == 0 || scissor.extent.height == 0 || count == 0 {
                        continue;
                    }
                    device.cmd_set_scissor(ctx.cmd, 0, &[scissor]);
                    device.cmd_bind_descriptor_sets(
                        ctx.cmd,
                        vk::PipelineBindPoint::GRAPHICS,
                        self.layout,
                        0,
                        &[texture.set],
                        &[],
                    );
                    device.cmd_draw_indexed(ctx.cmd, count, 1, first.0, first.1, 0);
                    draws += 1;
                }
                self.dr.cmd_end_rendering(ctx.cmd);
            }
        }
        stats.draw_calls.store(draws, Ordering::Relaxed);

        for id in &data.textures.free {
            if let Some(texture) = self.textures.remove(id) {
                self.retired.push((ctx.frame_index, texture));
            }
        }
        Ok(())
    }
}

impl UiPass for EguiPass {
    fn execute(&mut self, ctx: &mut FrameCtx<'_>, target: &ImageHandle) {
        let data = self
            .shared
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        let Some(data) = data else {
            self.shared.stats.draw_calls.store(0, Ordering::Relaxed);
            return;
        };
        if let Err(e) = self.record(ctx, target, data) {
            tracing::error!("UI pass failed: {e}");
        }
    }
}

impl Drop for EguiPass {
    fn drop(&mut self) {
        // SAFETY: the renderer waited for the device to be idle before
        // dropping its passes.
        unsafe {
            let _ = self.device.device_wait_idle();
        }
        for (_, texture) in std::mem::take(&mut self.retired) {
            self.destroy_texture(texture);
        }
        let textures: Vec<Texture> = self.textures.drain().map(|(_, t)| t).collect();
        for texture in textures {
            self.destroy_texture(texture);
        }
        for mut slot in std::mem::take(&mut self.slots) {
            for mut buffer in [
                slot.vertices.take(),
                slot.indices.take(),
                slot.staging.take(),
            ]
            .into_iter()
            .flatten()
            {
                self.destroy_buffer(&mut buffer);
            }
        }
        // SAFETY: nothing uses these objects any more; null handles are no-ops.
        unsafe {
            if let Some((pipeline, _)) = self.pipeline.take() {
                self.device.destroy_pipeline(pipeline, None);
            }
            self.device.destroy_descriptor_pool(self.pool, None);
            self.device.destroy_pipeline_layout(self.layout, None);
            self.device
                .destroy_descriptor_set_layout(self.set_layout, None);
            self.device.destroy_sampler(self.sampler_linear, None);
            self.device.destroy_sampler(self.sampler_nearest, None);
            // The allocator frees its memory blocks while the device lives.
            ManuallyDrop::drop(&mut self.allocator);
        }
    }
}

const COLOR_RANGE: vk::ImageSubresourceRange = vk::ImageSubresourceRange {
    aspect_mask: vk::ImageAspectFlags::COLOR,
    base_mip_level: 0,
    level_count: 1,
    base_array_layer: 0,
    layer_count: 1,
};

/// One image layout transition.
///
/// # Safety
/// `cmd` must be recording outside a rendering and `image` must be live.
unsafe fn transition(
    device: &ash::Device,
    cmd: vk::CommandBuffer,
    image: vk::Image,
    old: vk::ImageLayout,
    new: vk::ImageLayout,
    src: (vk::PipelineStageFlags, vk::AccessFlags),
    dst: (vk::PipelineStageFlags, vk::AccessFlags),
) {
    let barrier = vk::ImageMemoryBarrier::default()
        .old_layout(old)
        .new_layout(new)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(image)
        .subresource_range(COLOR_RANGE)
        .src_access_mask(if old == vk::ImageLayout::UNDEFINED {
            vk::AccessFlags::empty()
        } else {
            src.1
        })
        .dst_access_mask(dst.1);
    // SAFETY: forwarded from the caller.
    unsafe {
        device.cmd_pipeline_barrier(
            cmd,
            if old == vk::ImageLayout::UNDEFINED {
                vk::PipelineStageFlags::TOP_OF_PIPE
            } else {
                src.0
            },
            dst.0,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[barrier],
        );
    }
}

/// The scissor rectangle of a clip rectangle in points, clamped to the target.
fn scissor(clip: egui::Rect, ppp: f32, extent: vk::Extent2D) -> vk::Rect2D {
    let x0 = (clip.min.x * ppp).round().clamp(0.0, extent.width as f32) as u32;
    let y0 = (clip.min.y * ppp).round().clamp(0.0, extent.height as f32) as u32;
    let x1 = (clip.max.x * ppp)
        .round()
        .clamp(x0 as f32, extent.width as f32) as u32;
    let y1 = (clip.max.y * ppp)
        .round()
        .clamp(y0 as f32, extent.height as f32) as u32;
    vk::Rect2D {
        offset: vk::Offset2D {
            x: x0 as i32,
            y: y0 as i32,
        },
        extent: vk::Extent2D {
            width: x1 - x0,
            height: y1 - y0,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scissor_is_clamped_to_the_target() {
        let extent = vk::Extent2D {
            width: 200,
            height: 100,
        };
        let clip = egui::Rect::from_min_max(egui::pos2(-5.0, 10.0), egui::pos2(500.0, 20.0));
        let s = scissor(clip, 1.0, extent);
        assert_eq!((s.offset.x, s.offset.y), (0, 10));
        assert_eq!((s.extent.width, s.extent.height), (200, 10));
        let s = scissor(clip, 2.0, extent);
        assert_eq!((s.offset.y, s.extent.height), (20, 20));
    }

    #[test]
    fn submit_keeps_the_texture_changes_of_an_undrawn_frame() {
        let shared = Shared::default();
        let mut first = TexturesDelta::default();
        first.free.insert(TextureId::Managed(7));
        shared.submit(PaintData {
            primitives: Vec::new(),
            textures: first,
            pixels_per_point: 1.0,
        });
        shared.submit(PaintData {
            primitives: Vec::new(),
            textures: TexturesDelta::default(),
            pixels_per_point: 1.0,
        });
        let kept = shared.pending.lock().unwrap().take().unwrap();
        assert!(kept.textures.free.contains(&TextureId::Managed(7)));
    }
}

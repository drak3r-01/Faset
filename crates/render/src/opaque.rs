//! Opaque pass resources: the G-buffer targets, the pipeline and the static
//! mesh arena.

use ash::vk;
use gpu_allocator::MemoryLocation;

use crate::context::Context;
use crate::cull::GpuInstance;
use crate::frame::OwnedImage;
use crate::gpu::{create_image, GpuBuffer};
use crate::material::{MaterialTable, MATERIAL_TABLE_SIZE};
use crate::mesh::{Aabb, ArenaLayout, MeshId, MeshRange, Vertex};
use crate::RenderError;

pub(crate) const HDR_FORMAT: vk::Format = vk::Format::R16G16B16A16_SFLOAT;
pub(crate) const DEPTH_FORMAT: vk::Format = vk::Format::D32_SFLOAT;
pub(crate) const MOTION_FORMAT: vk::Format = vk::Format::R16G16_SFLOAT;
pub(crate) const NORMAL_FORMAT: vk::Format = vk::Format::R8G8_UNORM;
/// Reactive mask: opacity of the weather particles, for temporal upscalers.
pub(crate) const REACTIVE_FORMAT: vk::Format = vk::Format::R8_UNORM;

/// Vertices the arena holds (20 MiB).
pub(crate) const ARENA_VERTICES: u32 = 1 << 20;
/// Indices the arena holds (16 MiB).
pub(crate) const ARENA_INDICES: u32 = 4 << 20;

/// Opaque pass outputs, sized like the render target.
pub(crate) struct GBuffer {
    pub extent: vk::Extent2D,
    pub hdr: OwnedImage,
    pub depth: OwnedImage,
    pub motion: OwnedImage,
    pub normal: OwnedImage,
    /// Particle opacity; only meaningful while an upscaler is active.
    pub reactive: OwnedImage,
}

impl GBuffer {
    pub(crate) fn new(ctx: &mut Context, extent: vk::Extent2D) -> Result<Self, RenderError> {
        let color = vk::ImageAspectFlags::COLOR;
        let attachment = vk::ImageUsageFlags::COLOR_ATTACHMENT;
        let sampled = vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_SRC;
        let mut made: Vec<OwnedImage> = Vec::with_capacity(5);
        let specs = [
            ("gbuffer-hdr", HDR_FORMAT, attachment | sampled, color),
            (
                "gbuffer-depth",
                DEPTH_FORMAT,
                vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT | vk::ImageUsageFlags::SAMPLED,
                vk::ImageAspectFlags::DEPTH,
            ),
            ("gbuffer-motion", MOTION_FORMAT, attachment | sampled, color),
            ("gbuffer-normal", NORMAL_FORMAT, attachment | sampled, color),
            (
                "gbuffer-reactive",
                REACTIVE_FORMAT,
                attachment | sampled,
                color,
            ),
        ];
        for (name, format, usage, aspect) in specs {
            match create_image(ctx, name, extent, format, usage, aspect) {
                Ok(img) => made.push(img),
                Err(e) => {
                    for mut img in made {
                        img.destroy(ctx);
                    }
                    return Err(e);
                }
            }
        }
        let mut it = made.into_iter();
        Ok(Self {
            extent,
            hdr: it.next().expect("five images"),
            depth: it.next().expect("five images"),
            motion: it.next().expect("five images"),
            normal: it.next().expect("five images"),
            reactive: it.next().expect("five images"),
        })
    }

    pub(crate) fn into_images(self) -> [OwnedImage; 5] {
        [
            self.hdr,
            self.depth,
            self.motion,
            self.normal,
            self.reactive,
        ]
    }

    pub(crate) fn destroy(self, ctx: &mut Context) {
        for mut img in self.into_images() {
            img.destroy(ctx);
        }
    }
}

/// The opaque graphics pipeline (dynamic rendering) and the material table
/// it reads (descriptor set 0, binding 0, uniform buffer). Set 1 is the
/// lighting block ([`crate::lighting::LightingBindings`]).
pub(crate) struct OpaquePipeline {
    pub set_layout: vk::DescriptorSetLayout,
    pub layout: vk::PipelineLayout,
    pub pipeline: vk::Pipeline,
    pub materials: MaterialBuffer,
}

/// Bytes of the push constant block (`jitter: vec2<f32>, time: f32, pad`).
pub(crate) const PUSH_CONSTANT_SIZE: u32 = 16;
/// Stages reading the push constant block.
pub(crate) const PUSH_CONSTANT_STAGES: vk::ShaderStageFlags = vk::ShaderStageFlags::from_raw(
    vk::ShaderStageFlags::VERTEX.as_raw() | vk::ShaderStageFlags::FRAGMENT.as_raw(),
);

impl OpaquePipeline {
    /// Builds the pipeline from the embedded SPIR-V and uploads `table`.
    /// `lighting_layout` is descriptor set 1 (the per-frame lighting block).
    pub(crate) fn new(
        ctx: &mut Context,
        table: &MaterialTable,
        lighting_layout: vk::DescriptorSetLayout,
    ) -> Result<Self, RenderError> {
        let device = &ctx.device;
        let bindings = [vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::FRAGMENT)];
        // SAFETY: valid device and create info.
        let set_layout = unsafe {
            device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                None,
            )
        }?;
        let set_layouts = [set_layout, lighting_layout];
        let ranges = [vk::PushConstantRange::default()
            .stage_flags(PUSH_CONSTANT_STAGES)
            .offset(0)
            .size(PUSH_CONSTANT_SIZE)];
        // SAFETY: valid device and create info.
        let layout = match unsafe {
            device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .set_layouts(&set_layouts)
                    .push_constant_ranges(&ranges),
                None,
            )
        } {
            Ok(l) => l,
            Err(e) => {
                // SAFETY: unused set layout.
                unsafe { device.destroy_descriptor_set_layout(set_layout, None) };
                return Err(e.into());
            }
        };
        let pipeline = Self::create_pipeline(
            device,
            layout,
            faset_shaders::OPAQUE_VS,
            faset_shaders::OPAQUE_FS,
        );
        let pipeline = match pipeline {
            Ok(p) => p,
            Err(e) => {
                // SAFETY: unused layouts.
                unsafe {
                    device.destroy_pipeline_layout(layout, None);
                    device.destroy_descriptor_set_layout(set_layout, None);
                }
                return Err(e);
            }
        };
        match MaterialBuffer::new(ctx, set_layout, table) {
            Ok(materials) => Ok(Self {
                set_layout,
                layout,
                pipeline,
                materials,
            }),
            Err(e) => {
                // SAFETY: unused pipeline and layouts.
                unsafe {
                    ctx.device.destroy_pipeline(pipeline, None);
                    ctx.device.destroy_pipeline_layout(layout, None);
                    ctx.device.destroy_descriptor_set_layout(set_layout, None);
                }
                Err(e)
            }
        }
    }

    /// Swaps in a pipeline built from new SPIR-V. On error the current
    /// pipeline stays. The GPU must be done with the current pipeline.
    #[cfg(feature = "hot-reload")]
    pub(crate) fn replace_shaders(
        &mut self,
        device: &ash::Device,
        vs: &[u32],
        fs: &[u32],
    ) -> Result<(), RenderError> {
        let new = Self::create_pipeline(device, self.layout, vs, fs)?;
        let old = std::mem::replace(&mut self.pipeline, new);
        // SAFETY: the caller waited for the device to be idle.
        unsafe { device.destroy_pipeline(old, None) };
        Ok(())
    }

    fn create_pipeline(
        device: &ash::Device,
        layout: vk::PipelineLayout,
        vs: &[u32],
        fs: &[u32],
    ) -> Result<vk::Pipeline, RenderError> {
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
        let pipeline = Self::create_pipeline_with(device, layout, vs_module, fs_module);
        // SAFETY: modules may be destroyed once pipelines are created.
        unsafe {
            device.destroy_shader_module(vs_module, None);
            device.destroy_shader_module(fs_module, None);
        }
        pipeline
    }

    fn create_pipeline_with(
        device: &ash::Device,
        layout: vk::PipelineLayout,
        vs_module: vk::ShaderModule,
        fs_module: vk::ShaderModule,
    ) -> Result<vk::Pipeline, RenderError> {
        let stages = [
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::VERTEX)
                .module(vs_module)
                .name(c"vs_main"),
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .module(fs_module)
                .name(c"fs_main"),
        ];
        let bindings = [
            vk::VertexInputBindingDescription {
                binding: 0,
                stride: std::mem::size_of::<Vertex>() as u32,
                input_rate: vk::VertexInputRate::VERTEX,
            },
            vk::VertexInputBindingDescription {
                binding: 1,
                stride: std::mem::size_of::<GpuInstance>() as u32,
                input_rate: vk::VertexInputRate::INSTANCE,
            },
        ];
        let mut attributes = vec![
            vk::VertexInputAttributeDescription {
                location: 0,
                binding: 0,
                format: vk::Format::R32G32B32_SFLOAT,
                offset: 0,
            },
            vk::VertexInputAttributeDescription {
                location: 1,
                binding: 0,
                format: vk::Format::R8G8B8A8_SNORM,
                offset: 12,
            },
            vk::VertexInputAttributeDescription {
                location: 2,
                binding: 0,
                format: vk::Format::R8G8B8A8_UNORM,
                offset: 16,
            },
        ];
        // 4 + 4 + 3 vec4 columns of `GpuInstance`.
        for column in 0..11u32 {
            attributes.push(vk::VertexInputAttributeDescription {
                location: 3 + column,
                binding: 1,
                format: vk::Format::R32G32B32A32_SFLOAT,
                offset: column * 16,
            });
        }
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(&bindings)
            .vertex_attribute_descriptions(&attributes);
        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let viewport = vk::PipelineViewportStateCreateInfo::default()
            .viewport_count(1)
            .scissor_count(1);
        // Counter-clockwise front faces: the projection's y flip keeps the
        // on-screen winding of counter-clockwise meshes.
        let raster = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::BACK)
            .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let depth = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(true)
            .depth_write_enable(true)
            .depth_compare_op(vk::CompareOp::GREATER_OR_EQUAL);
        let blend_attachments = [vk::PipelineColorBlendAttachmentState::default()
            .color_write_mask(vk::ColorComponentFlags::RGBA); 3];
        let blend =
            vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachments);
        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
        let color_formats = [HDR_FORMAT, MOTION_FORMAT, NORMAL_FORMAT];
        let mut rendering = vk::PipelineRenderingCreateInfo::default()
            .color_attachment_formats(&color_formats)
            .depth_attachment_format(DEPTH_FORMAT);
        let info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&input_assembly)
            .viewport_state(&viewport)
            .rasterization_state(&raster)
            .multisample_state(&multisample)
            .depth_stencil_state(&depth)
            .color_blend_state(&blend)
            .dynamic_state(&dynamic)
            .layout(layout)
            .push_next(&mut rendering);
        // SAFETY: every pointer in `info` outlives the call.
        let pipelines =
            unsafe { device.create_graphics_pipelines(vk::PipelineCache::null(), &[info], None) }
                .map_err(|(_, e)| e)?;
        Ok(pipelines[0])
    }

    pub(crate) fn destroy(&mut self, ctx: &mut Context) {
        self.materials.destroy(ctx);
        // SAFETY: the device is idle.
        unsafe {
            ctx.device.destroy_pipeline(self.pipeline, None);
            ctx.device.destroy_pipeline_layout(self.layout, None);
            ctx.device
                .destroy_descriptor_set_layout(self.set_layout, None);
        }
    }
}

/// The material table on the GPU: a host-visible uniform buffer and the
/// descriptor set that binds it.
pub(crate) struct MaterialBuffer {
    buffer: GpuBuffer,
    pool: vk::DescriptorPool,
    pub set: vk::DescriptorSet,
}

impl MaterialBuffer {
    fn new(
        ctx: &mut Context,
        set_layout: vk::DescriptorSetLayout,
        table: &MaterialTable,
    ) -> Result<Self, RenderError> {
        let mut buffer = GpuBuffer::new(
            ctx,
            "material-table",
            MATERIAL_TABLE_SIZE,
            vk::BufferUsageFlags::UNIFORM_BUFFER,
            MemoryLocation::CpuToGpu,
        )?;
        let sizes = [vk::DescriptorPoolSize {
            ty: vk::DescriptorType::UNIFORM_BUFFER,
            descriptor_count: 1,
        }];
        // SAFETY: valid device and create info.
        let pool = match unsafe {
            ctx.device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(1)
                    .pool_sizes(&sizes),
                None,
            )
        } {
            Ok(p) => p,
            Err(e) => {
                buffer.destroy(ctx);
                return Err(e.into());
            }
        };
        let layouts = [set_layout];
        // SAFETY: the pool has room for this one set.
        let set = match unsafe {
            ctx.device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(pool)
                    .set_layouts(&layouts),
            )
        } {
            Ok(sets) => sets[0],
            Err(e) => {
                // SAFETY: unused pool.
                unsafe { ctx.device.destroy_descriptor_pool(pool, None) };
                buffer.destroy(ctx);
                return Err(e.into());
            }
        };
        let infos = [vk::DescriptorBufferInfo {
            buffer: buffer.buffer,
            offset: 0,
            range: MATERIAL_TABLE_SIZE,
        }];
        let write = vk::WriteDescriptorSet::default()
            .dst_set(set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
            .buffer_info(&infos);
        // SAFETY: live set and buffer; the set is not in use yet.
        unsafe { ctx.device.update_descriptor_sets(&[write], &[]) };
        let mut out = Self { buffer, pool, set };
        out.write(table);
        Ok(out)
    }

    /// Copies `table` into the buffer. The GPU must not be reading it.
    pub(crate) fn write(&mut self, table: &MaterialTable) {
        let gpu = table.to_gpu();
        let bytes: &[u8] = bytemuck::cast_slice(&gpu);
        self.buffer.mapped()[..bytes.len()].copy_from_slice(bytes);
    }

    fn destroy(&mut self, ctx: &mut Context) {
        // SAFETY: the device is idle; destroying the pool frees the set.
        unsafe { ctx.device.destroy_descriptor_pool(self.pool, None) };
        self.buffer.destroy(ctx);
    }
}

/// One vertex and one index buffer holding every static mesh.
pub(crate) struct MeshArena {
    pub vertices: GpuBuffer,
    pub indices: GpuBuffer,
    pub layout: ArenaLayout,
    pub meshes: Vec<MeshRange>,
}

impl MeshArena {
    pub(crate) fn new(ctx: &mut Context) -> Result<Self, RenderError> {
        let dst = vk::BufferUsageFlags::TRANSFER_DST;
        let mut vertices = GpuBuffer::new(
            ctx,
            "arena-vertices",
            u64::from(ARENA_VERTICES) * std::mem::size_of::<Vertex>() as u64,
            vk::BufferUsageFlags::VERTEX_BUFFER | dst,
            MemoryLocation::GpuOnly,
        )?;
        let indices = match GpuBuffer::new(
            ctx,
            "arena-indices",
            u64::from(ARENA_INDICES) * 4,
            vk::BufferUsageFlags::INDEX_BUFFER | dst,
            MemoryLocation::GpuOnly,
        ) {
            Ok(b) => b,
            Err(e) => {
                vertices.destroy(ctx);
                return Err(e);
            }
        };
        Ok(Self {
            vertices,
            indices,
            layout: ArenaLayout::new(ARENA_VERTICES, ARENA_INDICES),
            meshes: Vec::new(),
        })
    }

    /// Reserves space for a mesh and records its range. Returns the id and
    /// the byte offsets of its vertices and indices in the arena buffers.
    pub(crate) fn add(
        &mut self,
        vertices: &[Vertex],
        index_count: usize,
    ) -> Result<(MeshId, u64, u64), RenderError> {
        let (first_vertex, first_index) = self
            .layout
            .reserve(vertices.len(), index_count)
            .ok_or(RenderError::ArenaFull)?;
        let id = MeshId(self.meshes.len() as u32);
        self.meshes.push(MeshRange {
            first_index,
            index_count: index_count as u32,
            base_vertex: first_vertex as i32,
            bounds: Aabb::from_vertices(vertices),
        });
        Ok((
            id,
            u64::from(first_vertex) * std::mem::size_of::<Vertex>() as u64,
            u64::from(first_index) * 4,
        ))
    }

    pub(crate) fn destroy(&mut self, ctx: &mut Context) {
        self.vertices.destroy(ctx);
        self.indices.destroy(ctx);
    }
}

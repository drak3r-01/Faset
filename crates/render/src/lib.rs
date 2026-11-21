//! Vulkan 1.2 renderer: device selection, windowed swapchain frame loop, a
//! headless offscreen target that reads back to PNG, and the opaque pass
//! that draws static meshes as instances.
//!
//! The renderer never reads the ECS; it only consumes a [`RenderWorld`].
//! Colours are linear; the opaque pass renders to RGBA16F and the result is
//! blitted to the 8-bit sRGB target.
//!
//! Surfaces are painted by procedural materials chosen by the vertex
//! material id (see [`MaterialTable`]); shaders come from `faset-shaders`.
//! The stylised lighting model (banded sun and point lights, hemispheric
//! ambient, rim, height fog) is driven by the [`AmbienceProfile`] and the
//! [`PointLight`]s of the [`RenderWorld`].
//! With the `hot-reload` feature, [`ShaderWatcher`] swaps in edited shaders.

mod ambience;
mod camera;
mod context;
mod cull;
mod deferred;
mod frame;
mod gpu;
#[cfg(feature = "hot-reload")]
mod hot_reload;
mod lighting;
mod material;
mod mesh;
mod opaque;
mod select;
mod target;

use std::path::Path;

use ash::{khr, vk};
use gpu_allocator::MemoryLocation;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};

use context::{Context, WindowHandles};
use cull::{DrawBatch, GpuInstance};
use deferred::{DeferredQueue, FRAMES_IN_FLIGHT};
use frame::{FrameData, Garbage};
use gpu::{image_barriers, GpuBuffer, ImageBarrier};
use lighting::{pack_lighting, LightingBindings, LIGHTING_ALIGN, LIGHTING_SIZE};
use opaque::{GBuffer, MeshArena, OpaquePipeline};
use target::{Offscreen, Swapchain};

pub use ambience::{
    AmbienceError, AmbienceProfile, Ambient, Fog, Grading, Ramp, Rim, Sky, Sun, SunSource, Water,
    Weather, WeatherKind, BUILTIN_PROFILES,
};
pub use camera::{
    halton, jitter, jitter_ndc, jittered_projection, motion_vector, perspective_reverse_z, Camera,
    JITTER_SAMPLES,
};
pub use cull::Instance;
pub use faset_core::glam;
#[cfg(feature = "hot-reload")]
pub use hot_reload::ShaderWatcher;
pub use lighting::{height_fog, point_light_attenuation, PointLight, MAX_POINT_LIGHTS};
pub use material::{Material, MaterialError, MaterialTable, Pattern};
pub use mesh::{cube_mesh, Aabb, CubeFaceColors, MeshId, Vertex, CUBE_FACE_TINTS};

/// Which physical device to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GpuPreference {
    /// Highest score: discrete > integrated > software.
    #[default]
    Auto,
    /// Device at this enumeration index; falls back to `Auto` when it is
    /// missing or unsuitable.
    Index(u32),
}

/// Renderer settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderConfig {
    pub gpu: GpuPreference,
    /// FIFO when on; MAILBOX, else IMMEDIATE, else FIFO when off.
    pub vsync: bool,
    /// Target size in pixels. Used by the headless target; a windowed
    /// renderer takes the window's inner size instead.
    pub size: (u32, u32),
}

impl Default for RenderConfig {
    fn default() -> Self {
        Self {
            gpu: GpuPreference::Auto,
            vsync: true,
            size: (1280, 720),
        }
    }
}

/// What to draw this frame.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderWorld {
    /// Linear RGBA colour of pixels no mesh covers.
    pub clear_color: [f32; 4],
    pub camera: Camera,
    /// Static mesh instances; per-instance data is rewritten every frame.
    pub instances: Vec<Instance>,
    /// Point lights; the first [`MAX_POINT_LIGHTS`] are used, the rest are
    /// dropped with a warning.
    pub lights: Vec<PointLight>,
    /// Lighting and fog parameters. The default (neutral) profile leaves
    /// surfaces at albedo plus emission.
    pub ambience: AmbienceProfile,
}

impl Default for RenderWorld {
    fn default() -> Self {
        Self {
            clear_color: [0.0, 0.0, 0.0, 1.0],
            camera: Camera::default(),
            instances: Vec::new(),
            lights: Vec::new(),
            ambience: AmbienceProfile::default(),
        }
    }
}

/// Counters of the last rendered frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FrameStats {
    /// `vkCmdDrawIndexed` calls (one per visible mesh).
    pub draw_calls: u32,
    /// Instances that passed frustum culling.
    pub instances_drawn: u32,
    /// Instances rejected by frustum culling.
    pub instances_culled: u32,
}

/// Renderer errors.
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("Vulkan loader not found: {0}")]
    Loader(#[from] ash::LoadingError),
    #[error("Vulkan 1.2 is required, the loader provides {0}")]
    UnsupportedVersion(String),
    #[error("no GPU with Vulkan 1.2, VK_KHR_dynamic_rendering and a usable queue")]
    NoSuitableGpu,
    #[error("the surface offers no 8-bit sRGB format")]
    NoSrgbFormat,
    #[error("the swapchain images cannot be transfer destinations")]
    SwapchainUsage,
    #[error("window handle unavailable: {0}")]
    WindowHandle(#[from] raw_window_handle::HandleError),
    #[error("Vulkan call failed: {0}")]
    Vulkan(#[from] vk::Result),
    #[error("GPU memory allocation failed: {0}")]
    Allocation(#[from] gpu_allocator::AllocationError),
    #[error("read-back is only available on a headless renderer")]
    NotHeadless,
    #[error("no frame has been rendered at the current size")]
    NothingRendered,
    #[error("PNG encoding failed: {0}")]
    Png(#[from] image::ImageError),
    #[error("the static mesh arena is full")]
    ArenaFull,
    #[error("invalid mesh: index count not a multiple of 3, or index out of range")]
    InvalidMesh,
}

struct Windowed {
    loader: khr::swapchain::Device,
    /// `None` while minimised or before the first frame.
    swapchain: Option<Swapchain>,
    needs_rebuild: bool,
    vsync: bool,
}

enum Target {
    Windowed(Windowed),
    Headless(Offscreen),
}

/// The renderer. Drop it before the window it was created for.
pub struct Renderer {
    frames: Vec<FrameData>,
    target: Target,
    deferred: DeferredQueue<Garbage>,
    opaque: OpaquePipeline,
    lighting: LightingBindings,
    /// True while the point light list overflows (warned once).
    light_overflow_warned: bool,
    /// Created at the first frame and whenever the target size changes.
    gbuffer: Option<GBuffer>,
    /// Created at the first mesh upload.
    arena: Option<MeshArena>,
    /// Scratch lists reused every frame.
    gpu_instances: Vec<GpuInstance>,
    draws: Vec<DrawBatch>,
    stats: FrameStats,
    /// Requested size; rendering pauses while either side is 0.
    size: (u32, u32),
    /// Frames submitted so far; also the index of the next frame.
    frame_index: u64,
    /// Material animation time in seconds.
    material_time: f32,
    // Last field: dropped after `Drop for Renderer` released everything above.
    ctx: Context,
}

impl Renderer {
    /// Creates a renderer presenting to `window`.
    ///
    /// The window must outlive the renderer.
    pub fn new_windowed(
        window: &winit::window::Window,
        cfg: &RenderConfig,
    ) -> Result<Self, RenderError> {
        let handles = WindowHandles {
            display: window.display_handle()?.as_raw(),
            window: window.window_handle()?.as_raw(),
        };
        let size = window.inner_size();
        Self::build(cfg, Some(&handles), (size.width, size.height), |ctx| {
            Ok(Target::Windowed(Windowed {
                loader: khr::swapchain::Device::new(&ctx.instance, &ctx.device),
                swapchain: None,
                needs_rebuild: true,
                vsync: cfg.vsync,
            }))
        })
    }

    /// Creates a renderer drawing into an offscreen image of `cfg.size`.
    pub fn new_headless(cfg: &RenderConfig) -> Result<Self, RenderError> {
        Self::build(cfg, None, cfg.size, |ctx| {
            // A 0x0 request gets a 1x1 placeholder; frames are skipped until
            // `resize` gives a real size.
            let size = if select::is_zero_area(cfg.size) {
                (1, 1)
            } else {
                cfg.size
            };
            Ok(Target::Headless(Offscreen::new(ctx, size)?))
        })
    }

    fn build(
        cfg: &RenderConfig,
        window: Option<&WindowHandles>,
        size: (u32, u32),
        make_target: impl FnOnce(&mut Context) -> Result<Target, RenderError>,
    ) -> Result<Self, RenderError> {
        let mut ctx = Context::new(cfg.gpu, window)?;
        let mut frames = Vec::with_capacity(FRAMES_IN_FLIGHT);
        let mut result = Ok(());
        for slot in 0..FRAMES_IN_FLIGHT {
            match FrameData::new(&mut ctx, slot) {
                Ok(f) => frames.push(f),
                Err(e) => {
                    result = Err(e);
                    break;
                }
            }
        }
        let mut lighting = None;
        let mut opaque = None;
        let target = result
            .and_then(|()| {
                let bindings = lighting.insert(LightingBindings::new(&ctx, &frames)?);
                let layout = bindings.set_layout;
                opaque = Some(OpaquePipeline::new(
                    &mut ctx,
                    &MaterialTable::builtin(),
                    layout,
                )?);
                Ok(())
            })
            .and_then(|()| make_target(&mut ctx));
        match (target, opaque, lighting) {
            (Ok(target), Some(opaque), Some(lighting)) => Ok(Self {
                frames,
                target,
                deferred: DeferredQueue::new(),
                opaque,
                lighting,
                light_overflow_warned: false,
                gbuffer: None,
                arena: None,
                gpu_instances: Vec::new(),
                draws: Vec::new(),
                stats: FrameStats::default(),
                size,
                frame_index: 0,
                material_time: 0.0,
                ctx,
            }),
            (result, opaque, lighting) => {
                if let Some(mut p) = opaque {
                    p.destroy(&mut ctx);
                }
                if let Some(mut l) = lighting {
                    l.destroy(&ctx);
                }
                for mut f in frames {
                    f.destroy(&mut ctx);
                }
                Err(result
                    .err()
                    .expect("the pipeline exists when the target does"))
            }
        }
    }

    /// Records a new target size. A 0 side pauses rendering until a non-zero
    /// size arrives.
    pub fn resize(&mut self, width: u32, height: u32) {
        if self.size == (width, height) {
            return;
        }
        self.size = (width, height);
        if select::is_zero_area(self.size) {
            return;
        }
        match &mut self.target {
            Target::Windowed(w) => w.needs_rebuild = true,
            Target::Headless(current) => match Offscreen::new(&mut self.ctx, self.size) {
                Ok(new) => {
                    let old = std::mem::replace(current, new);
                    // Earlier frames may still read the old image.
                    self.deferred
                        .push(self.frame_index, Garbage::Image(old.image));
                }
                Err(e) => tracing::error!("failed to resize the offscreen target: {e}"),
            },
        }
    }

    /// Copies a flat-shaded mesh into the static arena.
    ///
    /// Indices are local to `vertices`; triangles are counter-clockwise seen
    /// from outside. Waits for the GPU to go idle (load time only).
    pub fn upload_mesh(
        &mut self,
        vertices: &[Vertex],
        indices: &[u32],
    ) -> Result<MeshId, RenderError> {
        if !indices.len().is_multiple_of(3) || indices.iter().any(|&i| i as usize >= vertices.len())
        {
            return Err(RenderError::InvalidMesh);
        }
        if self.arena.is_none() {
            self.arena = Some(MeshArena::new(&mut self.ctx)?);
        }
        let arena = self.arena.as_mut().expect("created above");
        let (id, vertex_offset, index_offset) = arena.add(vertices, indices.len())?;
        let (vertex_buffer, index_buffer) = (arena.vertices.buffer, arena.indices.buffer);

        let vertex_bytes: &[u8] = bytemuck::cast_slice(vertices);
        let index_bytes: &[u8] = bytemuck::cast_slice(indices);
        let total = (vertex_bytes.len() + index_bytes.len()) as u64;
        if total == 0 {
            return Ok(id);
        }
        let mut staging = GpuBuffer::new(
            &mut self.ctx,
            "mesh-staging",
            total,
            vk::BufferUsageFlags::TRANSFER_SRC,
            MemoryLocation::CpuToGpu,
        )?;
        let mapped = staging.mapped();
        mapped[..vertex_bytes.len()].copy_from_slice(vertex_bytes);
        mapped[vertex_bytes.len()..total as usize].copy_from_slice(index_bytes);
        let src = staging.buffer;
        let result = one_shot(&self.ctx, &self.frames[0], |device, cmd| {
            let barrier = |buffer| {
                vk::BufferMemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                    .dst_access_mask(
                        vk::AccessFlags::VERTEX_ATTRIBUTE_READ | vk::AccessFlags::INDEX_READ,
                    )
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .buffer(buffer)
                    .size(vk::WHOLE_SIZE)
            };
            // SAFETY: recording command buffer, live buffers, regions inside
            // both buffers (the arena reserved them).
            unsafe {
                if !vertex_bytes.is_empty() {
                    let region = vk::BufferCopy {
                        src_offset: 0,
                        dst_offset: vertex_offset,
                        size: vertex_bytes.len() as u64,
                    };
                    device.cmd_copy_buffer(cmd, src, vertex_buffer, &[region]);
                }
                if !index_bytes.is_empty() {
                    let region = vk::BufferCopy {
                        src_offset: vertex_bytes.len() as u64,
                        dst_offset: index_offset,
                        size: index_bytes.len() as u64,
                    };
                    device.cmd_copy_buffer(cmd, src, index_buffer, &[region]);
                }
                device.cmd_pipeline_barrier(
                    cmd,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::PipelineStageFlags::VERTEX_INPUT,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[barrier(vertex_buffer), barrier(index_buffer)],
                    &[],
                );
            }
        });
        staging.destroy(&mut self.ctx);
        result.map(|()| id)
    }

    /// Renders one frame. Returns `Ok` without drawing while the target has
    /// a zero area.
    pub fn render_frame(&mut self, world: &RenderWorld) -> Result<(), RenderError> {
        if select::is_zero_area(self.size) {
            return Ok(());
        }
        if let Target::Windowed(w) = &mut self.target {
            if w.needs_rebuild || w.swapchain.is_none() {
                rebuild_swapchain(&self.ctx, w, self.size)?;
            }
            if w.swapchain.is_none() {
                // The surface reports a zero extent (minimised).
                return Ok(());
            }
        }

        let slot = (self.frame_index % FRAMES_IN_FLIGHT as u64) as usize;
        let fence = self.frames[slot].fence;
        // SAFETY: the fence belongs to this device.
        unsafe { self.ctx.device.wait_for_fences(&[fence], true, u64::MAX) }?;
        for garbage in self.deferred.take_ready(self.frame_index) {
            garbage.destroy(&mut self.ctx);
        }

        // Opaque targets follow the output size; the old ones may still be
        // in use by the other frame in flight.
        let extent = match &self.target {
            Target::Windowed(w) => w.swapchain.as_ref().expect("checked above").extent,
            Target::Headless(o) => o.extent,
        };
        if self.gbuffer.as_ref().is_none_or(|g| g.extent != extent) {
            let new = GBuffer::new(&mut self.ctx, extent)?;
            if let Some(old) = self.gbuffer.replace(new) {
                for image in old.into_images() {
                    self.deferred.push(self.frame_index, Garbage::Image(image));
                }
            }
        }

        // Culling and per-instance data.
        let camera = &world.camera;
        let view_proj = camera.view_proj();
        let meshes = self.arena.as_ref().map_or(&[][..], |a| &a.meshes[..]);
        let unknown = cull::cull_and_batch(
            &view_proj,
            &camera.prev_view_proj,
            &world.instances,
            meshes,
            &mut self.gpu_instances,
            &mut self.draws,
        );
        if unknown > 0 {
            tracing::warn!(
                unknown,
                "instances reference meshes that were never uploaded"
            );
        }
        let frame = &mut self.frames[slot];
        let instance_bytes: &[u8] = bytemuck::cast_slice(&self.gpu_instances);
        if !instance_bytes.is_empty() {
            let needed = instance_bytes.len() as u64;
            if frame.instances.as_ref().is_none_or(|b| b.size < needed) {
                if let Some(mut old) = frame.instances.take() {
                    // This slot's last submission finished (fence above).
                    old.destroy(&mut self.ctx);
                }
                frame.instances = Some(GpuBuffer::new(
                    &mut self.ctx,
                    "frame-instances",
                    needed.next_power_of_two().max(64 * 1024),
                    vk::BufferUsageFlags::VERTEX_BUFFER,
                    MemoryLocation::CpuToGpu,
                )?);
            }
            let buffer = frame.instances.as_mut().expect("allocated above");
            buffer.mapped()[..instance_bytes.len()].copy_from_slice(instance_bytes);
        }
        self.stats = FrameStats {
            draw_calls: self.draws.len() as u32,
            instances_drawn: self.gpu_instances.len() as u32,
            instances_culled: (world.instances.len() - self.gpu_instances.len() - unknown) as u32,
        };
        let jitter = jitter_ndc(camera.jitter_px, (extent.width, extent.height));

        // Lighting block, written into this frame's uniform ring.
        let camera_pos = camera.view.inverse().w_axis.truncate();
        let lighting_block = pack_lighting(
            &world.ambience,
            &world.lights,
            camera_pos,
            &mut self.light_overflow_warned,
        );
        let frame = &mut self.frames[slot];
        frame.uniforms.cursor.reset();
        let lighting_offset = frame
            .uniforms
            .cursor
            .alloc(LIGHTING_SIZE, LIGHTING_ALIGN)
            .expect("the uniform ring holds the lighting block");
        let start = lighting_offset as usize;
        frame
            .uniforms
            .allocation
            .as_mut()
            .and_then(|a| a.mapped_slice_mut())
            .expect("host-visible uniform ring")[start..start + LIGHTING_SIZE as usize]
            .copy_from_slice(bytemuck::bytes_of(&lighting_block));
        let lighting_set = self.lighting.sets[slot];
        let device = &self.ctx.device;

        // Pick the image to draw into.
        let (image, image_index) = match &mut self.target {
            Target::Windowed(w) => {
                let sc = w.swapchain.as_ref().expect("checked above");
                // SAFETY: live swapchain; the semaphore is unsignalled because
                // this slot's previous submission, which waited on it, is done.
                let acquired = unsafe {
                    w.loader.acquire_next_image(
                        sc.handle,
                        u64::MAX,
                        frame.image_available,
                        vk::Fence::null(),
                    )
                };
                let index = match acquired {
                    Ok((index, suboptimal)) => {
                        w.needs_rebuild |= suboptimal;
                        index
                    }
                    Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                        w.needs_rebuild = true;
                        return Ok(());
                    }
                    Err(e) => return Err(e.into()),
                };
                (sc.images[index as usize], Some(index))
            }
            Target::Headless(o) => (o.image.image, None),
        };

        let gbuffer = self.gbuffer.as_ref().expect("created above");
        let arena = self.arena.as_ref();
        let color = vk::ImageAspectFlags::COLOR;
        let attachment = |image| ImageBarrier {
            image,
            aspect: color,
            old: vk::ImageLayout::UNDEFINED,
            new: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
            src_access: vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
            dst_access: vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
        };
        let begin_barriers = [
            attachment(gbuffer.hdr.image),
            attachment(gbuffer.motion.image),
            attachment(gbuffer.normal.image),
            ImageBarrier {
                image: gbuffer.depth.image,
                aspect: vk::ImageAspectFlags::DEPTH,
                old: vk::ImageLayout::UNDEFINED,
                new: vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL,
                src_access: vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE,
                dst_access: vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_READ
                    | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE,
            },
        ];
        let clear_color = |c: [f32; 4]| vk::ClearValue {
            color: vk::ClearColorValue { float32: c },
        };
        let color_attachments = [
            (gbuffer.hdr.view, clear_color(world.clear_color)),
            (gbuffer.motion.view, clear_color([0.0; 4])),
            (gbuffer.normal.view, clear_color([0.5, 0.5, 0.0, 0.0])),
        ]
        .map(|(view, clear)| {
            vk::RenderingAttachmentInfo::default()
                .image_view(view)
                .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                .load_op(vk::AttachmentLoadOp::CLEAR)
                .store_op(vk::AttachmentStoreOp::STORE)
                .clear_value(clear)
        });
        let depth_attachment = vk::RenderingAttachmentInfo::default()
            .image_view(gbuffer.depth.view)
            .image_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .clear_value(vk::ClearValue {
                depth_stencil: vk::ClearDepthStencilValue {
                    depth: 0.0,
                    stencil: 0,
                },
            });
        let full = vk::Rect2D {
            offset: vk::Offset2D::default(),
            extent,
        };
        let rendering = vk::RenderingInfo::default()
            .render_area(full)
            .layer_count(1)
            .color_attachments(&color_attachments)
            .depth_attachment(&depth_attachment);

        let shader_read = |image| ImageBarrier {
            image,
            aspect: color,
            old: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
            new: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            src_access: vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
            dst_access: vk::AccessFlags::SHADER_READ | vk::AccessFlags::TRANSFER_READ,
        };
        let end_barriers = [
            ImageBarrier {
                image: gbuffer.hdr.image,
                aspect: color,
                old: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                new: vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                src_access: vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
                dst_access: vk::AccessFlags::TRANSFER_READ,
            },
            shader_read(gbuffer.motion.image),
            shader_read(gbuffer.normal.image),
            ImageBarrier {
                image,
                aspect: color,
                old: vk::ImageLayout::UNDEFINED,
                new: vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                src_access: vk::AccessFlags::empty(),
                dst_access: vk::AccessFlags::TRANSFER_WRITE,
            },
        ];
        let corner = vk::Offset3D {
            x: extent.width as i32,
            y: extent.height as i32,
            z: 1,
        };
        let layers = vk::ImageSubresourceLayers {
            aspect_mask: color,
            mip_level: 0,
            base_array_layer: 0,
            layer_count: 1,
        };
        let blit = vk::ImageBlit {
            src_subresource: layers,
            src_offsets: [vk::Offset3D::default(), corner],
            dst_subresource: layers,
            dst_offsets: [vk::Offset3D::default(), corner],
        };
        let (final_layout, final_access, final_stage) = if image_index.is_some() {
            (
                vk::ImageLayout::PRESENT_SRC_KHR,
                vk::AccessFlags::empty(),
                vk::PipelineStageFlags::BOTTOM_OF_PIPE,
            )
        } else {
            (
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                vk::AccessFlags::TRANSFER_READ,
                vk::PipelineStageFlags::TRANSFER,
            )
        };
        let final_barrier = [ImageBarrier {
            image,
            aspect: color,
            old: vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            new: final_layout,
            src_access: vk::AccessFlags::TRANSFER_WRITE,
            dst_access: final_access,
        }];
        let viewport = vk::Viewport {
            x: 0.0,
            y: 0.0,
            width: extent.width as f32,
            height: extent.height as f32,
            min_depth: 0.0,
            max_depth: 1.0,
        };
        let push_bytes: [u8; opaque::PUSH_CONSTANT_SIZE as usize] =
            bytemuck::cast([jitter.x, jitter.y, self.material_time, 0.0]);

        // SAFETY: the fence wait above means the GPU is done with this slot's
        // pool, command buffer and instance buffer; the fence is reset only
        // once an image was acquired, so an early return never leaves it
        // unsignalled. The G-buffer is shared by both frames in flight: the
        // first barrier orders this frame after the previous one's writes and
        // blit reads (same queue, submission order).
        unsafe {
            device.reset_fences(&[fence])?;
            device.reset_command_pool(frame.pool, vk::CommandPoolResetFlags::empty())?;
            device.begin_command_buffer(
                frame.cmd,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )?;
            image_barriers(
                device,
                frame.cmd,
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                    | vk::PipelineStageFlags::LATE_FRAGMENT_TESTS
                    | vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                    | vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS
                    | vk::PipelineStageFlags::LATE_FRAGMENT_TESTS,
                &begin_barriers,
            );
            self.ctx
                .dynamic_rendering
                .cmd_begin_rendering(frame.cmd, &rendering);
            device.cmd_bind_pipeline(
                frame.cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.opaque.pipeline,
            );
            device.cmd_set_viewport(frame.cmd, 0, &[viewport]);
            device.cmd_set_scissor(frame.cmd, 0, &[full]);
            device.cmd_push_constants(
                frame.cmd,
                self.opaque.layout,
                opaque::PUSH_CONSTANT_STAGES,
                0,
                &push_bytes,
            );
            device.cmd_bind_descriptor_sets(
                frame.cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.opaque.layout,
                0,
                &[self.opaque.materials.set, lighting_set],
                &[lighting_offset as u32],
            );
            if let (Some(arena), Some(instances), false) =
                (arena, frame.instances.as_ref(), self.draws.is_empty())
            {
                device.cmd_bind_vertex_buffers(
                    frame.cmd,
                    0,
                    &[arena.vertices.buffer, instances.buffer],
                    &[0, 0],
                );
                device.cmd_bind_index_buffer(
                    frame.cmd,
                    arena.indices.buffer,
                    0,
                    vk::IndexType::UINT32,
                );
                for d in &self.draws {
                    device.cmd_draw_indexed(
                        frame.cmd,
                        d.index_count,
                        d.instance_count,
                        d.first_index,
                        d.base_vertex,
                        d.first_instance,
                    );
                }
            }
            self.ctx.dynamic_rendering.cmd_end_rendering(frame.cmd);
            image_barriers(
                device,
                frame.cmd,
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT | vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::TRANSFER | vk::PipelineStageFlags::FRAGMENT_SHADER,
                &end_barriers,
            );
            device.cmd_blit_image(
                frame.cmd,
                gbuffer.hdr.image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[blit],
                vk::Filter::NEAREST,
            );
            image_barriers(
                device,
                frame.cmd,
                vk::PipelineStageFlags::TRANSFER,
                final_stage,
                &final_barrier,
            );
            device.end_command_buffer(frame.cmd)?;
        }

        let cmds = [frame.cmd];
        match (&mut self.target, image_index) {
            (Target::Windowed(w), Some(index)) => {
                let sc = w.swapchain.as_ref().expect("checked above");
                let wait = [frame.image_available];
                // The image is first touched by the transfer barrier.
                let wait_stages = [vk::PipelineStageFlags::TRANSFER];
                let signal = [sc.render_finished[index as usize]];
                let submit = vk::SubmitInfo::default()
                    .command_buffers(&cmds)
                    .wait_semaphores(&wait)
                    .wait_dst_stage_mask(&wait_stages)
                    .signal_semaphores(&signal);
                // SAFETY: recorded command buffer, live semaphores and fence.
                unsafe { device.queue_submit(self.ctx.queue, &[submit], fence) }?;
                let swapchains = [sc.handle];
                let indices = [index];
                let present = vk::PresentInfoKHR::default()
                    .wait_semaphores(&signal)
                    .swapchains(&swapchains)
                    .image_indices(&indices);
                // SAFETY: the image was acquired this frame and reaches
                // PRESENT_SRC layout before `signal` fires.
                match unsafe { w.loader.queue_present(self.ctx.queue, &present) } {
                    Ok(suboptimal) => w.needs_rebuild |= suboptimal,
                    Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => w.needs_rebuild = true,
                    Err(e) => return Err(e.into()),
                }
            }
            (Target::Headless(o), _) => {
                let submit = vk::SubmitInfo::default().command_buffers(&cmds);
                // SAFETY: recorded command buffer and live fence.
                unsafe { device.queue_submit(self.ctx.queue, &[submit], fence) }?;
                o.rendered = true;
            }
            (Target::Windowed(_), None) => unreachable!("windowed frames acquire an image"),
        }
        self.frame_index += 1;
        Ok(())
    }

    /// Copies the last rendered headless frame to a PNG file (RGBA, sRGB).
    pub fn read_back_png(&mut self, path: &Path) -> Result<(), RenderError> {
        let Target::Headless(offscreen) = &self.target else {
            return Err(RenderError::NotHeadless);
        };
        if !offscreen.rendered || select::is_zero_area(self.size) {
            return Err(RenderError::NothingRendered);
        }
        let (image, extent) = (offscreen.image.image, offscreen.extent);
        let bytes = self.copy_to_host(image, extent, vk::ImageLayout::TRANSFER_SRC_OPTIMAL, 4)?;
        image::save_buffer(
            path,
            &bytes,
            extent.width,
            extent.height,
            image::ExtendedColorType::Rgba8,
        )?;
        Ok(())
    }

    /// Motion vectors of the last headless frame, row-major, in UV units per
    /// frame (current minus previous position, x right, y down).
    pub fn read_back_motion(&mut self) -> Result<Vec<[f32; 2]>, RenderError> {
        let Target::Headless(offscreen) = &self.target else {
            return Err(RenderError::NotHeadless);
        };
        let gbuffer = match &self.gbuffer {
            Some(g) if offscreen.rendered && !select::is_zero_area(self.size) => g,
            _ => return Err(RenderError::NothingRendered),
        };
        let (image, extent) = (gbuffer.motion.image, gbuffer.extent);
        let bytes =
            self.copy_to_host(image, extent, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL, 4)?;
        Ok(bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| {
                [
                    f16_to_f32(u16::from_le_bytes([c[0], c[1]])),
                    f16_to_f32(u16::from_le_bytes([c[2], c[3]])),
                ]
            })
            .collect())
    }

    /// Copies a colour image (left in `layout`) to host memory.
    fn copy_to_host(
        &mut self,
        image: vk::Image,
        extent: vk::Extent2D,
        layout: vk::ImageLayout,
        bytes_per_pixel: u64,
    ) -> Result<Vec<u8>, RenderError> {
        let byte_len = u64::from(extent.width) * u64::from(extent.height) * bytes_per_pixel;
        let mut buffer = GpuBuffer::new(
            &mut self.ctx,
            "readback",
            byte_len,
            vk::BufferUsageFlags::TRANSFER_DST,
            MemoryLocation::GpuToCpu,
        )?;
        let dst = buffer.buffer;
        let region = vk::BufferImageCopy::default()
            .image_subresource(vk::ImageSubresourceLayers {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                mip_level: 0,
                base_array_layer: 0,
                layer_count: 1,
            })
            .image_extent(extent.into());
        let to_host = vk::BufferMemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
            .dst_access_mask(vk::AccessFlags::HOST_READ)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .buffer(dst)
            .size(vk::WHOLE_SIZE);
        let needs_transition = layout != vk::ImageLayout::TRANSFER_SRC_OPTIMAL;
        let swap = |old, new| ImageBarrier {
            image,
            aspect: vk::ImageAspectFlags::COLOR,
            old,
            new,
            src_access: vk::AccessFlags::MEMORY_WRITE,
            dst_access: vk::AccessFlags::MEMORY_READ,
        };
        let copied = one_shot(&self.ctx, &self.frames[0], |device, cmd| {
            // SAFETY: recording command buffer, live image and buffer.
            unsafe {
                if needs_transition {
                    image_barriers(
                        device,
                        cmd,
                        vk::PipelineStageFlags::ALL_COMMANDS,
                        vk::PipelineStageFlags::TRANSFER,
                        &[swap(layout, vk::ImageLayout::TRANSFER_SRC_OPTIMAL)],
                    );
                }
                device.cmd_copy_image_to_buffer(
                    cmd,
                    image,
                    vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                    dst,
                    &[region],
                );
                device.cmd_pipeline_barrier(
                    cmd,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::PipelineStageFlags::HOST,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[to_host],
                    &[],
                );
                if needs_transition {
                    image_barriers(
                        device,
                        cmd,
                        vk::PipelineStageFlags::TRANSFER,
                        vk::PipelineStageFlags::ALL_COMMANDS,
                        &[swap(vk::ImageLayout::TRANSFER_SRC_OPTIMAL, layout)],
                    );
                }
            }
        });
        let result = copied.map(|()| buffer.mapped()[..byte_len as usize].to_vec());
        buffer.destroy(&mut self.ctx);
        result
    }

    /// Current target size in pixels.
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// Frames submitted so far (skipped frames are not counted).
    pub fn frames_rendered(&self) -> u64 {
        self.frame_index
    }

    /// Culling and draw counters of the last rendered frame.
    pub fn last_frame_stats(&self) -> FrameStats {
        self.stats
    }

    /// Replaces the material table (the built-in one at creation). Waits
    /// for the GPU to go idle (load time only).
    pub fn set_materials(&mut self, table: &MaterialTable) -> Result<(), RenderError> {
        // SAFETY: valid device; once idle no frame reads the table.
        unsafe { self.ctx.device.device_wait_idle() }?;
        self.opaque.materials.write(table);
        Ok(())
    }

    /// Sets the time, in seconds, that animates water, lava, emissive and
    /// hologram materials from the next frame on (0 at creation).
    pub fn set_material_time(&mut self, seconds: f32) {
        self.material_time = seconds;
    }

    /// Swaps the opaque pipeline for one built from new SPIR-V; keeps the
    /// current one on error.
    #[cfg(feature = "hot-reload")]
    pub(crate) fn replace_opaque_shaders(
        &mut self,
        vs: &[u32],
        fs: &[u32],
    ) -> Result<(), RenderError> {
        // SAFETY: valid device; once idle no frame uses the old pipeline.
        unsafe { self.ctx.device.device_wait_idle() }?;
        self.opaque.replace_shaders(&self.ctx.device, vs, fs)
    }

    /// True when the Khronos validation layer is active (debug builds with
    /// the layer installed).
    pub fn validation_enabled(&self) -> bool {
        self.ctx.validation_enabled()
    }

    /// Warning and error messages reported by the validation layer so far.
    pub fn validation_message_count(&self) -> u32 {
        self.ctx.validation_messages()
    }
}

/// Records and runs one command buffer on `frame`, waiting for it. Waits for
/// the device to be idle first, so the slot is free.
fn one_shot(
    ctx: &Context,
    frame: &FrameData,
    record: impl FnOnce(&ash::Device, vk::CommandBuffer),
) -> Result<(), RenderError> {
    let device = &ctx.device;
    let cmds = [frame.cmd];
    let submit = vk::SubmitInfo::default().command_buffers(&cmds);
    // SAFETY: once the device is idle the slot's pool, command buffer and
    // (signalled) fence are free; the fence is signalled again on return.
    unsafe {
        device.device_wait_idle()?;
        device.reset_command_pool(frame.pool, vk::CommandPoolResetFlags::empty())?;
        device.begin_command_buffer(
            frame.cmd,
            &vk::CommandBufferBeginInfo::default()
                .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
        )?;
        record(device, frame.cmd);
        device.end_command_buffer(frame.cmd)?;
        device.reset_fences(&[frame.fence])?;
        device.queue_submit(ctx.queue, &[submit], frame.fence)?;
        device.wait_for_fences(&[frame.fence], true, u64::MAX)?;
    }
    Ok(())
}

/// IEEE 754 half to single precision.
fn f16_to_f32(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exponent = i32::from((h >> 10) & 0x1f);
    let mantissa = f32::from(h & 0x3ff);
    match exponent {
        0 => sign * mantissa * 2f32.powi(-24),
        31 if mantissa == 0.0 => sign * f32::INFINITY,
        31 => f32::NAN,
        e => sign * (1.0 + mantissa / 1024.0) * 2f32.powi(e - 15),
    }
}

/// Recreates the swapchain for `size` after waiting for the GPU.
fn rebuild_swapchain(ctx: &Context, w: &mut Windowed, size: (u32, u32)) -> Result<(), RenderError> {
    // SAFETY: valid device. Waiting keeps the old swapchain's images and
    // semaphores out of GPU use before they are destroyed.
    unsafe { ctx.device.device_wait_idle() }?;
    let old = w.swapchain.take();
    let old_handle = old.as_ref().map_or(vk::SwapchainKHR::null(), |s| s.handle);
    let new = Swapchain::new(ctx, &w.loader, size, w.vsync, old_handle);
    if let Some(mut old) = old {
        old.destroy(ctx, &w.loader);
    }
    w.swapchain = new?;
    w.needs_rebuild = false;
    Ok(())
}

impl Drop for Renderer {
    fn drop(&mut self) {
        // SAFETY: valid device; waiting makes every resource below unused.
        unsafe {
            let _ = self.ctx.device.device_wait_idle();
        }
        for garbage in self.deferred.take_all() {
            garbage.destroy(&mut self.ctx);
        }
        match &mut self.target {
            Target::Windowed(w) => {
                if let Some(mut sc) = w.swapchain.take() {
                    sc.destroy(&self.ctx, &w.loader);
                }
            }
            Target::Headless(o) => o.image.destroy(&mut self.ctx),
        }
        if let Some(g) = self.gbuffer.take() {
            g.destroy(&mut self.ctx);
        }
        if let Some(mut a) = self.arena.take() {
            a.destroy(&mut self.ctx);
        }
        self.opaque.destroy(&mut self.ctx);
        self.lighting.destroy(&self.ctx);
        for mut f in self.frames.drain(..) {
            f.destroy(&mut self.ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::f16_to_f32;

    #[test]
    fn half_floats_decode() {
        assert_eq!(f16_to_f32(0x0000), 0.0);
        assert_eq!(f16_to_f32(0x3c00), 1.0);
        assert_eq!(f16_to_f32(0xc000), -2.0);
        assert_eq!(f16_to_f32(0x3555), 0.333_251_95);
        assert_eq!(f16_to_f32(0x0001), 2f32.powi(-24));
        assert!(f16_to_f32(0x7e00).is_nan());
    }
}

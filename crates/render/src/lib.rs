//! Vulkan 1.2 renderer: device selection, windowed swapchain frame loop and a
//! headless offscreen target that reads back to PNG.
//!
//! The renderer never reads the ECS; it only consumes a [`RenderWorld`].
//! Colours are linear; targets are 8-bit sRGB.

mod context;
mod deferred;
mod frame;
mod select;
mod target;

use std::path::Path;

use ash::{khr, vk};
use gpu_allocator::vulkan::{AllocationCreateDesc, AllocationScheme};
use gpu_allocator::MemoryLocation;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};

use context::{Context, WindowHandles};
use deferred::{DeferredQueue, FRAMES_IN_FLIGHT};
use frame::{transition, FrameData, Garbage, Transition};
use target::{Offscreen, Swapchain};

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

/// What to draw this frame. Skeleton: later lots add contents.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderWorld {
    /// Linear RGBA clear colour.
    pub clear_color: [f32; 4],
}

impl Default for RenderWorld {
    fn default() -> Self {
        Self {
            clear_color: [0.0, 0.0, 0.0, 1.0],
        }
    }
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
    /// Requested size; rendering pauses while either side is 0.
    size: (u32, u32),
    /// Frames submitted so far; also the index of the next frame.
    frame_index: u64,
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
        let target = result.and_then(|()| make_target(&mut ctx));
        match target {
            Ok(target) => Ok(Self {
                frames,
                target,
                deferred: DeferredQueue::new(),
                size,
                frame_index: 0,
                ctx,
            }),
            Err(e) => {
                for mut f in frames {
                    f.destroy(&mut ctx);
                }
                Err(e)
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

        let frame = &mut self.frames[slot];
        frame.uniforms.cursor.reset();
        let device = &self.ctx.device;

        // Pick the image to draw into.
        let (image, view, extent, image_index) = match &mut self.target {
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
                let i = index as usize;
                (sc.images[i], sc.views[i], sc.extent, Some(index))
            }
            Target::Headless(o) => (o.image.image, o.image.view, o.extent, None),
        };

        let final_transition = if image_index.is_some() {
            Transition {
                old: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                new: vk::ImageLayout::PRESENT_SRC_KHR,
                src_stage: vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                src_access: vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
                dst_stage: vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                dst_access: vk::AccessFlags::empty(),
            }
        } else {
            Transition {
                old: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                new: vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                src_stage: vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                src_access: vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
                dst_stage: vk::PipelineStageFlags::TRANSFER,
                dst_access: vk::AccessFlags::TRANSFER_READ,
            }
        };
        let attachments = [vk::RenderingAttachmentInfo::default()
            .image_view(view)
            .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .clear_value(vk::ClearValue {
                color: vk::ClearColorValue {
                    float32: world.clear_color,
                },
            })];
        let rendering = vk::RenderingInfo::default()
            .render_area(vk::Rect2D {
                offset: vk::Offset2D::default(),
                extent,
            })
            .layer_count(1)
            .color_attachments(&attachments);

        // SAFETY: the fence wait above means the GPU is done with this slot's
        // pool and command buffer; the fence is reset only once an image was
        // acquired, so an early return never leaves it unsignalled.
        unsafe {
            device.reset_fences(&[fence])?;
            device.reset_command_pool(frame.pool, vk::CommandPoolResetFlags::empty())?;
            device.begin_command_buffer(
                frame.cmd,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )?;
            transition(
                device,
                frame.cmd,
                image,
                &Transition {
                    old: vk::ImageLayout::UNDEFINED,
                    new: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                    src_stage: vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                    src_access: vk::AccessFlags::empty(),
                    dst_stage: vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                    dst_access: vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
                },
            );
            self.ctx
                .dynamic_rendering
                .cmd_begin_rendering(frame.cmd, &rendering);
            self.ctx.dynamic_rendering.cmd_end_rendering(frame.cmd);
            transition(device, frame.cmd, image, &final_transition);
            device.end_command_buffer(frame.cmd)?;
        }

        let cmds = [frame.cmd];
        match (&mut self.target, image_index) {
            (Target::Windowed(w), Some(index)) => {
                let sc = w.swapchain.as_ref().expect("checked above");
                let wait = [frame.image_available];
                let wait_stages = [vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT];
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
        let extent = offscreen.extent;
        let image = offscreen.image.image;
        let byte_len = u64::from(extent.width) * u64::from(extent.height) * 4;
        let ctx = &mut self.ctx;

        // SAFETY: valid device; the wait makes every frame slot reusable.
        unsafe { ctx.device.device_wait_idle() }?;
        let info = vk::BufferCreateInfo::default()
            .size(byte_len)
            .usage(vk::BufferUsageFlags::TRANSFER_DST)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        // SAFETY: valid create info.
        let buffer = unsafe { ctx.device.create_buffer(&info, None) }?;
        // SAFETY: live buffer.
        let requirements = unsafe { ctx.device.get_buffer_memory_requirements(buffer) };
        let allocation = match ctx.allocator.allocate(&AllocationCreateDesc {
            name: "readback",
            requirements,
            location: MemoryLocation::GpuToCpu,
            linear: true,
            allocation_scheme: AllocationScheme::GpuAllocatorManaged,
        }) {
            Ok(a) => a,
            Err(e) => {
                // SAFETY: the buffer was never used.
                unsafe { ctx.device.destroy_buffer(buffer, None) };
                return Err(e.into());
            }
        };

        let frame = &self.frames[0];
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
            .buffer(buffer)
            .size(vk::WHOLE_SIZE);
        let cmds = [frame.cmd];
        let submit = vk::SubmitInfo::default().command_buffers(&cmds);
        let device = &ctx.device;
        // SAFETY: the device is idle, so frame 0's pool, command buffer and
        // (signalled) fence are free; a headless frame leaves the image in
        // TRANSFER_SRC layout; the memory fits the buffer's requirements.
        let copied = unsafe {
            (|| -> Result<(), vk::Result> {
                device.bind_buffer_memory(buffer, allocation.memory(), allocation.offset())?;
                device.reset_command_pool(frame.pool, vk::CommandPoolResetFlags::empty())?;
                device.begin_command_buffer(
                    frame.cmd,
                    &vk::CommandBufferBeginInfo::default()
                        .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                )?;
                device.cmd_copy_image_to_buffer(
                    frame.cmd,
                    image,
                    vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                    buffer,
                    &[region],
                );
                device.cmd_pipeline_barrier(
                    frame.cmd,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::PipelineStageFlags::HOST,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[to_host],
                    &[],
                );
                device.end_command_buffer(frame.cmd)?;
                device.reset_fences(&[frame.fence])?;
                device.queue_submit(ctx.queue, &[submit], frame.fence)?;
                device.wait_for_fences(&[frame.fence], true, u64::MAX)
            })()
        };

        let result = copied.map_err(RenderError::from).and_then(|()| {
            let bytes = allocation
                .mapped_slice()
                .expect("GpuToCpu memory is host-mapped");
            image::save_buffer(
                path,
                &bytes[..byte_len as usize],
                extent.width,
                extent.height,
                image::ExtendedColorType::Rgba8,
            )
            .map_err(RenderError::from)
        });

        // SAFETY: the copy finished (or never started); nothing else uses it.
        unsafe { ctx.device.destroy_buffer(buffer, None) };
        if let Err(e) = ctx.allocator.free(allocation) {
            tracing::warn!("failed to free read-back memory: {e}");
        }
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
        for mut f in self.frames.drain(..) {
            f.destroy(&mut self.ctx);
        }
    }
}

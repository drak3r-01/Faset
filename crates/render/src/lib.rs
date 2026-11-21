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
//! [`PointLight`]s of the [`RenderWorld`]. The sun casts shadows through
//! three texel-snapped cascades ([`fit_cascades`]) drawn in a first pass.
//! A procedural sky (gradient, sun, moon, stars, clouds, and a nebula with
//! planets in space profiles) fills the pixels no mesh covers when
//! [`RenderWorld::draw_sky`] is set; its time of day, latitude and clouds come
//! from [`SkyParams`], and with `SunSource::TimeOfDay` the sun, the sky and
//! the ambient light follow the map time.
//! Visual weather ([`WeatherState`]: rain, snow, sand, fog, wind, humidity and
//! storm) is drawn as stateless GPU particles in a box that follows the
//! camera, masked under roofs by a [`ShelterMap`]; humidity darkens and
//! reflects surfaces, storms flash lightning and queue [`ThunderEvent`]s for
//! the audio lot ([`Renderer::take_thunder_events`]); [`WeatherSchedule`]
//! blends keyframes over 10 to 30 s.
//! Water ([`WaterParams`]: a rest height and up to eight Gerstner waves) is a
//! displaced grid around the camera; below it ([`RenderWorld::underwater`])
//! the height fog is replaced by a dense blue-green exponential fog, sunlit
//! surfaces get animated caustics and suspended particles drift in the view.
//! The optional post chain ([`Renderer::set_post_settings`], [`PostSettings`],
//! [`PostParams`]) runs on the HDR buffer: contours, bloom, ACES tonemapping,
//! grading, screen effects and FXAA, each switchable and driven by the profile.
//! An optional pass 6 ([`Renderer::set_upscaler`], [`UpscalePass`]) sits
//! between the contours and the rest of the post chain: the scene is drawn at
//! the internal resolution the pass asks for, with the motion vectors (opaque
//! meshes, sky and water), depth and a reactive mask (weather particles) it
//! reads, and its output has the display size. The `faset-upscale` crate
//! implements it (native, temporal anti-aliasing, FSR 1).
//! An optional pass 9 ([`Renderer::set_ui_pass`], [`UiPass`]) draws the
//! interface on the finished frame at the display resolution; the `faset-ui`
//! crate implements it.
//! With the `hot-reload` feature, [`ShaderWatcher`] swaps in edited shaders.

mod ambience;
mod camera;
mod context;
mod cull;
mod deferred;
mod extensions;
mod frame;
mod gpu;
mod hook;
#[cfg(feature = "hot-reload")]
mod hot_reload;
mod lighting;
mod material;
mod mesh;
mod opaque;
mod post;
mod select;
mod shadow;
mod shelter;
mod sky;
mod target;
mod water;
mod weather;

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
use post::{PostChain, PostInputs};
use shadow::ShadowResources;
use shelter::ShelterBuffer;
use sky::SkyPipeline;
use target::{Offscreen, Swapchain};
use water::{GpuWater, WaterPipeline};
use weather::{GpuWeather, WeatherPipeline};

pub use ambience::{
    AmbienceError, AmbienceProfile, Ambient, Fog, Grading, Ramp, Rim, ShadowBias, Sky, Sun,
    SunSource, Water, Weather, WeatherKind, BUILTIN_PROFILES,
};
pub use camera::{
    halton, jitter, jitter_ndc, jittered_projection, motion_vector, perspective_reverse_z, Camera,
    JITTER_SAMPLES,
};
pub use cull::Instance;
pub use extensions::DeviceExtensions;
pub use faset_core::glam;
pub use hook::{FrameCtx, GpuHandles, ImageHandle, UiPass, UpscaleInputs, UpscalePass};
#[cfg(feature = "hot-reload")]
pub use hot_reload::ShaderWatcher;
pub use lighting::{height_fog, point_light_attenuation, PointLight, MAX_POINT_LIGHTS};
pub use material::{Material, MaterialError, MaterialTable, Pattern};
pub use mesh::{cube_mesh, Aabb, CubeFaceColors, MeshId, Vertex, CUBE_FACE_TINTS};
pub use post::{
    aces_tonemap, bloom_extents, tonemap_pixel, PostParams, PostSettings, ScreenEffects,
    BLOOM_LEVELS,
};
pub use shadow::{
    cascade_splits, fit_cascades, ShadowCascades, CASCADE_COUNT, SHADOW_DISTANCE, SHADOW_MAP_SIZE,
};
pub use shelter::{ShelterError, ShelterMap, MAX_SHELTER_SIDE, NO_ROOF};
pub use sky::{
    cell_has_star, hash_cell, henyey_greenstein, star_cell, sun_altitude_azimuth, sun_direction,
    SkyParams, STAR_COLS, STAR_ROWS, SUN_PHASE_G,
};
pub use water::{
    angular_frequency, gerstner_displace, is_underwater, mote_position, underwater_profile,
    wavenumber, GerstnerWave, WaterError, WaterParams, GRAVITY, GRID_CELLS, GRID_EXTENT, KIND_MOTE,
    MAX_WAVES, MOTE_BOX, MOTE_COUNT, MOTE_VELOCITY,
};
pub use weather::{
    flash_envelope, lightning_flash, particle_count, particle_hash, particle_position,
    particle_velocity, snow_sway, strike_in_slot, strikes_between, thunder_delay, Strike,
    ThunderEvent, WeatherError, WeatherKeyframe, WeatherSchedule, WeatherState, BOX_SIZE,
    FLASH_DURATION_S, KIND_RAIN, KIND_SAND, KIND_SNOW, LIGHTNING_MIN_STORM, LIGHTNING_SLOT_S,
    MAX_PARTICLES, MAX_TRANSITION_S, MAX_WIND_SPEED, MIN_TRANSITION_S, SNOW_SWAY, SPEED_OF_SOUND,
};

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
    /// Map time, latitude and cloud settings of the sky.
    pub sky: SkyParams,
    /// Draws the procedural sky where no mesh covers a pixel, and makes the
    /// fog take the sky colour. Off by default: those pixels then keep
    /// `clear_color` and the fog its profile colour.
    pub draw_sky: bool,
    /// Visual weather of the frame (sample a [`WeatherSchedule`] to get it).
    /// Particles and lightning animate with the time set by
    /// [`Renderer::set_material_time`]. Clear by default.
    pub weather: WeatherState,
    /// The map's water, drawn as a surface displaced by Gerstner waves
    /// (invalid parameters are ignored with a warning). Caustics need it.
    pub water: Option<WaterParams>,
    /// The camera is below the water surface: underwater fog, caustics and
    /// suspended particles replace the height fog, and the sky is not drawn.
    /// Keep it in sync with [`RenderWorld::update_underwater`]. The image
    /// ripple of lot 2.9 reads this flag.
    pub underwater: bool,
    /// Gameplay screen effects (damage flash, speed lines) of the post chain.
    pub screen: ScreenEffects,
    /// Replaces the post parameters otherwise derived from the ambience
    /// profile ([`PostParams::from_profile`]) and `screen`.
    pub post: Option<PostParams>,
}

impl RenderWorld {
    /// Sets [`RenderWorld::underwater`]: true exactly when the camera is
    /// below the height of [`RenderWorld::water`] (false without water).
    pub fn update_underwater(&mut self) {
        let camera_y = self.camera.view.inverse().w_axis.y;
        self.underwater = self
            .water
            .as_ref()
            .is_some_and(|w| w.contains_camera_height(camera_y));
    }
}

impl Default for RenderWorld {
    fn default() -> Self {
        Self {
            clear_color: [0.0, 0.0, 0.0, 1.0],
            camera: Camera::default(),
            instances: Vec::new(),
            lights: Vec::new(),
            ambience: AmbienceProfile::default(),
            sky: SkyParams::default(),
            draw_sky: false,
            weather: WeatherState::CLEAR,
            water: None,
            underwater: false,
            screen: ScreenEffects::default(),
            post: None,
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
    sky: SkyPipeline,
    weather: WeatherPipeline,
    water: WaterPipeline,
    /// True once invalid water parameters were reported.
    water_warned: bool,
    /// The post-processing chain; `None` (the default) copies the HDR buffer
    /// straight to the target. See [`Renderer::set_post_settings`].
    post: Option<PostChain>,
    /// Pass 6, between the scene and the post chain (none: the scene is
    /// drawn at the display resolution). See [`Renderer::set_upscaler`].
    upscaler: Option<Box<dyn UpscalePass>>,
    /// Pass 9, after the post chain, at the display resolution (none: no
    /// interface). See [`Renderer::set_ui_pass`].
    ui_pass: Option<Box<dyn UiPass>>,
    /// The next frame discards the upscaler's temporal history.
    reset_history: bool,
    /// Seconds between frames, as given to the upscaler.
    frame_time: f32,
    /// Roofs that mask weather particles (none until `set_shelter_map`).
    shelter: ShelterBuffer,
    /// Thunder of the strikes since the last `take_thunder_events`.
    thunder: Vec<ThunderEvent>,
    /// Material time of the previous frame, to find the strikes in between.
    last_weather_time: Option<f32>,
    shadow: ShadowResources,
    lighting: LightingBindings,
    /// Fits the cascades to this camera instead of the view camera.
    shadow_camera: Option<Camera>,
    /// Per-cascade shadow caster draws; their instances follow the view's
    /// instances in the frame's instance buffer.
    shadow_draws: [Vec<DrawBatch>; CASCADE_COUNT],
    /// Scratch lists of one cascade's culling.
    shadow_scratch: (Vec<GpuInstance>, Vec<DrawBatch>),
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
        let mut shadow = None;
        let mut lighting = None;
        let mut opaque = None;
        let mut sky = None;
        let mut weather = None;
        let mut water = None;
        let mut shelter = None;
        let target = result
            .and_then(|()| {
                let maps = shadow.insert(ShadowResources::new(&mut ctx)?);
                let roofs = shelter.insert(ShelterBuffer::new(&mut ctx, &ShelterMap::empty())?);
                let bindings = lighting.insert(LightingBindings::new(
                    &ctx,
                    &frames,
                    maps,
                    roofs.buffer.buffer,
                )?);
                let layout = bindings.set_layout;
                let pipeline = opaque.insert(OpaquePipeline::new(
                    &mut ctx,
                    &MaterialTable::builtin(),
                    layout,
                )?);
                sky = Some(SkyPipeline::new(&ctx.device, pipeline.layout)?);
                weather = Some(WeatherPipeline::new(&ctx.device, pipeline.layout)?);
                water = Some(WaterPipeline::new(&ctx.device, pipeline.layout)?);
                Ok(())
            })
            .and_then(|()| make_target(&mut ctx));
        match (
            target, opaque, sky, weather, water, shelter, lighting, shadow,
        ) {
            (
                Ok(target),
                Some(opaque),
                Some(sky),
                Some(weather),
                Some(water),
                Some(shelter),
                Some(lighting),
                Some(shadow),
            ) => Ok(Self {
                frames,
                target,
                deferred: DeferredQueue::new(),
                opaque,
                sky,
                weather,
                water,
                water_warned: false,
                post: None,
                upscaler: None,
                ui_pass: None,
                reset_history: false,
                frame_time: 1.0 / 60.0,
                shelter,
                thunder: Vec::new(),
                last_weather_time: None,
                shadow,
                lighting,
                shadow_camera: None,
                shadow_draws: Default::default(),
                shadow_scratch: (Vec::new(), Vec::new()),
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
            (result, opaque, sky, weather, water, shelter, lighting, shadow) => {
                if let Some(mut s) = sky {
                    s.destroy(&ctx.device);
                }
                if let Some(mut w) = weather {
                    w.destroy(&ctx.device);
                }
                if let Some(mut w) = water {
                    w.destroy(&ctx.device);
                }
                if let Some(mut s) = shelter {
                    s.destroy(&mut ctx);
                }
                if let Some(mut p) = opaque {
                    p.destroy(&mut ctx);
                }
                if let Some(mut l) = lighting {
                    l.destroy(&ctx);
                }
                if let Some(mut s) = shadow {
                    s.destroy(&mut ctx);
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

        // Opaque targets follow the internal resolution: the output size, or
        // what the upscaler asks for. The old ones may still be in use by the
        // other frame in flight.
        let display = match &self.target {
            Target::Windowed(w) => w.swapchain.as_ref().expect("checked above").extent,
            Target::Headless(o) => o.extent,
        };
        let upscaling = self.upscaler.is_some();
        let extent = match &self.upscaler {
            Some(u) => {
                let e = u.render_extent(display);
                vk::Extent2D {
                    width: e.width.clamp(1, display.width),
                    height: e.height.clamp(1, display.height),
                }
            }
            None => display,
        };
        let temporal = self.upscaler.as_ref().is_some_and(|u| u.is_temporal());
        if self.gbuffer.as_ref().is_none_or(|g| g.extent != extent) {
            let new = GBuffer::new(&mut self.ctx, extent)?;
            if let Some(old) = self.gbuffer.replace(new) {
                for image in old.into_images() {
                    self.deferred.push(self.frame_index, Garbage::Image(image));
                }
            }
        }

        // Thunder of the strikes since the previous frame.
        self.queue_thunder(world.weather.storm);

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
        self.stats = FrameStats {
            draw_calls: self.draws.len() as u32,
            instances_drawn: self.gpu_instances.len() as u32,
            instances_culled: (world.instances.len() - self.gpu_instances.len() - unknown) as u32,
        };

        // Sun shadow cascades, and the casters of each one (culled against
        // the cascade volume, so casters outside the view are kept). Their
        // instances are appended after the view's.
        // The map time sets the sun (and the sky colours) of the frame.
        // The weather tints the sky, dims the light and thickens the fog
        // first; lightning then flashes on top.
        let state = &world.weather;
        let flash = weather::lightning_flash(self.material_time, state.storm);
        let (weathered, weathered_sky) = weather::apply(&world.ambience, &world.sky, state);
        let mut resolved = sky::resolve(&weathered, &weathered_sky, world.draw_sky, camera);
        weather::apply_flash(&mut resolved, flash);
        let mut gpu_weather = GpuWeather::new(
            state,
            flash,
            resolved.gpu.horizon,
            &self.shelter.info,
            &view_proj,
        );
        // Underwater, the uniform water fog replaces the height fog (and the
        // weather's fog tint and the sky colour it would take).
        let underwater = world.underwater;
        if underwater {
            resolved.ambience = underwater_profile(&resolved.ambience);
            resolved.gpu.params[3] = 0.0;
            gpu_weather.fog_tint[3] = 0.0;
        }
        let water = world.water.as_ref().filter(|w| match w.validate() {
            Ok(()) => true,
            Err(e) => {
                if !self.water_warned {
                    tracing::warn!("water ignored: {e}");
                }
                self.water_warned = true;
                false
            }
        });
        let gpu_water = GpuWater::new(&resolved.ambience.water, water, underwater);
        let frame_clear = if underwater {
            let fog = resolved.ambience.fog.color;
            [fog[0], fog[1], fog[2], 1.0]
        } else {
            world.clear_color
        };
        let draw_sky = world.draw_sky && !underwater;
        let ambience = &resolved.ambience;
        // Post chain: parameters from the (weathered) profile plus gameplay effects.
        let post_params = world.post.unwrap_or_else(|| PostParams {
            damage_flash: world.screen.damage_flash,
            speed_lines: world.screen.speed_lines,
            ..PostParams::from_profile(ambience, underwater)
        });
        if let Some(chain) = &mut self.post {
            chain.temporal_aa = temporal;
        }
        let post_active = self
            .post
            .as_ref()
            .is_some_and(|p| p.is_active(&post_params));
        if post_active {
            let post = self.post.as_mut().expect("checked above");
            post.ensure_targets(&mut self.ctx, display)?;
            // With an upscaler the contours run before it, at the internal
            // resolution.
            if upscaling && post.contours_active(&post_params) {
                post.ensure_scene(&mut self.ctx, extent)?;
            }
        }
        let sun_dir = glam::Vec3::from_array(ambience.sun.direction).normalize_or(glam::Vec3::Y);
        let cascades = (ambience.sun.intensity > 0.0).then(|| {
            let fit_camera = self.shadow_camera.as_ref().unwrap_or(camera);
            let near = fit_camera.proj.w_axis.z;
            let near = if near > 0.0 { near } else { 0.1 };
            fit_cascades(fit_camera, sun_dir, near, SHADOW_DISTANCE)
        });
        for draws in &mut self.shadow_draws {
            draws.clear();
        }
        if let Some(cascades) = &cascades {
            for (matrix, draws) in cascades.matrices.iter().zip(&mut self.shadow_draws) {
                let (scratch, scratch_draws) = &mut self.shadow_scratch;
                cull::cull_and_batch(
                    matrix,
                    matrix,
                    &world.instances,
                    meshes,
                    scratch,
                    scratch_draws,
                );
                let base = self.gpu_instances.len() as u32;
                draws.extend(scratch_draws.iter().map(|d| DrawBatch {
                    first_instance: d.first_instance + base,
                    ..*d
                }));
                self.gpu_instances.extend_from_slice(scratch);
            }
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
        let jitter = jitter_ndc(camera.jitter_px, (extent.width, extent.height));
        let near_plane = camera.proj.w_axis.z;
        let near_plane = if near_plane > 0.0 { near_plane } else { 0.1 };
        let reset = std::mem::take(&mut self.reset_history);
        let frame_time = self.frame_time;

        // Lighting block, written into this frame's uniform ring.
        let camera_pos = camera.view.inverse().w_axis.truncate();
        let lighting_block = pack_lighting(
            ambience,
            &world.lights,
            camera_pos,
            cascades.as_ref(),
            resolved.gpu,
            gpu_weather,
            gpu_water,
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
        let (image, image_index, target_handle) = match &mut self.target {
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
                let handle = ImageHandle {
                    image: sc.images[index as usize],
                    view: sc.views[index as usize],
                    extent: sc.extent,
                    format: sc.format,
                };
                (handle.image, Some(index), handle)
            }
            Target::Headless(o) => {
                let handle = ImageHandle {
                    image: o.image.image,
                    view: o.image.view,
                    extent: o.extent,
                    format: target::OFFSCREEN_FORMAT,
                };
                (handle.image, None, handle)
            }
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
            attachment(gbuffer.reactive.image),
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
            (gbuffer.hdr.view, clear_color(frame_clear)),
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
        // With the post chain or an upscaler the HDR buffer (and the depth)
        // are sampled first; the final result is moved to TRANSFER_SRC before
        // the blit.
        let sampled = post_active || upscaling;
        let mut end_barriers = vec![
            if sampled {
                shader_read(gbuffer.hdr.image)
            } else {
                ImageBarrier {
                    image: gbuffer.hdr.image,
                    aspect: color,
                    old: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                    new: vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                    src_access: vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
                    dst_access: vk::AccessFlags::TRANSFER_READ,
                }
            },
            shader_read(gbuffer.motion.image),
            shader_read(gbuffer.normal.image),
            shader_read(gbuffer.reactive.image),
            ImageBarrier {
                image,
                aspect: color,
                old: vk::ImageLayout::UNDEFINED,
                new: vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                src_access: vk::AccessFlags::empty(),
                dst_access: vk::AccessFlags::TRANSFER_WRITE,
            },
        ];
        if sampled {
            end_barriers.push(ImageBarrier {
                image: gbuffer.depth.image,
                aspect: vk::ImageAspectFlags::DEPTH,
                old: vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL,
                new: vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL,
                src_access: vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE,
                dst_access: vk::AccessFlags::SHADER_READ,
            });
        }
        let corner = vk::Offset3D {
            x: display.width as i32,
            y: display.height as i32,
            z: 1,
        };
        let layers = vk::ImageSubresourceLayers {
            aspect_mask: color,
            mip_level: 0,
            base_array_layer: 0,
            layer_count: 1,
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
        let has_ui = self.ui_pass.is_some();
        let final_barrier = [ImageBarrier {
            image,
            aspect: color,
            old: if has_ui {
                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL
            } else {
                vk::ImageLayout::TRANSFER_DST_OPTIMAL
            },
            new: final_layout,
            src_access: if has_ui {
                vk::AccessFlags::COLOR_ATTACHMENT_WRITE
            } else {
                vk::AccessFlags::TRANSFER_WRITE
            },
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
            // Pass 1: sun shadow cascades. The maps are shared by both
            // frames in flight; the first barrier orders these writes after
            // the previous frame's reads (same queue, submission order).
            self.shadow.barrier(
                device,
                frame.cmd,
                vk::PipelineStageFlags::FRAGMENT_SHADER
                    | vk::PipelineStageFlags::LATE_FRAGMENT_TESTS,
                vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS
                    | vk::PipelineStageFlags::LATE_FRAGMENT_TESTS,
                vk::ImageLayout::UNDEFINED,
                vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL,
                vk::AccessFlags::SHADER_READ | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE,
                vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE,
            );
            if cascades.is_some() {
                let map_area = vk::Rect2D {
                    offset: vk::Offset2D::default(),
                    extent: vk::Extent2D {
                        width: SHADOW_MAP_SIZE,
                        height: SHADOW_MAP_SIZE,
                    },
                };
                let map_viewport = vk::Viewport {
                    x: 0.0,
                    y: 0.0,
                    width: SHADOW_MAP_SIZE as f32,
                    height: SHADOW_MAP_SIZE as f32,
                    min_depth: 0.0,
                    max_depth: 1.0,
                };
                for (view, draws) in self.shadow.layer_views.iter().zip(&self.shadow_draws) {
                    let depth = vk::RenderingAttachmentInfo::default()
                        .image_view(*view)
                        .image_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL)
                        .load_op(vk::AttachmentLoadOp::CLEAR)
                        .store_op(vk::AttachmentStoreOp::STORE)
                        .clear_value(vk::ClearValue {
                            depth_stencil: vk::ClearDepthStencilValue {
                                depth: 0.0,
                                stencil: 0,
                            },
                        });
                    let pass = vk::RenderingInfo::default()
                        .render_area(map_area)
                        .layer_count(1)
                        .depth_attachment(&depth);
                    self.ctx
                        .dynamic_rendering
                        .cmd_begin_rendering(frame.cmd, &pass);
                    if let (Some(arena), Some(instances), false) =
                        (arena, frame.instances.as_ref(), draws.is_empty())
                    {
                        device.cmd_bind_pipeline(
                            frame.cmd,
                            vk::PipelineBindPoint::GRAPHICS,
                            self.shadow.pipeline,
                        );
                        device.cmd_set_viewport(frame.cmd, 0, &[map_viewport]);
                        device.cmd_set_scissor(frame.cmd, 0, &[map_area]);
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
                        for d in draws {
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
                }
            }
            self.shadow.barrier(
                device,
                frame.cmd,
                vk::PipelineStageFlags::LATE_FRAGMENT_TESTS,
                vk::PipelineStageFlags::FRAGMENT_SHADER,
                vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL,
                vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL,
                vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE,
                vk::AccessFlags::SHADER_READ,
            );
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
            if draw_sky {
                // Sky: a full-screen triangle at depth 0, so only pixels no
                // mesh covered pass the depth test. Same layout, sets and
                // push constants as the opaque draws above.
                device.cmd_bind_pipeline(
                    frame.cmd,
                    vk::PipelineBindPoint::GRAPHICS,
                    self.sky.pipeline,
                );
                device.cmd_draw(frame.cmd, 3, 1, 0, 0);
            }
            self.ctx.dynamic_rendering.cmd_end_rendering(frame.cmd);
            // Water and weather, in a second rendering over the same targets
            // plus the reactive mask. The water surface writes colour, depth,
            // motion and normals; weather: instanced quads per particle
            // kind, alpha-blended into the HDR target (and the reactive
            // mask) and depth-tested against the scene; the push constant
            // `kind` picks the shape.
            let counts = [
                weather::particle_count(weather::KIND_RAIN, state.rain),
                weather::particle_count(weather::KIND_SNOW, state.snow),
                weather::particle_count(weather::KIND_SAND, state.sand),
                if underwater { water::MOTE_COUNT } else { 0 },
            ];
            // The water surface and the particles share this rendering. An
            // upscaler reads the reactive mask it clears.
            if water.is_some() || counts.iter().any(|&c| c > 0) || upscaling {
                // The first rendering's colour and depth writes before this
                // one's blending and depth reads.
                device.cmd_pipeline_barrier(
                    frame.cmd,
                    vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                        | vk::PipelineStageFlags::LATE_FRAGMENT_TESTS,
                    vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                        | vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS,
                    vk::DependencyFlags::empty(),
                    &[vk::MemoryBarrier::default()
                        .src_access_mask(
                            vk::AccessFlags::COLOR_ATTACHMENT_WRITE
                                | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE,
                        )
                        .dst_access_mask(
                            vk::AccessFlags::COLOR_ATTACHMENT_READ
                                | vk::AccessFlags::COLOR_ATTACHMENT_WRITE
                                | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_READ,
                        )],
                    &[],
                    &[],
                );
                let load = |view| {
                    vk::RenderingAttachmentInfo::default()
                        .image_view(view)
                        .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                        .load_op(vk::AttachmentLoadOp::LOAD)
                        .store_op(vk::AttachmentStoreOp::STORE)
                };
                let hdr_load = [
                    load(gbuffer.hdr.view),
                    load(gbuffer.motion.view),
                    load(gbuffer.normal.view),
                    vk::RenderingAttachmentInfo::default()
                        .image_view(gbuffer.reactive.view)
                        .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                        .load_op(vk::AttachmentLoadOp::CLEAR)
                        .store_op(vk::AttachmentStoreOp::STORE)
                        .clear_value(clear_color([0.0; 4])),
                ];
                let depth_load = vk::RenderingAttachmentInfo::default()
                    .image_view(gbuffer.depth.view)
                    .image_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL)
                    .load_op(vk::AttachmentLoadOp::LOAD)
                    .store_op(vk::AttachmentStoreOp::STORE);
                let weather_pass = vk::RenderingInfo::default()
                    .render_area(full)
                    .layer_count(1)
                    .color_attachments(&hdr_load)
                    .depth_attachment(&depth_load);
                self.ctx
                    .dynamic_rendering
                    .cmd_begin_rendering(frame.cmd, &weather_pass);
                device.cmd_set_viewport(frame.cmd, 0, &[viewport]);
                device.cmd_set_scissor(frame.cmd, 0, &[full]);
                device.cmd_bind_descriptor_sets(
                    frame.cmd,
                    vk::PipelineBindPoint::GRAPHICS,
                    self.opaque.layout,
                    0,
                    &[self.opaque.materials.set, lighting_set],
                    &[lighting_offset as u32],
                );
                if water.is_some() {
                    let push: [u8; opaque::PUSH_CONSTANT_SIZE as usize] =
                        bytemuck::cast([jitter.x, jitter.y, self.material_time, 0.0]);
                    device.cmd_bind_pipeline(
                        frame.cmd,
                        vk::PipelineBindPoint::GRAPHICS,
                        self.water.pipeline,
                    );
                    device.cmd_push_constants(
                        frame.cmd,
                        self.opaque.layout,
                        opaque::PUSH_CONSTANT_STAGES,
                        0,
                        &push,
                    );
                    device.cmd_draw(
                        frame.cmd,
                        water::GRID_CELLS * water::GRID_CELLS * 6,
                        1,
                        0,
                        0,
                    );
                }
                device.cmd_bind_pipeline(
                    frame.cmd,
                    vk::PipelineBindPoint::GRAPHICS,
                    self.weather.pipeline,
                );
                for (kind, count) in (0u32..).zip(counts) {
                    if count == 0 {
                        continue;
                    }
                    let push: [u8; opaque::PUSH_CONSTANT_SIZE as usize] =
                        bytemuck::cast([jitter.x, jitter.y, self.material_time, kind as f32]);
                    device.cmd_push_constants(
                        frame.cmd,
                        self.opaque.layout,
                        opaque::PUSH_CONSTANT_STAGES,
                        0,
                        &push,
                    );
                    device.cmd_draw(frame.cmd, 6, count, 0, 0);
                }
                self.ctx.dynamic_rendering.cmd_end_rendering(frame.cmd);
            }
            image_barriers(
                device,
                frame.cmd,
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                    | vk::PipelineStageFlags::LATE_FRAGMENT_TESTS
                    | vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::TRANSFER | vk::PipelineStageFlags::FRAGMENT_SHADER,
                &end_barriers,
            );
            let mut blit_source = gbuffer.hdr.image;
            // Normally the display size: the post chain, an upscaler, or the
            // scene itself when it is drawn at that size.
            let mut blit_extent = extent;
            let mut chain_input = PostInputs {
                hdr: gbuffer.hdr.image,
                hdr_view: gbuffer.hdr.view,
                depth_view: gbuffer.depth.view,
                normal_view: gbuffer.normal.view,
            };
            // Contours already drawn at the internal resolution.
            let mut contours_done = false;
            if let Some(upscaler) = self.upscaler.as_mut() {
                let handle = |image: &frame::OwnedImage, format| ImageHandle {
                    image: image.image,
                    view: image.view,
                    extent,
                    format,
                };
                let mut colour = handle(&gbuffer.hdr, opaque::HDR_FORMAT);
                if post_active {
                    let chain = self.post.as_ref().expect("checked above");
                    if let Some((image, view)) = chain.record_scene_contours(
                        &self.ctx,
                        frame.cmd,
                        slot,
                        &chain_input,
                        &post_params,
                        near_plane,
                    )? {
                        colour = ImageHandle {
                            image,
                            view,
                            ..colour
                        };
                        contours_done = true;
                    }
                }
                let inputs = UpscaleInputs {
                    colour,
                    depth: handle(&gbuffer.depth, opaque::DEPTH_FORMAT),
                    motion: handle(&gbuffer.motion, opaque::MOTION_FORMAT),
                    reactive: Some(handle(&gbuffer.reactive, opaque::REACTIVE_FORMAT)),
                    display,
                    jitter: camera.jitter_px,
                    exposure: if post_active {
                        post_params.exposure
                    } else {
                        1.0
                    },
                    frame_time,
                    near: near_plane,
                    far: f32::INFINITY,
                    fov_y: fov_y(&camera.proj),
                    reset,
                };
                let mut frame_ctx = FrameCtx {
                    device,
                    cmd: frame.cmd,
                    slot,
                    frames_in_flight: FRAMES_IN_FLIGHT,
                    frame_index: self.frame_index,
                };
                let out = upscaler.execute(&mut frame_ctx, &inputs);
                chain_input.hdr = out.image;
                chain_input.hdr_view = out.view;
                blit_source = out.image;
                blit_extent = out.extent;
            }
            if post_active {
                let chain = self.post.as_ref().expect("checked above");
                blit_source = chain.record(
                    &self.ctx,
                    frame.cmd,
                    slot,
                    &chain_input,
                    &post_params,
                    near_plane,
                    self.material_time,
                    contours_done,
                )?;
                blit_extent = display;
            }
            if sampled {
                image_barriers(
                    device,
                    frame.cmd,
                    vk::PipelineStageFlags::ALL_COMMANDS,
                    vk::PipelineStageFlags::TRANSFER,
                    &[ImageBarrier {
                        image: blit_source,
                        aspect: color,
                        old: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                        new: vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                        src_access: vk::AccessFlags::MEMORY_WRITE,
                        dst_access: vk::AccessFlags::TRANSFER_READ,
                    }],
                );
            }
            let blit = vk::ImageBlit {
                src_subresource: layers,
                src_offsets: [
                    vk::Offset3D::default(),
                    vk::Offset3D {
                        x: blit_extent.width as i32,
                        y: blit_extent.height as i32,
                        z: 1,
                    },
                ],
                dst_subresource: layers,
                dst_offsets: [vk::Offset3D::default(), corner],
            };
            let filter = if blit_extent == display {
                vk::Filter::NEAREST
            } else {
                vk::Filter::LINEAR
            };
            device.cmd_blit_image(
                frame.cmd,
                blit_source,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[blit],
                filter,
            );
            let mut final_src_stage = vk::PipelineStageFlags::TRANSFER;
            if let Some(ui) = self.ui_pass.as_mut() {
                // Pass 9: the interface, drawn at the display resolution
                // straight onto the target, after the blit: the render scale
                // and the post chain never touch it.
                image_barriers(
                    device,
                    frame.cmd,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                    &[ImageBarrier {
                        image,
                        aspect: color,
                        old: vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                        new: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                        src_access: vk::AccessFlags::TRANSFER_WRITE,
                        dst_access: vk::AccessFlags::COLOR_ATTACHMENT_READ
                            | vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
                    }],
                );
                let mut frame_ctx = FrameCtx {
                    device,
                    cmd: frame.cmd,
                    slot,
                    frames_in_flight: FRAMES_IN_FLIGHT,
                    frame_index: self.frame_index,
                };
                ui.execute(&mut frame_ctx, &target_handle);
                final_src_stage = vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT;
            }
            image_barriers(
                device,
                frame.cmd,
                final_src_stage,
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

    /// Turns the post-processing chain on with `settings`, or off with `None`
    /// (the default: the HDR buffer is copied to the target untouched, so
    /// colours stay linear and un-tonemapped). The chain is built at the first
    /// call; later calls only change which effects run.
    pub fn set_post_settings(&mut self, settings: Option<PostSettings>) -> Result<(), RenderError> {
        match (settings, &mut self.post) {
            (None, slot) => {
                if let Some(mut chain) = slot.take() {
                    // SAFETY: valid device; makes the chain unused.
                    unsafe { self.ctx.device.device_wait_idle() }?;
                    chain.destroy(&mut self.ctx);
                }
            }
            (Some(s), Some(chain)) => chain.settings = s,
            (Some(s), slot @ None) => *slot = Some(PostChain::new(&self.ctx, s)?),
        }
        Ok(())
    }

    /// The Vulkan objects an upscaler needs to create its own resources (see
    /// [`UpscalePass`]). Drop the upscaler before the renderer, or hand it to
    /// [`Renderer::set_upscaler`], which does.
    pub fn gpu_handles(&self) -> GpuHandles {
        GpuHandles {
            instance: self.ctx.instance.clone(),
            device: self.ctx.device.clone(),
            physical: self.ctx.physical,
            queue_family: self.ctx.queue_family,
            extensions: self.ctx.extensions,
        }
    }

    /// The optional device extensions (`VK_NVX_binary_import`,
    /// `VK_NVX_image_view_handle`, `VK_KHR_push_descriptor`) the device was
    /// created with: those the GPU offers, none being required.
    pub fn device_extensions(&self) -> DeviceExtensions {
        self.ctx.extensions
    }

    /// Installs the pass between the scene and the post chain, or removes it
    /// (`None`, the default). The scene is then drawn at the pass's
    /// [`UpscalePass::render_extent`]: the motion vectors, depth and
    /// reactive mask it reads have that size, and the post chain, the
    /// interface and the target have the display size. The previous pass is
    /// dropped after the GPU went idle.
    pub fn set_upscaler(&mut self, pass: Option<Box<dyn UpscalePass>>) -> Result<(), RenderError> {
        // SAFETY: valid device; makes the old pass's resources unused.
        unsafe { self.ctx.device.device_wait_idle() }?;
        self.upscaler = pass;
        self.reset_history = true;
        Ok(())
    }

    /// Installs pass 9, the interface, or removes it (`None`, the default).
    /// It is recorded after the post chain, straight onto the display
    /// resolution target, so the render scale does not affect it (see
    /// [`UiPass`]). The previous pass is dropped after the GPU went idle.
    pub fn set_ui_pass(&mut self, pass: Option<Box<dyn UiPass>>) -> Result<(), RenderError> {
        // SAFETY: valid device; makes the old pass's resources unused.
        unsafe { self.ctx.device.device_wait_idle() }?;
        self.ui_pass = pass;
        Ok(())
    }

    /// True while an upscaler is installed.
    pub fn has_upscaler(&self) -> bool {
        self.upscaler.is_some()
    }

    /// The jitter the installed upscaler wants for `frame_index`, in internal
    /// pixels; set it as [`Camera::jitter_px`]. Zero without an upscaler.
    pub fn upscaler_jitter(&self, frame_index: u32) -> glam::Vec2 {
        self.upscaler
            .as_ref()
            .map_or(glam::Vec2::ZERO, |u| u.jitter(frame_index))
    }

    /// The internal resolution the scene is drawn at for a display of the
    /// given size (the display size itself without an upscaler).
    pub fn internal_extent(&self, display: (u32, u32)) -> (u32, u32) {
        match &self.upscaler {
            Some(u) => {
                let e = u.render_extent(vk::Extent2D {
                    width: display.0,
                    height: display.1,
                });
                (
                    e.width.clamp(1, display.0.max(1)),
                    e.height.clamp(1, display.1.max(1)),
                )
            }
            None => display,
        }
    }

    /// Makes the next frame discard the upscaler's temporal history: call it
    /// on respawn, teleport and camera cuts.
    pub fn reset_temporal_history(&mut self) {
        self.reset_history = true;
    }

    /// Seconds between frames, passed to the upscaler (1/60 s by default).
    pub fn set_frame_time(&mut self, seconds: f32) {
        if seconds.is_finite() && seconds > 0.0 {
            self.frame_time = seconds;
        }
    }

    /// Sets the time, in seconds, that animates water, lava, emissive and
    /// hologram materials from the next frame on (0 at creation).
    pub fn set_material_time(&mut self, seconds: f32) {
        self.material_time = seconds;
    }

    /// Fits the sun shadow cascades to `camera` instead of the view camera
    /// (`None` goes back to the view camera). Lets tools and tests move the
    /// cascades while the image stays put, e.g. to check that shadow edges
    /// do not shimmer.
    pub fn set_shadow_camera(&mut self, camera: Option<Camera>) {
        self.shadow_camera = camera;
    }

    /// Loads the roofs that shelter the map from weather particles (an empty
    /// map at creation). Call it once at map load: it waits for the GPU to go
    /// idle.
    pub fn set_shelter_map(&mut self, map: &ShelterMap) -> Result<(), RenderError> {
        let new = ShelterBuffer::new(&mut self.ctx, map)?;
        // SAFETY: valid device; once idle no frame reads the old buffer or
        // the descriptor sets being updated.
        if let Err(e) = unsafe { self.ctx.device.device_wait_idle() } {
            let mut new = new;
            new.destroy(&mut self.ctx);
            return Err(e.into());
        }
        self.lighting.set_shelter(&self.ctx, new.buffer.buffer);
        let mut old = std::mem::replace(&mut self.shelter, new);
        old.destroy(&mut self.ctx);
        Ok(())
    }

    /// Takes the thunder of the lightning strikes drawn since the last call,
    /// oldest first. The audio lot plays each one `delay_s` seconds later.
    pub fn take_thunder_events(&mut self) -> Vec<ThunderEvent> {
        std::mem::take(&mut self.thunder)
    }

    /// Queues the thunder of the strikes between the previous frame's
    /// material time and this one's. Time going backwards or jumping by more
    /// than 30 s (a reset or a teleport) skips detection for that frame.
    fn queue_thunder(&mut self, storm: f32) {
        let now = self.material_time;
        if let Some(previous) = self.last_weather_time.replace(now) {
            if now > previous && now - previous <= 30.0 {
                self.thunder.extend(
                    weather::strikes_between(previous, now, storm)
                        .iter()
                        .map(weather::Strike::thunder),
                );
                // Nobody is listening: keep the newest ones only.
                let excess = self.thunder.len().saturating_sub(64);
                self.thunder.drain(..excess);
            }
        }
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

/// Vertical field of view of a projection built by [`perspective_reverse_z`].
fn fov_y(proj: &glam::Mat4) -> f32 {
    let f = proj.y_axis.y.abs();
    if f > 0.0 {
        2.0 * (1.0 / f).atan()
    } else {
        0.0
    }
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
        // The upscaler frees its own Vulkan objects, so before the device.
        self.upscaler = None;
        self.ui_pass = None;
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
        self.sky.destroy(&self.ctx.device);
        self.weather.destroy(&self.ctx.device);
        if let Some(mut p) = self.post.take() {
            p.destroy(&mut self.ctx);
        }
        self.water.destroy(&self.ctx.device);
        self.shelter.destroy(&mut self.ctx);
        self.opaque.destroy(&mut self.ctx);
        self.lighting.destroy(&self.ctx);
        self.shadow.destroy(&mut self.ctx);
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

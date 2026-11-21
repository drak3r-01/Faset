//! Visual weather: the weather vector and its timed transitions, the
//! stateless particle model, lightning with thunder delay, and the way the
//! weather tints the sky, the light and the fog.
//!
//! The particles themselves are drawn by `faset-shaders/src/weather.wgsl`
//! (see [`WeatherPipeline`]); the functions here mirror its formulas and are
//! what the unit tests check. Weather is visual (and, through
//! [`ThunderEvent`], audible) only: it has no effect on physics.

use ash::vk;
use bytemuck::{Pod, Zeroable};
use faset_core::glam::Vec3;
use serde::{Deserialize, Serialize};

use crate::ambience::AmbienceProfile;
use crate::opaque::{DEPTH_FORMAT, HDR_FORMAT};
use crate::shelter::ShelterInfo;
use crate::sky::{hash_u32, Resolved, SkyParams};
use crate::RenderError;

/// Speed of sound in air, in metres per second.
pub const SPEED_OF_SOUND: f32 = 343.0;
/// Shortest transition between two weather keyframes, in seconds.
pub const MIN_TRANSITION_S: f32 = 10.0;
/// Longest transition between two weather keyframes, in seconds.
pub const MAX_TRANSITION_S: f32 = 30.0;
/// Strongest wind a [`WeatherState`] accepts, in metres per second.
/// Decision: 60 m/s (a violent storm) as the upper bound.
pub const MAX_WIND_SPEED: f32 = 60.0;

/// The weather at one instant: a vector of intensities in [0, 1] and a wind.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct WeatherState {
    pub rain: f32,
    pub snow: f32,
    pub sand: f32,
    pub fog: f32,
    /// Wind velocity in metres per second (world axes, Y up).
    pub wind: Vec3,
    /// Surface wetness: darkens surfaces, adds reflections and puddles.
    pub humidity: f32,
    /// Storm strength: overcast sky and lightning.
    pub storm: f32,
}

/// Why a [`WeatherState`] or [`WeatherSchedule`] is rejected.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum WeatherError {
    #[error("weather `{field}` must be finite and in [0, 1], got {value}")]
    OutOfRange { field: &'static str, value: f32 },
    #[error("wind must be finite with a speed of at most {MAX_WIND_SPEED} m/s")]
    InvalidWind,
    #[error(
        "transition {index} lasts {duration} s, outside {MIN_TRANSITION_S} to {MAX_TRANSITION_S} s"
    )]
    InvalidDuration { index: usize, duration: f32 },
    #[error("transition {index} starts at {start} s, before the previous one ends")]
    Overlap { index: usize, start: f32 },
}

impl WeatherState {
    /// Clear weather: every intensity 0, no wind.
    pub const CLEAR: Self = Self {
        rain: 0.0,
        snow: 0.0,
        sand: 0.0,
        fog: 0.0,
        wind: Vec3::ZERO,
        humidity: 0.0,
        storm: 0.0,
    };

    fn scalars(&self) -> [(&'static str, f32); 6] {
        [
            ("rain", self.rain),
            ("snow", self.snow),
            ("sand", self.sand),
            ("fog", self.fog),
            ("humidity", self.humidity),
            ("storm", self.storm),
        ]
    }

    /// Checks that every intensity is in [0, 1] and the wind is finite and
    /// at most [`MAX_WIND_SPEED`].
    pub fn validate(&self) -> Result<(), WeatherError> {
        for (field, value) in self.scalars() {
            if !(0.0..=1.0).contains(&value) {
                return Err(WeatherError::OutOfRange { field, value });
            }
        }
        if !self.wind.is_finite() || self.wind.length() > MAX_WIND_SPEED {
            return Err(WeatherError::InvalidWind);
        }
        Ok(())
    }

    /// Component-wise interpolation: `self` at `f = 0`, `other` at `f = 1`.
    pub fn lerp(&self, other: &Self, f: f32) -> Self {
        let mix = |a: f32, b: f32| a + (b - a) * f;
        Self {
            rain: mix(self.rain, other.rain),
            snow: mix(self.snow, other.snow),
            sand: mix(self.sand, other.sand),
            fog: mix(self.fog, other.fog),
            wind: self.wind.lerp(other.wind, f),
            humidity: mix(self.humidity, other.humidity),
            storm: mix(self.storm, other.storm),
        }
    }

    /// True when nothing is falling and nothing is wet or foggy.
    pub fn is_clear(&self) -> bool {
        self.rain <= 0.0
            && self.snow <= 0.0
            && self.sand <= 0.0
            && self.fog <= 0.0
            && self.humidity <= 0.0
            && self.storm <= 0.0
    }
}

/// A timed transition: the weather moves linearly from the previous target
/// to `target` over `duration_s` seconds, starting at `start_s`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WeatherKeyframe {
    pub start_s: f32,
    pub duration_s: f32,
    pub target: WeatherState,
}

/// Plain data of a [`WeatherSchedule`], validated on deserialisation.
#[derive(Deserialize)]
struct ScheduleData {
    initial: WeatherState,
    transitions: Vec<WeatherKeyframe>,
}

/// The weather over time: an initial state and transitions to new ones.
///
/// Decision: this is the local input of the renderer; the network delivery of
/// schedules is a later lot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "ScheduleData")]
pub struct WeatherSchedule {
    initial: WeatherState,
    transitions: Vec<WeatherKeyframe>,
}

impl TryFrom<ScheduleData> for WeatherSchedule {
    type Error = WeatherError;

    fn try_from(data: ScheduleData) -> Result<Self, WeatherError> {
        Self::new(data.initial, data.transitions)
    }
}

impl WeatherSchedule {
    /// Builds a schedule. Transitions must be in time order without overlap,
    /// last [`MIN_TRANSITION_S`] to [`MAX_TRANSITION_S`] seconds, and every
    /// state must be valid.
    pub fn new(
        initial: WeatherState,
        transitions: Vec<WeatherKeyframe>,
    ) -> Result<Self, WeatherError> {
        initial.validate()?;
        let mut free_from = f32::NEG_INFINITY;
        for (index, k) in transitions.iter().enumerate() {
            k.target.validate()?;
            if !(MIN_TRANSITION_S..=MAX_TRANSITION_S).contains(&k.duration_s) {
                return Err(WeatherError::InvalidDuration {
                    index,
                    duration: k.duration_s,
                });
            }
            if !k.start_s.is_finite() || k.start_s < free_from {
                return Err(WeatherError::Overlap {
                    index,
                    start: k.start_s,
                });
            }
            free_from = k.start_s + k.duration_s;
        }
        Ok(Self {
            initial,
            transitions,
        })
    }

    /// A schedule that never changes.
    pub fn constant(state: WeatherState) -> Result<Self, WeatherError> {
        Self::new(state, Vec::new())
    }

    /// The weather at time `t` seconds: the initial state before the first
    /// transition, a linear blend during one, the target after it.
    pub fn sample(&self, t: f32) -> WeatherState {
        let mut from = self.initial;
        if t.is_nan() {
            return from;
        }
        for k in &self.transitions {
            if t < k.start_s {
                break;
            }
            let end = k.start_s + k.duration_s;
            if t < end {
                return from.lerp(&k.target, (t - k.start_s) / k.duration_s);
            }
            from = k.target;
        }
        from
    }

    pub fn initial(&self) -> &WeatherState {
        &self.initial
    }

    pub fn transitions(&self) -> &[WeatherKeyframe] {
        &self.transitions
    }
}

/// Thunder the audio lot plays: the sound reaches the listener `delay_s`
/// seconds after the flash.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ThunderEvent {
    pub delay_s: f32,
}

/// Seconds the sound of thunder takes to travel `distance_m` metres.
pub fn thunder_delay(distance_m: f32) -> f32 {
    distance_m.max(0.0) / SPEED_OF_SOUND
}

/// Length of the time slot in which at most one lightning strike happens.
pub const LIGHTNING_SLOT_S: f32 = 7.0;
/// Seconds a strike's flash lasts.
pub const FLASH_DURATION_S: f32 = 0.6;
/// Storm strength below which there is no lightning.
pub const LIGHTNING_MIN_STORM: f32 = 0.35;

/// One lightning strike.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Strike {
    /// Time of the flash, in seconds.
    pub start_s: f32,
    /// Distance from the listener, in metres.
    pub distance_m: f32,
}

impl Strike {
    /// The thunder this strike makes.
    pub fn thunder(&self) -> ThunderEvent {
        ThunderEvent {
            delay_s: thunder_delay(self.distance_m),
        }
    }
}

fn unit_hash(a: u32, b: u32) -> f32 {
    let h = hash_u32(a ^ hash_u32(b.wrapping_add(0x68bc_21eb)));
    (h >> 8) as f32 * (1.0 / 16_777_215.0)
}

/// The strike of time slot `slot` (`[slot, slot + 1) * LIGHTNING_SLOT_S`),
/// if the storm is strong enough to make one: the chance grows from 0 at
/// [`LIGHTNING_MIN_STORM`] to 0.85 at full storm. Which slots strike, when
/// and how far away is a hash of the slot number, so every client sees the
/// same lightning.
pub fn strike_in_slot(slot: i64, storm: f32) -> Option<Strike> {
    if storm < LIGHTNING_MIN_STORM {
        return None;
    }
    let chance = 0.85 * (storm - LIGHTNING_MIN_STORM) / (1.0 - LIGHTNING_MIN_STORM);
    let key = slot as u32;
    if unit_hash(key, 11) >= chance {
        return None;
    }
    Some(Strike {
        start_s: (slot as f32 + 0.8 * unit_hash(key, 12)) * LIGHTNING_SLOT_S,
        distance_m: 300.0 + 3700.0 * unit_hash(key, 13),
    })
}

/// Strikes whose flash starts in `(from_s, to_s]`.
pub fn strikes_between(from_s: f32, to_s: f32, storm: f32) -> Vec<Strike> {
    if !(from_s.is_finite() && to_s.is_finite() && to_s > from_s) {
        return Vec::new();
    }
    let first = (from_s / LIGHTNING_SLOT_S).floor() as i64;
    let last = (to_s / LIGHTNING_SLOT_S).floor() as i64;
    (first..=last)
        .filter_map(|slot| strike_in_slot(slot, storm))
        .filter(|s| s.start_s > from_s && s.start_s <= to_s)
        .collect()
}

/// Brightness of the flash `age_s` seconds after a strike, in [0, 1]: a sharp
/// main stroke and a weaker second one.
pub fn flash_envelope(age_s: f32) -> f32 {
    if !(0.0..FLASH_DURATION_S).contains(&age_s) {
        return 0.0;
    }
    let pulse = |a: f32| if a < 0.0 { 0.0 } else { (-28.0 * a).exp() };
    pulse(age_s).max(0.75 * pulse(age_s - 0.15))
}

/// Light boost of the lightning at time `time_s`, in [0, 1].
pub fn lightning_flash(time_s: f32, storm: f32) -> f32 {
    if storm < LIGHTNING_MIN_STORM || !time_s.is_finite() {
        return 0.0;
    }
    let slot = (time_s / LIGHTNING_SLOT_S).floor() as i64;
    [slot - 1, slot]
        .into_iter()
        .filter_map(|s| strike_in_slot(s, storm))
        .map(|s| flash_envelope(time_s - s.start_s))
        .fold(0.0, f32::max)
}

// ---------------------------------------------------------------------------
// Stateless particles
// ---------------------------------------------------------------------------

/// Kind of particle; also the push constant of the weather draw.
pub const KIND_RAIN: u32 = 0;
pub const KIND_SNOW: u32 = 1;
pub const KIND_SAND: u32 = 2;

/// Particles drawn at intensity 1, per kind.
pub const MAX_PARTICLES: [u32; 3] = [16_000, 10_000, 14_000];
/// Side of the camera-following particle box, in metres, per kind.
pub const BOX_SIZE: [f32; 3] = [24.0, 18.0, 24.0];

/// Particles to draw for `kind` at `intensity` (0 draws nothing).
pub fn particle_count(kind: u32, intensity: f32) -> u32 {
    let max = MAX_PARTICLES[(kind as usize).min(2)];
    if intensity.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
        return 0;
    }
    ((max as f32 * intensity.min(1.0)).ceil() as u32).min(max)
}

/// Velocity of a particle of `kind` in the given wind, in metres per second.
/// Rain falls fast and slants with the wind, snow drifts slowly, sand is
/// blown nearly horizontally (at least 6 m/s along the wind, or along +X in a
/// calm).
pub fn particle_velocity(kind: u32, wind: Vec3) -> Vec3 {
    match kind {
        KIND_RAIN => Vec3::new(wind.x, -9.0 + wind.y, wind.z),
        KIND_SNOW => Vec3::new(0.6 * wind.x, -1.3 + 0.6 * wind.y, 0.6 * wind.z),
        _ => {
            let flat = Vec3::new(wind.x, 0.0, wind.z);
            let dir = flat.normalize_or(Vec3::X);
            let speed = flat.length().max(6.0);
            dir * speed + Vec3::new(0.0, -0.4 + wind.y, 0.0)
        }
    }
}

/// Per-particle hash in [0, 1] of particle `index` along `axis` (same bits as
/// `particle_hash` in `weather.wgsl`).
pub fn particle_hash(index: u32, axis: u32) -> f32 {
    unit_hash(index, axis)
}

/// World position of particle `index` at time `t`, from nothing but its
/// index, the velocity, the box size `b` and the camera position `cam`:
///
/// `p = fract(h + v t / B - o / B) B + o`, with `o = cam - B / 2`,
///
/// where `h` is the per-particle hash. The particle slides along `v`, wraps
/// inside a cube of side `B` centred on the camera, and its position modulo
/// `B` does not depend on the camera, so the field does not swim when the
/// camera moves (the plan's `O_cam` offset is `o`).
pub fn particle_position(index: u32, t: f32, velocity: Vec3, b: f32, cam: Vec3) -> Vec3 {
    let h = Vec3::new(
        particle_hash(index, 0),
        particle_hash(index, 1),
        particle_hash(index, 2),
    );
    let o = cam - Vec3::splat(0.5 * b);
    let u = h + velocity * (t / b) - o / b;
    let fract = u - u.floor();
    fract * b + o
}

/// Lateral sway of a snowflake at time `t`, in metres, added to its fall
/// (a sinusoid of the time with a per-particle phase and frequency).
pub fn snow_sway(index: u32, t: f32) -> Vec3 {
    let tau = std::f32::consts::TAU;
    let (a, b, c) = (
        particle_hash(index, 3),
        particle_hash(index, 4),
        particle_hash(index, 5),
    );
    let freq = 0.8 + 1.2 * c;
    Vec3::new(
        SNOW_SWAY * (t * freq + tau * a).sin(),
        0.0,
        SNOW_SWAY * (t * (freq * 0.83) + tau * b).cos(),
    )
}

/// Amplitude of the snow sway, in metres.
pub const SNOW_SWAY: f32 = 0.35;

// ---------------------------------------------------------------------------
// Effect on the sky, light and fog
// ---------------------------------------------------------------------------

fn luminance(c: Vec3) -> f32 {
    c.dot(Vec3::new(0.2126, 0.7152, 0.0722))
}

/// Sand fog colour at full brightness.
const SAND_TINT: Vec3 = Vec3::new(0.72, 0.55, 0.34);
/// Rain, snow and fog colour at full brightness.
const GREY_TINT: Vec3 = Vec3::new(0.62, 0.66, 0.72);

/// Fraction in [0, 1] by which the weather greys the sky and dims the sun.
fn gloom(w: &WeatherState) -> f32 {
    (0.5 * w.rain + 0.35 * w.snow + 0.6 * w.storm + 0.8 * w.sand + 0.3 * w.fog).clamp(0.0, 0.85)
}

/// Applies the weather to the profile and the sky settings before the time
/// of day is resolved: rain, snow, storm and fog cover the sky, grey it and
/// dim the sun and the ambient light; fog and sand thicken the height fog.
/// Clear weather returns both inputs unchanged.
pub fn apply(
    ambience: &AmbienceProfile,
    sky: &SkyParams,
    w: &WeatherState,
) -> (AmbienceProfile, SkyParams) {
    if w.is_clear() {
        return (*ambience, *sky);
    }
    let mut a = *ambience;
    let mut s = *sky;
    let g = gloom(w);
    let cover = (0.75 * w.rain)
        .max(0.6 * w.snow)
        .max(0.95 * w.storm)
        .max(0.25 * w.fog)
        .max(0.5 * w.sand);
    s.cloud_cover = s.cloud_cover.max(cover);
    // Clouds follow the wind.
    if w.wind.x != 0.0 || w.wind.z != 0.0 {
        s.wind = faset_core::glam::Vec2::new(w.wind.x, w.wind.z);
    }

    let grey = |c: [f32; 3], amount: f32, tint: Vec3| {
        let c = Vec3::from_array(c);
        c.lerp(tint * luminance(c).max(0.01) * 1.1, amount)
            .to_array()
    };
    let tint = GREY_TINT.lerp(SAND_TINT, (w.sand * 1.5).clamp(0.0, 1.0));
    a.sky.horizon = grey(a.sky.horizon, g, tint);
    a.sky.zenith = grey(a.sky.zenith, g, tint);
    a.sky.stars *= 1.0 - g;
    a.sun.intensity *= 1.0 - 0.65 * g;
    let dim = 1.0 - 0.2 * g;
    a.ambient.sky = grey(a.ambient.sky, g, tint).map(|c| c * dim);
    a.ambient.ground = grey(a.ambient.ground, g, tint).map(|c| c * dim);

    a.fog.density += 0.035 * w.fog + 0.07 * w.sand + 0.004 * w.rain + 0.006 * w.snow;
    (a, s)
}

/// Adds the lightning flash (in [0, 1]) to the resolved light and sky.
pub fn apply_flash(resolved: &mut Resolved, flash: f32) {
    if flash <= 0.0 {
        return;
    }
    let boost = Vec3::new(0.85, 0.92, 1.0) * (1.4 * flash);
    for colour in [
        &mut resolved.ambience.ambient.sky,
        &mut resolved.ambience.ambient.ground,
    ] {
        *colour = (Vec3::from_array(*colour) + boost).to_array();
    }
    for colour in [&mut resolved.gpu.horizon, &mut resolved.gpu.zenith] {
        for (c, b) in colour.iter_mut().zip(boost.to_array()) {
            *c += 0.55 * b;
        }
    }
}

/// The weather part of the lighting uniform block (std140, every member
/// 16-byte aligned); must match the `weather_*` members of `Lighting` in
/// `lighting.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub(crate) struct GpuWeather {
    /// x rain, y snow, z sand, w fog.
    pub precip: [f32; 4],
    /// xyz wind in m/s, w humidity.
    pub wind_humidity: [f32; 4],
    /// x storm, y lightning flash.
    pub storm: [f32; 4],
    /// rgb colour fog is tinted towards, w tint strength.
    pub fog_tint: [f32; 4],
    /// xy world XZ of the shelter map's minimum corner, z cell size, w 1 when
    /// a shelter map is loaded.
    pub shelter_origin: [f32; 4],
    /// x width, y height of the shelter map in cells.
    pub shelter_dims: [f32; 4],
    /// Unjittered clip-from-world of this frame, column-major.
    pub clip_from_world: [f32; 16],
}

impl GpuWeather {
    /// Builds the block. `horizon` is the frame's resolved horizon colour,
    /// which sets how bright the tinted fog is.
    pub(crate) fn new(
        w: &WeatherState,
        flash: f32,
        horizon: [f32; 4],
        shelter: &ShelterInfo,
        clip_from_world: &faset_core::glam::Mat4,
    ) -> Self {
        let brightness = (luminance(Vec3::new(horizon[0], horizon[1], horizon[2])) * 1.2 + 0.03)
            .clamp(0.02, 1.0);
        let sand_part = (w.sand * 1.5).clamp(0.0, 1.0);
        let tint = GREY_TINT.lerp(SAND_TINT, sand_part) * brightness;
        let strength = (w.fog + w.sand + 0.3 * w.rain + 0.3 * w.snow).clamp(0.0, 1.0) * 0.85;
        Self {
            precip: [w.rain, w.snow, w.sand, w.fog],
            wind_humidity: [w.wind.x, w.wind.y, w.wind.z, w.humidity],
            storm: [w.storm, flash, 0.0, 0.0],
            fog_tint: [tint.x, tint.y, tint.z, strength],
            shelter_origin: [
                shelter.origin[0],
                shelter.origin[1],
                shelter.cell_size,
                if shelter.enabled { 1.0 } else { 0.0 },
            ],
            shelter_dims: [shelter.width as f32, shelter.height as f32, 0.0, 0.0],
            clip_from_world: clip_from_world.to_cols_array(),
        }
    }
}

// ---------------------------------------------------------------------------
// Pipeline
// ---------------------------------------------------------------------------

/// The weather pipeline: instanced camera-facing quads sharing the opaque
/// pass's pipeline layout (material table, lighting block, push constants).
/// The push constant `kind` selects rain, snow or sand.
pub(crate) struct WeatherPipeline {
    pub pipeline: vk::Pipeline,
}

impl WeatherPipeline {
    pub(crate) fn new(
        device: &ash::Device,
        layout: vk::PipelineLayout,
    ) -> Result<Self, RenderError> {
        let module = |words: &[u32]| {
            // SAFETY: SPIR-V produced and validated by naga.
            unsafe {
                device
                    .create_shader_module(&vk::ShaderModuleCreateInfo::default().code(words), None)
            }
        };
        let vs_module = module(faset_shaders::WEATHER_VS)?;
        let fs_module = match module(faset_shaders::WEATHER_FS) {
            Ok(m) => m,
            Err(e) => {
                // SAFETY: unused module.
                unsafe { device.destroy_shader_module(vs_module, None) };
                return Err(e.into());
            }
        };
        let pipeline = Self::create(device, layout, vs_module, fs_module);
        // SAFETY: modules may be destroyed once pipelines are created.
        unsafe {
            device.destroy_shader_module(vs_module, None);
            device.destroy_shader_module(fs_module, None);
        }
        Ok(Self {
            pipeline: pipeline?,
        })
    }

    fn create(
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
        // Particles are hidden by meshes (reverse-Z depth test) and write no
        // depth.
        let depth = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(true)
            .depth_write_enable(false)
            .depth_compare_op(vk::CompareOp::GREATER_OR_EQUAL);
        // Alpha blending into the HDR colour target only (a separate rendering
        // without the motion and normal targets, which need no blending).
        let colour = vk::PipelineColorBlendAttachmentState::default()
            .blend_enable(true)
            .src_color_blend_factor(vk::BlendFactor::SRC_ALPHA)
            .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .color_blend_op(vk::BlendOp::ADD)
            .src_alpha_blend_factor(vk::BlendFactor::ZERO)
            .dst_alpha_blend_factor(vk::BlendFactor::ONE)
            .alpha_blend_op(vk::BlendOp::ADD)
            .color_write_mask(vk::ColorComponentFlags::RGBA);
        let blend_attachments = [colour];
        let blend =
            vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachments);
        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
        let color_formats = [HDR_FORMAT];
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

    pub(crate) fn destroy(&mut self, device: &ash::Device) {
        // SAFETY: the device is idle.
        unsafe { device.destroy_pipeline(self.pipeline, None) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shelter::ShelterMap;
    use faset_core::glam::Vec2;

    fn state(rain: f32, wind: Vec3) -> WeatherState {
        WeatherState {
            rain,
            wind,
            ..WeatherState::CLEAR
        }
    }

    fn close(a: &WeatherState, b: &WeatherState) -> bool {
        (a.rain - b.rain).abs() < 1e-6
            && (a.snow - b.snow).abs() < 1e-6
            && (a.sand - b.sand).abs() < 1e-6
            && (a.fog - b.fog).abs() < 1e-6
            && (a.humidity - b.humidity).abs() < 1e-6
            && (a.storm - b.storm).abs() < 1e-6
            && (a.wind - b.wind).length() < 1e-5
    }

    fn schedule() -> WeatherSchedule {
        let wet = WeatherState {
            rain: 0.8,
            humidity: 0.6,
            storm: 0.4,
            wind: Vec3::new(4.0, 0.0, -2.0),
            ..WeatherState::CLEAR
        };
        let snow = WeatherState {
            snow: 1.0,
            fog: 0.2,
            ..WeatherState::CLEAR
        };
        WeatherSchedule::new(
            WeatherState::CLEAR,
            vec![
                WeatherKeyframe {
                    start_s: 100.0,
                    duration_s: 20.0,
                    target: wet,
                },
                WeatherKeyframe {
                    start_s: 200.0,
                    duration_s: 10.0,
                    target: snow,
                },
            ],
        )
        .unwrap()
    }

    #[test]
    fn sampling_hits_the_keyframes_and_the_midpoint_is_the_average() {
        let s = schedule();
        let (wet, snow) = (s.transitions()[0].target, s.transitions()[1].target);
        assert_eq!(s.sample(-5.0), WeatherState::CLEAR);
        assert_eq!(
            s.sample(100.0),
            WeatherState::CLEAR,
            "start of transition 1"
        );
        assert_eq!(s.sample(120.0), wet, "end of transition 1");
        assert_eq!(s.sample(150.0), wet, "held between transitions");
        assert_eq!(s.sample(200.0), wet, "start of transition 2");
        assert_eq!(s.sample(210.0), snow, "end of transition 2");
        assert_eq!(s.sample(1.0e6), snow);
        let mid = s.sample(110.0);
        let average = WeatherState::CLEAR.lerp(&wet, 0.5);
        assert!(close(&mid, &average), "{mid:?}");
        assert!((mid.rain - 0.4).abs() < 1e-6);
        assert!((mid.wind - Vec3::new(2.0, 0.0, -1.0)).length() < 1e-6);
        let mid2 = s.sample(205.0);
        assert!((mid2.rain - 0.4).abs() < 1e-6 && (mid2.snow - 0.5).abs() < 1e-6);
        assert_eq!(s.sample(f32::NAN), WeatherState::CLEAR);
    }

    #[test]
    fn transition_durations_outside_ten_to_thirty_seconds_are_rejected() {
        let key = |d: f32| WeatherKeyframe {
            start_s: 0.0,
            duration_s: d,
            target: state(1.0, Vec3::ZERO),
        };
        for d in [0.0, 9.99, 30.01, 120.0, -10.0, f32::NAN] {
            assert!(
                matches!(
                    WeatherSchedule::new(WeatherState::CLEAR, vec![key(d)]),
                    Err(WeatherError::InvalidDuration { index: 0, .. })
                ),
                "{d}"
            );
        }
        for d in [10.0, 17.5, 30.0] {
            assert!(WeatherSchedule::new(WeatherState::CLEAR, vec![key(d)]).is_ok());
        }
    }

    #[test]
    fn overlapping_or_invalid_schedules_are_rejected() {
        let key = |start: f32, rain: f32| WeatherKeyframe {
            start_s: start,
            duration_s: 20.0,
            target: state(rain, Vec3::ZERO),
        };
        assert!(matches!(
            WeatherSchedule::new(WeatherState::CLEAR, vec![key(0.0, 1.0), key(19.0, 0.0)]),
            Err(WeatherError::Overlap { index: 1, .. })
        ));
        assert!(
            WeatherSchedule::new(WeatherState::CLEAR, vec![key(0.0, 1.0), key(20.0, 0.0)]).is_ok()
        );
        assert!(matches!(
            WeatherSchedule::new(WeatherState::CLEAR, vec![key(0.0, 1.5)]),
            Err(WeatherError::OutOfRange { field: "rain", .. })
        ));
        let storm = state(0.0, Vec3::new(f32::NAN, 0.0, 0.0));
        assert_eq!(storm.validate(), Err(WeatherError::InvalidWind));
        assert_eq!(
            state(0.0, Vec3::new(MAX_WIND_SPEED + 1.0, 0.0, 0.0)).validate(),
            Err(WeatherError::InvalidWind)
        );
    }

    #[test]
    fn schedule_round_trips_through_ron_and_rejects_bad_data() {
        let s = schedule();
        let text = ron::to_string(&s).unwrap();
        assert_eq!(ron::from_str::<WeatherSchedule>(&text).unwrap(), s);
        let bad = "(initial: (), transitions: [(start_s: 0.0, duration_s: 3.0, target: ())])";
        assert!(ron::from_str::<WeatherSchedule>(bad).is_err());
        let state: WeatherState = ron::from_str("(rain: 0.5)").unwrap();
        assert_eq!(state.rain, 0.5);
        assert_eq!(state.snow, 0.0);
    }

    #[test]
    fn thunder_delay_is_distance_over_the_speed_of_sound() {
        assert_eq!(thunder_delay(343.0), 1.0);
        assert_eq!(thunder_delay(0.0), 0.0);
        assert!((thunder_delay(686.0) - 2.0).abs() < 1e-6);
        assert_eq!(thunder_delay(-5.0), 0.0);
        let strike = Strike {
            start_s: 3.0,
            distance_m: 343.0,
        };
        assert_eq!(strike.thunder(), ThunderEvent { delay_s: 1.0 });
    }

    #[test]
    fn lightning_needs_a_storm_and_is_deterministic() {
        assert!(strikes_between(0.0, 10_000.0, 0.2).is_empty());
        assert_eq!(lightning_flash(5.0, 0.0), 0.0);
        let storm = strikes_between(0.0, 700.0, 1.0);
        assert!(storm.len() > 30, "{}", storm.len());
        assert_eq!(storm, strikes_between(0.0, 700.0, 1.0));
        // Splitting the interval neither loses nor duplicates strikes.
        let mut parts = strikes_between(0.0, 123.4, 1.0);
        parts.extend(strikes_between(123.4, 700.0, 1.0));
        assert_eq!(parts, storm);
        for s in &storm {
            assert!((300.0..=4000.0).contains(&s.distance_m));
            // The flash is at its peak at the strike and gone afterwards.
            assert!(lightning_flash(s.start_s, 1.0) > 0.99);
            assert!(lightning_flash(s.start_s + FLASH_DURATION_S + 0.01, 1.0) < 1.0);
        }
        assert!(strikes_between(0.0, 700.0, 0.5).len() < storm.len());
    }

    #[test]
    fn flash_envelope_decays_with_a_second_stroke() {
        assert_eq!(flash_envelope(-0.1), 0.0);
        assert_eq!(flash_envelope(0.0), 1.0);
        assert!(flash_envelope(0.1) < flash_envelope(0.0));
        assert!(flash_envelope(0.15) > flash_envelope(0.12), "second stroke");
        assert_eq!(flash_envelope(FLASH_DURATION_S), 0.0);
    }

    #[test]
    fn particle_counts_scale_with_intensity() {
        assert_eq!(particle_count(KIND_RAIN, 0.0), 0);
        assert_eq!(particle_count(KIND_RAIN, f32::NAN), 0);
        assert_eq!(particle_count(KIND_RAIN, 1.0), MAX_PARTICLES[0]);
        assert_eq!(particle_count(KIND_SNOW, 7.0), MAX_PARTICLES[1]);
        assert_eq!(particle_count(KIND_SAND, 0.5), MAX_PARTICLES[2] / 2);
        assert_eq!(particle_count(KIND_SAND, 1e-9), 1);
    }

    #[test]
    fn particles_stay_in_the_camera_box_and_move_with_the_velocity() {
        let b = BOX_SIZE[0];
        let v = particle_velocity(KIND_RAIN, Vec3::new(3.0, 0.0, 0.0));
        assert_eq!(v, Vec3::new(3.0, -9.0, 0.0));
        let cam = Vec3::new(10.0, 2.0, -5.0);
        for i in 0..2000 {
            let p = particle_position(i, 1.7, v, b, cam);
            let d = (p - cam).abs();
            assert!(d.max_element() <= 0.5 * b + 1e-3, "{i}: {p}");
        }
        // Over a short step an unwrapped particle moves by v dt.
        let (p0, p1) = (
            particle_position(7, 0.0, v, b, cam),
            particle_position(7, 0.01, v, b, cam),
        );
        assert!(((p1 - p0) - v * 0.01).length() < 1e-3 || (p1 - p0).length() > 1.0);
        // The field is fixed in the world: moving the camera by a whole box
        // changes nothing, and moving it less changes only the wrapping.
        let shifted = cam + Vec3::new(b, 0.0, -b);
        let a = particle_position(11, 3.0, v, b, cam);
        let c = particle_position(11, 3.0, v, b, shifted);
        let wrapped = (c - a) / b;
        assert!((wrapped - wrapped.round()).length() < 1e-3, "{wrapped}");
    }

    #[test]
    fn snow_sways_sideways_within_its_amplitude() {
        let mut widest = 0.0_f32;
        for i in 0..200 {
            for k in 0..50 {
                let s = snow_sway(i, k as f32 * 0.37);
                assert_eq!(s.y, 0.0);
                assert!(s.x.abs() <= SNOW_SWAY + 1e-5 && s.z.abs() <= SNOW_SWAY + 1e-5);
                widest = widest.max(s.x.abs());
            }
        }
        assert!(widest > 0.9 * SNOW_SWAY);
        assert_ne!(snow_sway(3, 1.0), snow_sway(4, 1.0));
    }

    #[test]
    fn particle_hash_is_uniform_and_deterministic() {
        assert_eq!(particle_hash(5, 1), particle_hash(5, 1));
        assert_ne!(particle_hash(5, 0), particle_hash(5, 1));
        let mean: f32 = (0..10_000).map(|i| particle_hash(i, 0)).sum::<f32>() / 10_000.0;
        assert!((mean - 0.5).abs() < 0.02, "{mean}");
    }

    #[test]
    fn a_particle_under_a_roof_is_masked_and_one_outside_is_not() {
        // A 10 x 10 m roof at 4 m above the origin cells 0..10.
        let roof = ShelterMap::from_boxes(
            Vec2::ZERO,
            20,
            20,
            1.0,
            &[(Vec3::new(0.0, 0.0, 0.0), Vec3::new(10.0, 4.0, 10.0))],
        )
        .unwrap();
        let cam = Vec3::new(5.0, 2.0, 5.0);
        let b = BOX_SIZE[0];
        let v = particle_velocity(KIND_RAIN, Vec3::ZERO);
        let (mut under, mut outside) = (0, 0);
        let (mut under_masked, mut outside_masked) = (0, 0);
        for i in 0..4000 {
            let p = particle_position(i, 0.3, v, b, cam);
            let inside = (0.0..10.0).contains(&p.x) && (0.0..10.0).contains(&p.z);
            if inside && p.y < 4.0 {
                under += 1;
                under_masked += usize::from(roof.is_sheltered(p));
            } else if !inside {
                outside += 1;
                outside_masked += usize::from(roof.is_sheltered(p));
            }
        }
        assert!(under > 20 && outside > 1000, "{under} {outside}");
        assert_eq!(under_masked, under);
        assert_eq!(outside_masked, 0);
        // Above the roof the rain falls.
        assert!(!roof.is_sheltered(Vec3::new(5.0, 6.0, 5.0)));
        assert!(roof.is_sheltered(Vec3::new(5.0, 3.0, 5.0)));
    }

    #[test]
    fn clear_weather_leaves_the_profile_untouched() {
        let profile = AmbienceProfile::builtin("day").unwrap();
        let sky = SkyParams::default();
        assert_eq!(apply(&profile, &sky, &WeatherState::CLEAR), (profile, sky));
    }

    #[test]
    fn rain_covers_the_sky_dims_the_sun_and_thickens_the_fog() {
        let profile = AmbienceProfile::builtin("day").unwrap();
        let sky = SkyParams::default();
        let rain = WeatherState {
            rain: 1.0,
            storm: 0.6,
            ..WeatherState::CLEAR
        };
        let (a, s) = apply(&profile, &sky, &rain);
        assert!(s.cloud_cover > 0.5);
        assert!(a.sun.intensity < 0.6 * profile.sun.intensity);
        assert!(a.fog.density > profile.fog.density);
        let fogged = WeatherState {
            fog: 1.0,
            ..WeatherState::CLEAR
        };
        assert!(apply(&profile, &sky, &fogged).0.fog.density > profile.fog.density + 0.03);
    }

    #[test]
    fn flash_lights_the_scene() {
        use crate::camera::Camera;
        let profile = AmbienceProfile::builtin("night").unwrap();
        let mut r = crate::sky::resolve(&profile, &SkyParams::default(), true, &Camera::default());
        let dark = r.ambience.ambient.sky;
        apply_flash(&mut r, 0.0);
        assert_eq!(r.ambience.ambient.sky, dark);
        apply_flash(&mut r, 1.0);
        assert!(r.ambience.ambient.sky[2] > dark[2] + 1.0);
    }

    #[test]
    fn gpu_weather_matches_the_std140_size() {
        assert_eq!(std::mem::size_of::<GpuWeather>(), 6 * 16 + 64);
    }
}

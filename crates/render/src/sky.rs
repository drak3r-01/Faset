//! Procedural sky: map time to sun position, the time-of-day colours that
//! also light the scene, the sky uniform block and the sky pipeline.
//!
//! The sky itself is one full-screen triangle (`faset-shaders/src/sky.wgsl`)
//! drawn after the opaque pass where the depth buffer is still empty. The CPU
//! functions here mirror the shader formulas and are what the unit tests
//! check.

use ash::vk;
use bytemuck::{Pod, Zeroable};
use faset_core::glam::{Mat3, Mat4, Vec2, Vec3};
use serde::{Deserialize, Serialize};

use crate::ambience::{AmbienceProfile, SunSource};
use crate::camera::Camera;
use crate::opaque::{DEPTH_FORMAT, HDR_FORMAT, MOTION_FORMAT, NORMAL_FORMAT};
use crate::RenderError;

/// Anisotropy of the sun halo's Henyey-Greenstein phase function.
pub const SUN_PHASE_G: f32 = 0.76;

/// Per-map sky settings, supplied by the caller every frame (the server is
/// the authority on time; networked sync is a later lot).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SkyParams {
    /// Time of day in hours, [0, 24): 0 is midnight, 12 is noon.
    pub time_h: f32,
    /// In-game hours that pass per real second ([`SkyParams::advance`]); 0
    /// freezes the time.
    pub time_speed: f32,
    /// Latitude in degrees, [-90, 90]; positive is north.
    pub latitude_deg: f32,
    /// Solar declination in degrees (season). Decision: a per-map value, 0
    /// (equinox) when unset.
    pub declination_deg: f32,
    /// Cloud cover: 0 is clear, 1 is overcast.
    pub cloud_cover: f32,
    /// Wind vector that scrolls the clouds, in metres per second.
    pub wind: Vec2,
    /// Fraction of the star grid cells that hold a star, [0, 1].
    pub star_density: f32,
}

impl Default for SkyParams {
    /// Noon at 45 degrees north on an equinox, clear sky, light wind.
    fn default() -> Self {
        Self {
            time_h: 12.0,
            time_speed: 0.0,
            latitude_deg: 45.0,
            declination_deg: 0.0,
            cloud_cover: 0.0,
            wind: Vec2::new(1.0, 0.0),
            star_density: 0.15,
        }
    }
}

impl SkyParams {
    /// Advances `time_h` by `dt` real seconds at `time_speed`, wrapping into
    /// [0, 24).
    pub fn advance(&mut self, dt: f32) {
        self.time_h = (self.time_h + self.time_speed * dt).rem_euclid(24.0);
    }
}

/// Sun altitude above the horizon and azimuth, in radians.
///
/// Hour angle `h = 2 pi (t / 24 - 0.5)`; with latitude `phi` and declination
/// `delta`:
/// `sin(el) = sin phi sin delta + cos phi cos delta cos h` and
/// `az = atan2(sin h, cos h sin phi - tan delta cos phi)`.
/// Decision: that `az` is measured from the south, positive towards the
/// west; the azimuth returned here is converted to the world convention
/// (from -Z, clockwise towards +X) by adding pi, so the sun rises in +X and
/// is due +Z at noon in the northern hemisphere.
pub fn sun_altitude_azimuth(time_h: f32, latitude_deg: f32, declination_deg: f32) -> (f32, f32) {
    let h = std::f32::consts::TAU * (time_h / 24.0 - 0.5);
    let phi = latitude_deg.to_radians();
    let delta = declination_deg.to_radians();
    let sin_el = phi.sin() * delta.sin() + phi.cos() * delta.cos() * h.cos();
    let el = sin_el.clamp(-1.0, 1.0).asin();
    let az = h.sin().atan2(h.cos() * phi.sin() - delta.tan() * phi.cos());
    (el, az + std::f32::consts::PI)
}

/// Unit vector from the viewer towards the sun, Y up, -Z forward, azimuth
/// measured from -Z clockwise towards +X (see [`sun_altitude_azimuth`]).
pub fn sun_direction(time_h: f32, latitude_deg: f32, declination_deg: f32) -> Vec3 {
    let (el, az) = sun_altitude_azimuth(time_h, latitude_deg, declination_deg);
    Vec3::new(az.sin() * el.cos(), el.sin(), -az.cos() * el.cos())
}

/// Henyey-Greenstein phase function of the angle `theta` whose cosine is
/// `cos_theta`; integrates to 1 over the sphere.
pub fn henyey_greenstein(cos_theta: f32, g: f32) -> f32 {
    let denom = 1.0 + g * g - 2.0 * g * cos_theta;
    (1.0 - g * g) / (4.0 * std::f32::consts::PI * denom * denom.sqrt())
}

/// Star grid: equal-height bands of an equal-area cylindrical projection.
pub const STAR_COLS: i32 = 200;
pub const STAR_ROWS: i32 = 64;

/// PCG hash of one 32-bit word (same bits as `noise.wgsl`).
fn hash_u32(v: u32) -> u32 {
    let state = v.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
    let word = ((state >> ((state >> 28) + 4)) ^ state).wrapping_mul(277_803_737);
    (word >> 22) ^ word
}

/// Hash of an integer cell in [0, 1] (same bits as `hash2` in `noise.wgsl`).
pub fn hash_cell(col: i32, row: i32) -> f32 {
    let h = hash_u32((col as u32) ^ hash_u32((row as u32).wrapping_add(0x9e37_79b9)));
    (h >> 8) as f32 * (1.0 / 16_777_215.0)
}

/// The star grid cell `(column, row)` a direction falls in; columns wrap
/// around the azimuth, rows go from -Y to +Y.
pub fn star_cell(dir: Vec3) -> (i32, i32) {
    let dir = dir.normalize_or(Vec3::Y);
    let col = (dir.z.atan2(dir.x) / std::f32::consts::TAU + 0.5) * STAR_COLS as f32;
    let row = (dir.y * 0.5 + 0.5) * STAR_ROWS as f32;
    (
        (col.floor() as i32).rem_euclid(STAR_COLS),
        (row.floor() as i32).clamp(0, STAR_ROWS - 1),
    )
}

/// True when the cell holds a star at the given density.
pub fn cell_has_star(col: i32, row: i32, density: f32) -> bool {
    hash_cell(col, row) < density
}

/// Smoothstep with `edge0 < edge1`.
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn mix3(a: Vec3, b: Vec3, t: f32) -> Vec3 {
    a * (1.0 - t) + b * t
}

fn luminance(c: Vec3) -> f32 {
    c.dot(Vec3::new(0.2126, 0.7152, 0.0722))
}

/// Sky horizon and zenith colour at night.
const NIGHT_HORIZON: Vec3 = Vec3::new(0.012, 0.018, 0.040);
const NIGHT_ZENITH: Vec3 = Vec3::new(0.002, 0.004, 0.014);
/// Colours the horizon and the zenith take at dusk and dawn.
const TWILIGHT_HORIZON: Vec3 = Vec3::new(1.0, 0.45, 0.20);
const TWILIGHT_ZENITH: Vec3 = Vec3::new(0.22, 0.16, 0.38);
/// Sun colour near the horizon.
const TWILIGHT_SUN: Vec3 = Vec3::new(1.0, 0.55, 0.25);

/// 0 at night, 1 by day, as a function of the sun's height `sin(el)`.
pub(crate) fn daylight_of_sun(sun_y: f32) -> f32 {
    smoothstep(-0.10, 0.20, sun_y)
}

/// Peaks (1) when the sun is on the horizon, 0 well above and below it.
pub(crate) fn twilight_of_sun(sun_y: f32) -> f32 {
    smoothstep(-0.18, 0.0, sun_y) * (1.0 - smoothstep(0.0, 0.35, sun_y))
}

/// Sun, sky and ambient values of one frame, after the time of day is
/// applied.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Resolved {
    /// The profile with the sun and ambient colours adjusted (and the sun
    /// direction computed) when its sun source is the time of day.
    pub ambience: AmbienceProfile,
    pub gpu: GpuSky,
}

/// Applies the map time to `ambience` and builds the sky uniform block.
///
/// With [`SunSource::TimeOfDay`] the sun direction comes from
/// [`sun_direction`], the sun fades out and reddens near the horizon, the
/// sky colours of the profile (its midday look) blend to a night palette and
/// a warm twilight, and the profile's ambient colours follow the zenith
/// colour: the ambient is read from the sky looking up. Otherwise the
/// profile is used as it is. `draw` says whether the sky pass runs (fog then
/// takes the sky colour).
pub(crate) fn resolve(
    ambience: &AmbienceProfile,
    sky: &SkyParams,
    draw: bool,
    camera: &Camera,
) -> Resolved {
    let mut a = *ambience;
    let mut horizon = Vec3::from_array(a.sky.horizon);
    let mut zenith = Vec3::from_array(a.sky.zenith);
    if a.sun.source == SunSource::TimeOfDay {
        let dir = sun_direction(sky.time_h, sky.latitude_deg, sky.declination_deg);
        let day = daylight_of_sun(dir.y);
        let twilight = twilight_of_sun(dir.y);
        a.sun.direction = dir.to_array();
        a.sun.intensity *= smoothstep(-0.05, 0.12, dir.y);
        a.sun.color = mix3(Vec3::from_array(a.sun.color), TWILIGHT_SUN, twilight * 0.8).to_array();

        let (profile_horizon, profile_zenith) = (horizon, zenith);
        horizon = mix3(NIGHT_HORIZON, profile_horizon, day);
        zenith = mix3(NIGHT_ZENITH, profile_zenith, day);
        horizon = mix3(horizon, TWILIGHT_HORIZON, twilight * 0.65);
        zenith = mix3(zenith, TWILIGHT_ZENITH, twilight * 0.35);
        a.sky.horizon = horizon.to_array();
        a.sky.zenith = zenith.to_array();

        // Ambient: the profile's, scaled by how the zenith colour changed.
        let guard = |c: Vec3| c.max(Vec3::splat(0.02));
        let ratio = (zenith / guard(profile_zenith)).clamp(Vec3::splat(0.08), Vec3::splat(1.5));
        let ground = (luminance(zenith) / luminance(guard(profile_zenith))).clamp(0.08, 1.5);
        a.ambient.sky = (Vec3::from_array(a.ambient.sky) * ratio).to_array();
        a.ambient.ground = (Vec3::from_array(a.ambient.ground) * ground).to_array();
    }

    let horizon_luma = luminance(horizon);
    // Stars fade as the sky brightens; clouds and the moon follow daylight.
    let star_visibility = 1.0 - smoothstep(0.02, 0.15, horizon_luma);
    let daylight = (horizon_luma * 3.0).clamp(0.0, 1.0);
    let space = a.sky.space.clamp(0.0, 1.0);
    let sun = Vec3::from_array(a.sun.direction).normalize_or(Vec3::Y);
    let moon = (-sun + Vec3::X * 0.35).normalize_or(Vec3::Y);
    let moon_brightness = (1.0 - 0.8 * daylight) * (1.0 - space);

    let view_rotation = Mat3::from_mat4(camera.view).transpose();
    let gpu = GpuSky {
        horizon: horizon.extend(a.sky.exponent).to_array(),
        zenith: zenith.extend(a.sky.stars * star_visibility).to_array(),
        moon: moon.extend(moon_brightness).to_array(),
        params: [
            sky.cloud_cover.clamp(0.0, 1.0),
            sky.star_density.clamp(0.0, 1.0),
            space,
            if draw { 1.0 } else { 0.0 },
        ],
        wind: [sky.wind.x, sky.wind.y, daylight, 0.0],
        ray: [
            1.0 / camera.proj.x_axis.x,
            1.0 / camera.proj.y_axis.y,
            0.0,
            0.0,
        ],
        world_from_view: Mat4::from_mat3(view_rotation).to_cols_array(),
        prev_view_proj: camera.prev_view_proj.to_cols_array(),
    };
    Resolved { ambience: a, gpu }
}

/// The sky part of the lighting uniform block (std140, every member 16-byte
/// aligned); must match the `sky_*` members of `Lighting` in `lighting.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub(crate) struct GpuSky {
    /// rgb horizon colour, w horizon-to-zenith exponent.
    pub horizon: [f32; 4],
    /// rgb zenith colour, w star brightness.
    pub zenith: [f32; 4],
    /// xyz unit vector towards the moon, w moon brightness.
    pub moon: [f32; 4],
    /// x cloud cover, y star density, z space strength, w 1 when drawn.
    pub params: [f32; 4],
    /// xy wind, z daylight.
    pub wind: [f32; 4],
    /// xy view-space ray slope per unit of NDC.
    pub ray: [f32; 4],
    /// World-from-view rotation, column-major.
    pub world_from_view: [f32; 16],
    /// Unjittered clip-from-world of the previous frame, column-major.
    pub prev_view_proj: [f32; 16],
}

/// The sky pipeline: a full-screen triangle sharing the opaque pass's
/// pipeline layout (material table, lighting block, push constants).
pub(crate) struct SkyPipeline {
    pub pipeline: vk::Pipeline,
}

impl SkyPipeline {
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
        let vs_module = module(faset_shaders::SKY_VS)?;
        let fs_module = match module(faset_shaders::SKY_FS) {
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
        // Reverse-Z: the cleared depth is 0 and the triangle sits at 0, so
        // only pixels no mesh covered pass. The sky writes no depth.
        let depth = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(true)
            .depth_write_enable(false)
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

    pub(crate) fn destroy(&mut self, device: &ash::Device) {
        // SAFETY: the device is idle.
        unsafe { device.destroy_pipeline(self.pipeline, None) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn altitude(t: f32, lat: f32, decl: f32) -> f32 {
        sun_altitude_azimuth(t, lat, decl).0
    }

    #[test]
    fn noon_at_the_equator_is_straight_up() {
        let el = altitude(12.0, 0.0, 0.0);
        assert!((el - std::f32::consts::FRAC_PI_2).abs() < 1e-4, "el = {el}");
        assert!((sun_direction(12.0, 0.0, 0.0) - Vec3::Y).length() < 1e-3);
    }

    #[test]
    fn sunrise_and_sunset_are_on_the_horizon_at_the_equator() {
        for t in [6.0, 18.0] {
            assert!(altitude(t, 0.0, 0.0).abs() < 1e-3, "t = {t}");
        }
        // Midnight: straight down.
        assert!((altitude(0.0, 0.0, 0.0) + std::f32::consts::FRAC_PI_2).abs() < 1e-4);
    }

    #[test]
    fn sun_rises_east_and_is_south_at_noon_in_the_north() {
        // +X is east, -Z north.
        let dawn = sun_direction(6.0, 0.0, 0.0);
        assert!(dawn.x > 0.99, "{dawn}");
        let dusk = sun_direction(18.0, 0.0, 0.0);
        assert!(dusk.x < -0.99, "{dusk}");
        let noon = sun_direction(12.0, 45.0, 0.0);
        assert!(noon.z > 0.7 && noon.x.abs() < 1e-3, "{noon}");
        assert!((noon.y - 45f32.to_radians().sin().max(0.0)).abs() < 1e-3);
        // Southern hemisphere: the noon sun is north.
        assert!(sun_direction(12.0, -45.0, 0.0).z < -0.7);
        // The result is a unit vector everywhere.
        for t in [0.0, 3.3, 9.9, 15.2, 23.9] {
            let d = sun_direction(t, 52.0, 12.0);
            assert!((d.length() - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn declination_raises_the_summer_sun() {
        let equinox = altitude(12.0, 45.0, 0.0);
        let summer = altitude(12.0, 45.0, 23.4);
        assert!((equinox - 45f32.to_radians()).abs() < 1e-4);
        assert!((summer - 68.4f32.to_radians()).abs() < 1e-3);
    }

    #[test]
    fn henyey_greenstein_integrates_to_one_over_the_sphere() {
        for g in [0.0, 0.3, SUN_PHASE_G] {
            // 2 pi integral of P over cos(theta) in [-1, 1], midpoint rule
            // in theta (the lobe is narrow near theta = 0).
            let n = 200_000;
            let step = std::f64::consts::PI / n as f64;
            let sum: f64 = (0..n)
                .map(|i| {
                    let theta = (i as f64 + 0.5) * step;
                    f64::from(henyey_greenstein(theta.cos() as f32, g))
                        * theta.sin()
                        * step
                        * std::f64::consts::TAU
                })
                .sum();
            assert!((sum - 1.0).abs() < 1e-3, "g = {g}: {sum}");
        }
    }

    #[test]
    fn star_hash_is_deterministic_and_wraps() {
        let dirs = [
            Vec3::new(0.3, 0.5, -0.8),
            Vec3::Y,
            Vec3::NEG_Y,
            Vec3::new(-1.0, 0.0, -1e-9),
            Vec3::new(1.0, 1e-3, 0.5),
        ];
        for d in dirs {
            let (c, r) = star_cell(d);
            assert_eq!(star_cell(d), (c, r));
            assert_eq!(star_cell(d * 7.0), (c, r), "length does not matter");
            assert!((0..STAR_COLS).contains(&c) && (0..STAR_ROWS).contains(&r));
            assert_eq!(hash_cell(c, r), hash_cell(c, r));
            assert!((0.0..=1.0).contains(&hash_cell(c, r)));
        }
        assert_ne!(hash_cell(0, 0), hash_cell(1, 0));
        assert_ne!(hash_cell(0, 0), hash_cell(0, 1));
    }

    #[test]
    fn about_density_of_the_cells_hold_a_star() {
        let total = STAR_COLS * STAR_ROWS;
        for density in [0.05_f32, 0.15, 0.5] {
            let stars = (0..STAR_COLS)
                .flat_map(|c| (0..STAR_ROWS).map(move |r| (c, r)))
                .filter(|&(c, r)| cell_has_star(c, r, density))
                .count();
            let fraction = stars as f32 / total as f32;
            assert!((fraction - density).abs() < 0.02, "{density}: {fraction}");
        }
        assert!(!cell_has_star(3, 4, 0.0));
    }

    #[test]
    fn resolve_follows_the_time_of_day() {
        let mut profile = AmbienceProfile::builtin("day").unwrap();
        profile.sun.source = SunSource::TimeOfDay;
        let camera = Camera::default();
        let sky_at = |t: f32| {
            let sky = SkyParams {
                time_h: t,
                latitude_deg: 0.0,
                ..SkyParams::default()
            };
            resolve(&profile, &sky, true, &camera)
        };
        let noon = sky_at(12.0);
        assert!((Vec3::from_array(noon.ambience.sun.direction) - Vec3::Y).length() < 1e-3);
        assert!((noon.ambience.sun.intensity - profile.sun.intensity).abs() < 1e-6);
        assert_eq!(noon.ambience.ambient, profile.ambient);
        assert_eq!(noon.gpu.horizon[..3], profile.sky.horizon);

        let midnight = sky_at(0.0);
        assert_eq!(midnight.ambience.sun.intensity, 0.0, "the sun is down");
        let dim = |a: &AmbienceProfile| Vec3::from_array(a.ambient.sky).length();
        assert!(dim(&midnight.ambience) < 0.2 * dim(&noon.ambience));
        assert!(
            midnight.gpu.zenith[3] == 0.0,
            "the day profile has no stars"
        );

        let dusk = sky_at(18.0);
        let warm = Vec3::from_array(dusk.ambience.sun.color);
        assert!(warm.x > warm.z, "reddish sun at the horizon");
        // The sky is darker and redder than at noon.
        assert!(dusk.gpu.horizon[0] > dusk.gpu.horizon[2]);

        // Without the time-of-day source the profile is untouched.
        let fixed = AmbienceProfile::builtin("day").unwrap();
        let r = resolve(&fixed, &SkyParams::default(), false, &camera);
        assert_eq!(r.ambience, fixed);
        assert_eq!(r.gpu.params[3], 0.0);
    }

    #[test]
    fn advance_wraps_the_time() {
        let mut sky = SkyParams {
            time_h: 23.5,
            time_speed: 2.0,
            ..SkyParams::default()
        };
        sky.advance(0.5);
        assert!((sky.time_h - 0.5).abs() < 1e-5);
        sky.time_speed = 0.0;
        sky.advance(10.0);
        assert!((sky.time_h - 0.5).abs() < 1e-5);
    }

    #[test]
    fn sky_params_round_trip_through_ron() {
        let sky = SkyParams {
            time_h: 6.5,
            cloud_cover: 0.4,
            wind: Vec2::new(3.0, -1.0),
            ..SkyParams::default()
        };
        let text = ron::to_string(&sky).unwrap();
        assert_eq!(ron::from_str::<SkyParams>(&text).unwrap(), sky);
        // Missing fields take their defaults.
        let partial: SkyParams = ron::from_str("(time_h: 3.0)").unwrap();
        assert_eq!(partial.time_h, 3.0);
        assert_eq!(partial.cloud_cover, 0.0);
    }

    #[test]
    fn gpu_sky_matches_the_std140_size() {
        assert_eq!(std::mem::size_of::<GpuSky>(), 6 * 16 + 2 * 64);
    }
}

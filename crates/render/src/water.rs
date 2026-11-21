//! Water surface and underwater medium: Gerstner waves, the underwater flag,
//! the underwater fog and the suspended particles.
//!
//! The surface is drawn by `faset-shaders/src/water.wgsl` (see
//! [`WaterPipeline`]) and the caustics by `caustics.wgsl`; the functions here
//! mirror their formulas and are what the unit tests check. Water is visual
//! only: buoyancy and swimming belong to the simulation.
//!
//! The wave sum (plan form), with `x0` the rest position of a surface point,
//! `D_i` the unit direction, `k_i = 2 pi / lambda_i` and
//! `phi_i = k_i D_i . x0 - omega_i t`:
//!
//! `x += sum_i Q_i A_i D_i cos(phi_i)`, `y += sum_i A_i sin(phi_i)`.

use ash::vk;
use bytemuck::{Pod, Zeroable};
use faset_core::glam::{Vec2, Vec3};
use serde::{Deserialize, Serialize};

use crate::ambience::{AmbienceProfile, Water};
use crate::opaque::{DEPTH_FORMAT, HDR_FORMAT};
use crate::weather::particle_position;
use crate::RenderError;

/// Gravity used by the dispersion relation, in m/s^2.
///
/// Decision: the plan gives no value for g; 9.81 m/s^2.
pub const GRAVITY: f32 = 9.81;

/// Waves the shader sums; a [`WaterParams`] holds at most this many.
pub const MAX_WAVES: usize = 8;

/// Cells per side of the surface grid drawn around the camera.
pub const GRID_CELLS: u32 = 160;

/// Half-size in metres of the surface grid (cells are denser near the
/// camera: the offset from the camera grows with the square of the grid
/// coordinate).
pub const GRID_EXTENT: f32 = 300.0;

/// One Gerstner wave.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GerstnerWave {
    /// Amplitude `A` in metres, >= 0.
    pub amplitude: f32,
    /// Steepness `Q` in [0, 1]: 0 is a sine, 1 a sharp crest.
    pub steepness: f32,
    /// Horizontal travel direction (XZ); normalised when used.
    pub direction: Vec2,
    /// Wavelength in metres, > 0.
    pub wavelength: f32,
}

impl GerstnerWave {
    /// Wavenumber `k = 2 pi / lambda`.
    pub fn wavenumber(&self) -> f32 {
        wavenumber(self.wavelength)
    }

    /// Angular frequency `omega = sqrt(g k)`.
    pub fn omega(&self) -> f32 {
        angular_frequency(self.wavelength)
    }

    fn unit_direction(&self) -> Vec2 {
        self.direction.normalize_or(Vec2::X)
    }
}

/// Wavenumber `k = 2 pi / lambda`.
pub fn wavenumber(wavelength: f32) -> f32 {
    std::f32::consts::TAU / wavelength
}

/// Deep-water dispersion `omega = sqrt(g k)` of a wave of `wavelength`.
pub fn angular_frequency(wavelength: f32) -> f32 {
    (GRAVITY * wavenumber(wavelength)).sqrt()
}

/// Water errors.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum WaterError {
    #[error("at most {MAX_WAVES} waves are supported, got {0}")]
    TooManyWaves(usize),
    #[error("wave {0}: amplitude must be >= 0 and finite")]
    Amplitude(usize),
    #[error("wave {0}: steepness must be in [0, 1]")]
    Steepness(usize),
    #[error("wave {0}: wavelength must be > 0 and finite")]
    Wavelength(usize),
    #[error("wave {0}: direction must be finite and non-zero")]
    Direction(usize),
    #[error("water height must be finite")]
    Height,
    #[error("the sum of Q k A is {0}, above 1: the surface would fold over")]
    FoldOver(f32),
}

/// The water of a map: its rest height and its waves.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WaterParams {
    /// Rest height of the surface in metres (world Y).
    pub height: f32,
    pub waves: Vec<GerstnerWave>,
}

impl WaterParams {
    /// Sum of `Q_i k_i A_i`; the surface does not fold over while it is <= 1.
    pub fn steepness_sum(&self) -> f32 {
        self.waves
            .iter()
            .map(|w| w.steepness * w.wavenumber() * w.amplitude)
            .sum()
    }

    /// Checks the waves: at most [`MAX_WAVES`], every value in range, and the
    /// steepness constraint `sum Q_i k_i A_i <= 1`.
    pub fn validate(&self) -> Result<(), WaterError> {
        if !self.height.is_finite() {
            return Err(WaterError::Height);
        }
        if self.waves.len() > MAX_WAVES {
            return Err(WaterError::TooManyWaves(self.waves.len()));
        }
        for (i, w) in self.waves.iter().enumerate() {
            if !(w.amplitude.is_finite() && w.amplitude >= 0.0) {
                return Err(WaterError::Amplitude(i));
            }
            if !(0.0..=1.0).contains(&w.steepness) {
                return Err(WaterError::Steepness(i));
            }
            if !(w.wavelength.is_finite() && w.wavelength > 0.0) {
                return Err(WaterError::Wavelength(i));
            }
            if !(w.direction.is_finite() && w.direction.length() > 0.0) {
                return Err(WaterError::Direction(i));
            }
        }
        let sum = self.steepness_sum();
        // A tiny slack absorbs the rounding of a sum meant to be exactly 1.
        if sum > 1.0 + 1.0e-6 {
            return Err(WaterError::FoldOver(sum));
        }
        Ok(())
    }

    /// True when a camera at height `camera_y` is under the surface.
    pub fn contains_camera_height(&self, camera_y: f32) -> bool {
        is_underwater(camera_y, self.height)
    }
}

/// The underwater flag: true exactly when the camera is below the water
/// height.
pub fn is_underwater(camera_y: f32, water_height: f32) -> bool {
    camera_y < water_height
}

/// Surface point of the Gerstner sum at rest position `p` (XZ) and time `t`
/// (seconds), and its unit normal. The position is relative to the rest
/// height (add the water height to Y); the normal is built from the analytic
/// partial derivatives of the sum:
///
/// `dP/dx0 = (1 - sum Q A k Dx^2 sin, sum A k Dx cos, -sum Q A k Dx Dz sin)`,
/// `dP/dz0 = (-sum Q A k Dx Dz sin, sum A k Dz cos, 1 - sum Q A k Dz^2 sin)`,
/// `N = normalize(dP/dz0 x dP/dx0)`.
pub fn gerstner_displace(p: Vec2, t: f32, waves: &[GerstnerWave]) -> (Vec3, Vec3) {
    let mut pos = Vec3::new(p.x, 0.0, p.y);
    let (mut dxx, mut dxz, mut dzz) = (0.0, 0.0, 0.0);
    let (mut hx, mut hz) = (0.0, 0.0);
    for w in waves {
        let d = w.unit_direction();
        let k = w.wavenumber();
        let phase = k * d.dot(p) - w.omega() * t;
        let (s, c) = phase.sin_cos();
        pos.x += w.steepness * w.amplitude * d.x * c;
        pos.z += w.steepness * w.amplitude * d.y * c;
        pos.y += w.amplitude * s;
        let qak = w.steepness * w.amplitude * k;
        dxx += qak * d.x * d.x * s;
        dxz += qak * d.x * d.y * s;
        dzz += qak * d.y * d.y * s;
        hx += w.amplitude * k * d.x * c;
        hz += w.amplitude * k * d.y * c;
    }
    let tangent_x = Vec3::new(1.0 - dxx, hx, -dxz);
    let tangent_z = Vec3::new(-dxz, hz, 1.0 - dzz);
    let normal = tangent_z.cross(tangent_x).normalize_or(Vec3::Y);
    (pos, normal)
}

/// The fog of the underwater medium: the profile's fog colour with the
/// water's density, uniform with height (the height fog of the profile is
/// replaced).
pub fn underwater_profile(ambience: &AmbienceProfile) -> AmbienceProfile {
    let mut a = *ambience;
    a.fog.density = a.water.fog_density;
    a.fog.falloff = 0.0;
    a
}

// ---------------------------------------------------------------------------
// Suspended particles (same stateless technique as the weather particles)
// ---------------------------------------------------------------------------

/// Push constant `kind` of the suspended-particle draw.
pub const KIND_MOTE: u32 = 3;
/// Suspended particles drawn underwater.
pub const MOTE_COUNT: u32 = 6_000;
/// Side of the camera-following box of the suspended particles, in metres.
pub const MOTE_BOX: f32 = 14.0;
/// Slow drift of the suspended particles, in metres per second.
pub const MOTE_VELOCITY: Vec3 = Vec3::new(0.05, 0.08, 0.03);

/// World position of suspended particle `index` at time `t`; the same
/// wrapping model as the weather particles (`weather::particle_position`).
pub fn mote_position(index: u32, t: f32, cam: Vec3) -> Vec3 {
    particle_position(index, t, MOTE_VELOCITY, MOTE_BOX, cam)
}

// ---------------------------------------------------------------------------
// GPU block
// ---------------------------------------------------------------------------

/// The water part of the lighting uniform block (std140); must match the
/// `water_*` members of `Lighting` in `lighting.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub(crate) struct GpuWater {
    /// x surface height, y 1 when the camera is underwater, z caustics
    /// strength (0 without water), w wave count.
    pub params: [f32; 4],
    /// rgb shallow colour, w underwater fog density.
    pub shallow: [f32; 4],
    /// rgb deep colour.
    pub deep: [f32; 4],
    /// xy unit direction, z wavenumber, w angular frequency.
    pub wave_dir: [[f32; 4]; MAX_WAVES],
    /// x amplitude, y steepness.
    pub wave_amp: [[f32; 4]; MAX_WAVES],
}

impl GpuWater {
    pub(crate) fn new(look: &Water, water: Option<&WaterParams>, underwater: bool) -> Self {
        let mut block = Self::zeroed();
        block.params = [
            water.map_or(0.0, |w| w.height),
            if underwater { 1.0 } else { 0.0 },
            if water.is_some() { look.caustics } else { 0.0 },
            0.0,
        ];
        block.shallow = [
            look.shallow[0],
            look.shallow[1],
            look.shallow[2],
            look.fog_density,
        ];
        block.deep = [look.deep[0], look.deep[1], look.deep[2], 0.0];
        if let Some(water) = water {
            let used = &water.waves[..water.waves.len().min(MAX_WAVES)];
            block.params[3] = used.len() as f32;
            for (i, w) in used.iter().enumerate() {
                let d = w.unit_direction();
                block.wave_dir[i] = [d.x, d.y, w.wavenumber(), w.omega()];
                block.wave_amp[i] = [w.amplitude, w.steepness, 0.0, 0.0];
            }
        }
        block
    }
}

// ---------------------------------------------------------------------------
// Pipeline
// ---------------------------------------------------------------------------

/// The water surface pipeline: a camera-centred grid generated from the
/// vertex index, drawn opaque into the HDR target (depth-tested and
/// depth-writing) with the opaque pass's pipeline layout.
pub(crate) struct WaterPipeline {
    pub pipeline: vk::Pipeline,
}

impl WaterPipeline {
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
        let vs_module = module(faset_shaders::WATER_VS)?;
        let fs_module = match module(faset_shaders::WATER_FS) {
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
        // Seen from above and from below.
        let raster = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::NONE)
            .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let depth = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(true)
            .depth_write_enable(true)
            .depth_compare_op(vk::CompareOp::GREATER_OR_EQUAL);
        let colour = vk::PipelineColorBlendAttachmentState::default()
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

    fn wave(a: f32, q: f32, dir: Vec2, lambda: f32) -> GerstnerWave {
        GerstnerWave {
            amplitude: a,
            steepness: q,
            direction: dir,
            wavelength: lambda,
        }
    }

    fn test_waves(scale: f32) -> Vec<GerstnerWave> {
        vec![
            wave(0.30, 0.5 * scale, Vec2::new(1.0, 0.0), 12.0),
            wave(0.15, 0.7 * scale, Vec2::new(0.6, 0.8), 7.0),
            wave(0.08, 0.9 * scale, Vec2::new(-0.3, 1.0), 4.0),
            wave(0.05, 1.0 * scale, Vec2::new(-1.0, -0.4), 2.5),
        ]
    }

    #[test]
    fn zero_steepness_is_a_pure_sine() {
        let w = wave(0.4, 0.0, Vec2::new(0.6, 0.8), 9.0);
        let mut worst = 0.0f32;
        for i in 0..200 {
            let p = Vec2::new(i as f32 * 0.37 - 20.0, i as f32 * 0.21);
            let t = i as f32 * 0.13;
            let (pos, _) = gerstner_displace(p, t, &[w]);
            let d = Vec2::new(0.6, 0.8);
            let phase = w.wavenumber() * d.dot(p) - w.omega() * t;
            worst = worst.max((pos.y - 0.4 * phase.sin()).abs());
            assert_eq!((pos.x, pos.z), (p.x, p.y));
        }
        assert!(worst < 1e-6, "max error {worst}");
    }

    #[test]
    fn dispersion_matches_the_plan() {
        for lambda in [0.5, 2.0, 7.3, 30.0, 150.0] {
            let w = wave(0.1, 0.0, Vec2::X, lambda);
            let expected = (GRAVITY * std::f32::consts::TAU / lambda).sqrt();
            assert!((w.omega() - expected).abs() < 1e-6, "lambda {lambda}");
            assert!((angular_frequency(lambda) - expected).abs() < 1e-6);
        }
    }

    #[test]
    fn normals_are_unit_and_point_up() {
        let waves = test_waves(1.0);
        let water = WaterParams {
            height: 0.0,
            waves: waves.clone(),
        };
        assert!(water.steepness_sum() <= 1.0, "{}", water.steepness_sum());
        water.validate().unwrap();
        for i in 0..400 {
            let p = Vec2::new((i % 20) as f32 * 3.1 - 30.0, (i / 20) as f32 * 2.3 - 20.0);
            let (_, n) = gerstner_displace(p, i as f32 * 0.071, &waves);
            assert!((n.length() - 1.0).abs() < 1e-4);
            assert!(n.y > 0.0, "fold-over at {p}: {n}");
        }
    }

    #[test]
    fn normal_matches_finite_differences() {
        let waves = test_waves(1.0);
        let (p, t, h) = (Vec2::new(3.7, -2.1), 1.9, 1e-2);
        // The normal belongs to the rest position, so differentiate the
        // displaced position with respect to it.
        let at = |q: Vec2| gerstner_displace(q, t, &waves).0;
        let dx = (at(p + Vec2::new(h, 0.0)) - at(p - Vec2::new(h, 0.0))) / (2.0 * h);
        let dz = (at(p + Vec2::new(0.0, h)) - at(p - Vec2::new(0.0, h))) / (2.0 * h);
        let expected = dz.cross(dx).normalize();
        let (_, n) = gerstner_displace(p, t, &waves);
        assert!((n - expected).length() < 2e-3, "{n} vs {expected}");
    }

    #[test]
    fn steepness_constraint_is_enforced() {
        let ok = WaterParams {
            height: 1.0,
            waves: vec![wave(0.1, 1.0, Vec2::X, 10.0)],
        };
        assert!(ok.validate().is_ok());
        // Q k A = 1 * (2 pi / 2) * 0.5 > 1.
        let folded = WaterParams {
            height: 1.0,
            waves: vec![wave(0.5, 1.0, Vec2::X, 2.0)],
        };
        assert!(matches!(folded.validate(), Err(WaterError::FoldOver(_))));
        let many = WaterParams {
            height: 0.0,
            waves: vec![wave(0.01, 0.1, Vec2::X, 10.0); MAX_WAVES + 1],
        };
        assert_eq!(
            many.validate(),
            Err(WaterError::TooManyWaves(MAX_WAVES + 1))
        );
        let bad_q = WaterParams {
            height: 0.0,
            waves: vec![wave(0.01, 1.5, Vec2::X, 10.0)],
        };
        assert_eq!(bad_q.validate(), Err(WaterError::Steepness(0)));
    }

    #[test]
    fn underwater_flag_is_exactly_below_the_surface() {
        let water = WaterParams {
            height: 2.0,
            waves: Vec::new(),
        };
        assert!(water.contains_camera_height(1.999));
        assert!(!water.contains_camera_height(2.0));
        assert!(!water.contains_camera_height(5.0));
        assert!(is_underwater(-3.0, 0.0));
    }

    #[test]
    fn underwater_fog_replaces_the_height_fog() {
        let a = AmbienceProfile::builtin("underwater").unwrap();
        let u = underwater_profile(&a);
        assert_eq!(u.fog.density, a.water.fog_density);
        assert_eq!(u.fog.falloff, 0.0);
        assert_eq!(u.fog.color, a.fog.color);
    }

    #[test]
    fn motes_stay_in_the_box_and_drift() {
        let cam = Vec3::new(10.0, -3.0, 4.0);
        for i in [0, 1, 17, 5999] {
            for t in [0.0, 3.3, 200.0] {
                let p = mote_position(i, t, cam);
                let d = (p - cam).abs().max_element();
                assert!(d <= 0.5 * MOTE_BOX + 1e-3, "{p}");
            }
        }
        assert_ne!(mote_position(3, 0.0, cam), mote_position(3, 5.0, cam));
    }

    #[test]
    fn gpu_block_packs_the_waves() {
        let a = AmbienceProfile::builtin("underwater").unwrap();
        let water = WaterParams {
            height: 4.0,
            waves: test_waves(0.5),
        };
        let g = GpuWater::new(&a.water, Some(&water), true);
        assert_eq!(g.params, [4.0, 1.0, a.water.caustics, 4.0]);
        assert_eq!(g.wave_amp[1][0], 0.15);
        let none = GpuWater::new(&a.water, None, false);
        assert_eq!(none.params[2], 0.0);
        assert_eq!(none.params[3], 0.0);
    }
}

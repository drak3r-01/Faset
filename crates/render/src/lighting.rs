//! Stylised lighting inputs: point lights, the per-frame lighting uniform
//! block (`faset-shaders/src/lighting.wgsl`) and its descriptor sets.
//!
//! The CPU functions here mirror the shader formulas and are what the unit
//! tests check.

use ash::vk;
use bytemuck::{Pod, Zeroable};
use faset_core::glam::{Mat4, Vec3};

use crate::ambience::AmbienceProfile;
use crate::context::Context;
use crate::deferred::FRAMES_IN_FLIGHT;
use crate::frame::FrameData;
use crate::shadow::{ShadowCascades, ShadowResources, CASCADE_COUNT};
use crate::sky::GpuSky;
use crate::weather::GpuWeather;
use crate::RenderError;

/// Point lights the shader reads; extra lights are dropped.
pub const MAX_POINT_LIGHTS: usize = 32;

/// An omnidirectional light with a finite range.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointLight {
    /// World position in metres.
    pub position: Vec3,
    /// Linear RGB (intensity included).
    pub color: Vec3,
    /// Range in metres: the light contributes nothing at `radius` and
    /// beyond.
    pub radius: f32,
}

/// Distance attenuation `(1 - (d / r)^2)^2`, exactly 0 for `d >= r` (and
/// for a non-positive radius).
pub fn point_light_attenuation(distance: f32, radius: f32) -> f32 {
    // Also rejects NaN distances.
    if distance.partial_cmp(&radius) != Some(std::cmp::Ordering::Less) {
        return 0.0;
    }
    let x = distance / radius;
    let k = 1.0 - x * x;
    k * k
}

/// Optical depth term of the exponential height fog along a ray of length
/// `d` with unit direction y component `ry`, starting at height `y0`:
/// `F = 1 - exp(-rho0 e^(-k (y0 - h0)) (1 - e^(-k d ry)) / (k ry))`.
/// With `x = k d ry`, the fraction is `d (1 - e^-x) / x`; below
/// `|x| = 0.01` its series `d (1 - x/2 + x^2/6)` replaces it, which also
/// covers `k = 0` and horizontal rays. Returns 0 when `density <= 0`.
pub fn height_fog(density: f32, falloff: f32, h0: f32, y0: f32, d: f32, ry: f32) -> f32 {
    if density <= 0.0 {
        return 0.0;
    }
    let x = falloff * d * ry;
    let path = if x.abs() < 1.0e-2 {
        d * (1.0 - x * (0.5 - x / 6.0))
    } else {
        (1.0 - (-x).exp()) / (falloff * ry)
    };
    1.0 - (-density * (-falloff * (y0 - h0)).exp() * path).exp()
}

/// One point light in the uniform block.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub(crate) struct GpuPointLight {
    /// xyz position, w radius.
    pub position_radius: [f32; 4],
    /// rgb colour, w unused.
    pub color: [f32; 4],
}

/// The lighting uniform block (std140, every member 16-byte aligned).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub(crate) struct GpuLighting {
    /// xyz camera world position.
    pub camera_pos: [f32; 4],
    /// xyz unit vector towards the sun.
    pub sun_dir: [f32; 4],
    /// rgb sun colour times intensity.
    pub sun_color: [f32; 4],
    pub ambient_ground: [f32; 4],
    pub ambient_sky: [f32; 4],
    /// rgb shadow multiplier, w unused.
    pub shadow: [f32; 4],
    /// x threshold, y softness, z rim power.
    pub ramp_rim: [f32; 4],
    /// rgb rim strength.
    pub rim_color: [f32; 4],
    /// rgb fog colour, w density.
    pub fog_color: [f32; 4],
    /// x falloff, y reference height.
    pub fog_params: [f32; 4],
    /// x number of point lights.
    pub counts: [u32; 4],
    pub lights: [GpuPointLight; MAX_POINT_LIGHTS],
    /// Clip-from-world of each sun shadow cascade, column-major.
    pub shadow_matrices: [[f32; 16]; CASCADE_COUNT],
    /// xyz far view depth of each cascade, w 1 when the sun casts shadows.
    pub shadow_splits: [f32; 4],
    /// xyz world size of one shadow texel of each cascade.
    pub shadow_texel: [f32; 4],
    /// x depth bias, y slope bias, z normal offset, in texels.
    pub shadow_bias: [f32; 4],
    /// Procedural sky: colours, stars, clouds and the camera data the sky
    /// pass needs (lot 2.6).
    pub sky: GpuSky,
    /// Weather, lightning and the shelter map header (lot 2.7).
    pub weather: GpuWeather,
}

/// Bytes of [`GpuLighting`].
pub(crate) const LIGHTING_SIZE: u64 = std::mem::size_of::<GpuLighting>() as u64;
/// Alignment of the block inside the frame's uniform ring (the largest
/// `minUniformBufferOffsetAlignment` Vulkan allows).
pub(crate) const LIGHTING_ALIGN: u64 = 256;

const _: () = assert!(
    std::mem::size_of::<GpuLighting>()
        == 11 * 16
            + MAX_POINT_LIGHTS * 32
            + CASCADE_COUNT * 64
            + 3 * 16
            + std::mem::size_of::<GpuSky>()
            + std::mem::size_of::<GpuWeather>()
);

fn rgb(c: [f32; 3]) -> [f32; 4] {
    [c[0], c[1], c[2], 0.0]
}

/// Builds the uniform block. Lights past [`MAX_POINT_LIGHTS`] are dropped;
/// one warning is logged when the list starts overflowing (`warned` keeps
/// that state between frames and clears once the list fits again).
/// `cascades` are the sun shadow cascades of the frame, `None` when the sun
/// casts no shadows. `sky` is the frame's sky block
/// ([`crate::sky::resolve`]); `ambience` is the resolved profile; `weather`
/// is the frame's weather block.
pub(crate) fn pack_lighting(
    ambience: &AmbienceProfile,
    lights: &[PointLight],
    camera_pos: Vec3,
    cascades: Option<&ShadowCascades>,
    sky: GpuSky,
    weather: GpuWeather,
    warned: &mut bool,
) -> GpuLighting {
    if lights.len() > MAX_POINT_LIGHTS {
        if !*warned {
            tracing::warn!(
                lights = lights.len(),
                max = MAX_POINT_LIGHTS,
                "too many point lights, the extra ones are dropped"
            );
        }
        *warned = true;
    } else {
        *warned = false;
    }
    let used = &lights[..lights.len().min(MAX_POINT_LIGHTS)];
    let mut gpu_lights = [GpuPointLight::zeroed(); MAX_POINT_LIGHTS];
    for (g, l) in gpu_lights.iter_mut().zip(used) {
        *g = GpuPointLight {
            position_radius: l.position.extend(l.radius).to_array(),
            color: l.color.extend(0.0).to_array(),
        };
    }
    let a = ambience;
    let sun_dir = Vec3::from_array(a.sun.direction).normalize_or(Vec3::Y);
    let sun_color = Vec3::from_array(a.sun.color) * a.sun.intensity;
    let (shadow_matrices, shadow_splits, shadow_texel) = match cascades {
        Some(c) => (
            c.matrices.map(|m| m.to_cols_array()),
            [c.splits[0], c.splits[1], c.splits[2], 1.0],
            [c.texel_size(0), c.texel_size(1), c.texel_size(2), 0.0],
        ),
        None => (
            [Mat4::IDENTITY.to_cols_array(); CASCADE_COUNT],
            [0.0; 4],
            [0.0; 4],
        ),
    };
    GpuLighting {
        camera_pos: camera_pos.extend(0.0).to_array(),
        sun_dir: sun_dir.extend(0.0).to_array(),
        sun_color: sun_color.extend(0.0).to_array(),
        ambient_ground: rgb(a.ambient.ground),
        ambient_sky: rgb(a.ambient.sky),
        shadow: rgb(a.ramp.shadow),
        ramp_rim: [a.ramp.threshold, a.ramp.softness, a.rim.power, 0.0],
        rim_color: rgb(a.rim.color),
        fog_color: [
            a.fog.color[0],
            a.fog.color[1],
            a.fog.color[2],
            a.fog.density,
        ],
        fog_params: [a.fog.falloff, a.fog.height, 0.0, 0.0],
        counts: [used.len() as u32, 0, 0, 0],
        lights: gpu_lights,
        shadow_matrices,
        shadow_splits,
        shadow_texel,
        shadow_bias: [
            a.shadow.depth_bias,
            a.shadow.slope_bias,
            a.shadow.normal_offset,
            0.0,
        ],
        sky,
        weather,
    }
}

/// Descriptor set 1: one dynamic uniform buffer binding per frame slot,
/// pointing at that frame's uniform ring.
pub(crate) struct LightingBindings {
    pub set_layout: vk::DescriptorSetLayout,
    pool: vk::DescriptorPool,
    pub sets: [vk::DescriptorSet; FRAMES_IN_FLIGHT],
}

impl LightingBindings {
    pub(crate) fn new(
        ctx: &Context,
        frames: &[FrameData],
        shadow: &ShadowResources,
        shelter: vk::Buffer,
    ) -> Result<Self, RenderError> {
        let device = &ctx.device;
        // The weather particle vertex shader reads the block and the shelter
        // map too.
        let binding = |index, ty, stages| {
            vk::DescriptorSetLayoutBinding::default()
                .binding(index)
                .descriptor_type(ty)
                .descriptor_count(1)
                .stage_flags(stages)
        };
        let fragment = vk::ShaderStageFlags::FRAGMENT;
        let both = vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT;
        let bindings = [
            binding(0, vk::DescriptorType::UNIFORM_BUFFER_DYNAMIC, both),
            binding(1, vk::DescriptorType::SAMPLED_IMAGE, fragment),
            binding(2, vk::DescriptorType::SAMPLER, fragment),
            binding(3, vk::DescriptorType::STORAGE_BUFFER, both),
        ];
        // SAFETY: valid device and create info.
        let set_layout = unsafe {
            device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                None,
            )
        }?;
        let frames_in_flight = FRAMES_IN_FLIGHT as u32;
        let sizes = [
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::UNIFORM_BUFFER_DYNAMIC,
                descriptor_count: frames_in_flight,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::SAMPLED_IMAGE,
                descriptor_count: frames_in_flight,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::SAMPLER,
                descriptor_count: frames_in_flight,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::STORAGE_BUFFER,
                descriptor_count: frames_in_flight,
            },
        ];
        // SAFETY: valid device and create info.
        let pool = match unsafe {
            device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(FRAMES_IN_FLIGHT as u32)
                    .pool_sizes(&sizes),
                None,
            )
        } {
            Ok(p) => p,
            Err(e) => {
                // SAFETY: unused layout.
                unsafe { device.destroy_descriptor_set_layout(set_layout, None) };
                return Err(e.into());
            }
        };
        let layouts = [set_layout; FRAMES_IN_FLIGHT];
        // SAFETY: the pool has room for these sets.
        let allocated = unsafe {
            device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(pool)
                    .set_layouts(&layouts),
            )
        };
        let allocated = match allocated {
            Ok(s) => s,
            Err(e) => {
                // SAFETY: unused pool and layout.
                unsafe {
                    device.destroy_descriptor_pool(pool, None);
                    device.destroy_descriptor_set_layout(set_layout, None);
                }
                return Err(e.into());
            }
        };
        let mut sets = [vk::DescriptorSet::null(); FRAMES_IN_FLIGHT];
        sets.copy_from_slice(&allocated);
        for (set, frame) in sets.iter().zip(frames) {
            let infos = [vk::DescriptorBufferInfo {
                buffer: frame.uniforms.buffer,
                offset: 0,
                range: LIGHTING_SIZE,
            }];
            let map = [vk::DescriptorImageInfo {
                sampler: vk::Sampler::null(),
                image_view: shadow.array_view,
                image_layout: vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL,
            }];
            let sampler = [vk::DescriptorImageInfo {
                sampler: shadow.sampler,
                image_view: vk::ImageView::null(),
                image_layout: vk::ImageLayout::UNDEFINED,
            }];
            let shelter_info = [vk::DescriptorBufferInfo {
                buffer: shelter,
                offset: 0,
                range: vk::WHOLE_SIZE,
            }];
            let writes = [
                vk::WriteDescriptorSet::default()
                    .dst_set(*set)
                    .dst_binding(3)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .buffer_info(&shelter_info),
                vk::WriteDescriptorSet::default()
                    .dst_set(*set)
                    .dst_binding(0)
                    .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER_DYNAMIC)
                    .buffer_info(&infos),
                vk::WriteDescriptorSet::default()
                    .dst_set(*set)
                    .dst_binding(1)
                    .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                    .image_info(&map),
                vk::WriteDescriptorSet::default()
                    .dst_set(*set)
                    .dst_binding(2)
                    .descriptor_type(vk::DescriptorType::SAMPLER)
                    .image_info(&sampler),
            ];
            // SAFETY: live set, buffer, view and sampler; the set is not in
            // use yet.
            unsafe { device.update_descriptor_sets(&writes, &[]) };
        }
        Ok(Self {
            set_layout,
            pool,
            sets,
        })
    }

    /// Points every frame's set at a new shelter map buffer. The GPU must be
    /// done with the sets (the device idle).
    pub(crate) fn set_shelter(&self, ctx: &Context, shelter: vk::Buffer) {
        let info = [vk::DescriptorBufferInfo {
            buffer: shelter,
            offset: 0,
            range: vk::WHOLE_SIZE,
        }];
        let writes: Vec<_> = self
            .sets
            .iter()
            .map(|set| {
                vk::WriteDescriptorSet::default()
                    .dst_set(*set)
                    .dst_binding(3)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .buffer_info(&info)
            })
            .collect();
        // SAFETY: live sets and buffer; the caller waited for the device.
        unsafe { ctx.device.update_descriptor_sets(&writes, &[]) };
    }

    pub(crate) fn destroy(&mut self, ctx: &Context) {
        // SAFETY: the device is idle; destroying the pool frees the sets.
        unsafe {
            ctx.device.destroy_descriptor_pool(self.pool, None);
            ctx.device
                .destroy_descriptor_set_layout(self.set_layout, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use tracing_subscriber::layer::{Context, SubscriberExt};
    use tracing_subscriber::Layer;

    use super::*;

    struct Warnings(Arc<AtomicUsize>);

    impl<S: tracing::Subscriber> Layer<S> for Warnings {
        fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
            if *event.metadata().level() == tracing::Level::WARN {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
    }

    fn light(k: usize) -> PointLight {
        PointLight {
            position: Vec3::new(k as f32, 1.0, 0.0),
            color: Vec3::splat(1.0 + k as f32),
            radius: 5.0,
        }
    }

    #[test]
    fn light_at_or_beyond_its_radius_contributes_exactly_zero() {
        for (d, r) in [
            (5.0, 5.0),
            (5.000_001, 5.0),
            (100.0, 2.0),
            (0.0, 0.0),
            (1.0, -1.0),
        ] {
            assert_eq!(point_light_attenuation(d, r), 0.0, "d = {d}, r = {r}");
        }
        assert_eq!(point_light_attenuation(0.0, 4.0), 1.0);
        assert_eq!(point_light_attenuation(2.0, 4.0), 0.5625);
        assert!(point_light_attenuation(4.999, 5.0) > 0.0);
        assert_eq!(point_light_attenuation(f32::NAN, 5.0), 0.0);
    }

    #[test]
    fn forty_lights_are_truncated_to_32_with_one_warning() {
        let count = Arc::new(AtomicUsize::new(0));
        let subscriber = tracing_subscriber::registry().with(Warnings(count.clone()));
        let _guard = tracing::subscriber::set_default(subscriber);
        let lights: Vec<PointLight> = (0..40).map(light).collect();
        let mut warned = false;
        let profile = AmbienceProfile::default();
        let gpu = pack_lighting(
            &profile,
            &lights,
            Vec3::ZERO,
            None,
            GpuSky::zeroed(),
            GpuWeather::zeroed(),
            &mut warned,
        );
        assert_eq!(gpu.counts[0], 32);
        assert_eq!(gpu.lights[31].position_radius, [31.0, 1.0, 0.0, 5.0]);
        assert_eq!(count.load(Ordering::SeqCst), 1);
        // Later frames with the same list do not warn again...
        pack_lighting(
            &profile,
            &lights,
            Vec3::ZERO,
            None,
            GpuSky::zeroed(),
            GpuWeather::zeroed(),
            &mut warned,
        );
        assert_eq!(count.load(Ordering::SeqCst), 1);
        // ...until the list fitted in between.
        let gpu = pack_lighting(
            &profile,
            &lights[..3],
            Vec3::ZERO,
            None,
            GpuSky::zeroed(),
            GpuWeather::zeroed(),
            &mut warned,
        );
        assert_eq!(gpu.counts[0], 3);
        assert_eq!(gpu.lights[3], GpuPointLight::zeroed());
        assert_eq!(count.load(Ordering::SeqCst), 1);
        pack_lighting(
            &profile,
            &lights,
            Vec3::ZERO,
            None,
            GpuSky::zeroed(),
            GpuWeather::zeroed(),
            &mut warned,
        );
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn shadow_block_carries_cascades_and_bias() {
        let profile = AmbienceProfile::builtin("day").unwrap();
        let camera = crate::Camera::default();
        let cascades = crate::fit_cascades(&camera, Vec3::Y, 0.1, 100.0);
        let mut warned = false;
        let gpu = pack_lighting(
            &profile,
            &[],
            Vec3::ZERO,
            Some(&cascades),
            GpuSky::zeroed(),
            GpuWeather::zeroed(),
            &mut warned,
        );
        assert_eq!(gpu.shadow_splits[3], 1.0);
        assert_eq!(gpu.shadow_splits[..3], cascades.splits);
        assert_eq!(gpu.shadow_matrices[1], cascades.matrices[1].to_cols_array());
        assert_eq!(gpu.shadow_texel[2], cascades.texel_size(2));
        assert_eq!(gpu.shadow_bias[..3], [1.0, 1.5, 1.0]);
        let off = pack_lighting(
            &profile,
            &[],
            Vec3::ZERO,
            None,
            GpuSky::zeroed(),
            GpuWeather::zeroed(),
            &mut warned,
        );
        assert_eq!(off.shadow_splits, [0.0; 4]);
    }

    #[test]
    fn fog_is_zero_without_density_and_grows_with_distance() {
        assert_eq!(height_fog(0.0, 0.2, 0.0, 3.0, 500.0, -0.1), 0.0);
        let near = height_fog(0.05, 0.1, 0.0, 2.0, 10.0, 0.0);
        let far = height_fog(0.05, 0.1, 0.0, 2.0, 100.0, 0.0);
        assert!(0.0 < near && near < far && far < 1.0);
        // Horizontal ray: closed form 1 - exp(-rho e^(-k y) d).
        let expected = 1.0 - (-0.05f32 * (-0.1f32 * 2.0).exp() * 10.0).exp();
        assert!((near - expected).abs() < 1e-6);
        // Both branches, on either side of |k d ry| = 0.01, match the closed
        // form evaluated in f64.
        let exact = |ry: f64| {
            let (rho, k, y, d) = (0.05f64, 0.1f64, 2.0f64, 10.0f64);
            1.0 - (-rho * (-k * y).exp() * (1.0 - (-k * d * ry).exp()) / (k * ry)).exp()
        };
        for ry in [0.0099, 0.0101, -0.0099, -0.0101, 0.3, -0.7] {
            let got = f64::from(height_fog(0.05, 0.1, 0.0, 2.0, 10.0, ry as f32));
            assert!((got - exact(ry)).abs() < 1e-6, "ry = {ry}");
        }
        // Looking up thins the fog, looking down thickens it.
        let up = height_fog(0.05, 0.1, 0.0, 2.0, 50.0, 0.5);
        let down = height_fog(0.05, 0.1, 0.0, 2.0, 50.0, -0.5);
        assert!(up < height_fog(0.05, 0.1, 0.0, 2.0, 50.0, 0.0) && down > up);
    }
}

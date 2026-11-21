//! Cascaded sun shadows: cascade fitting (CPU) and the depth-only pass
//! resources (three 2048 x 2048 `D32_SFLOAT` maps in one array image).
//!
//! Depth convention: reverse-Z like the main camera. Each cascade matrix is
//! orthographic and maps the point nearest to the sun to depth 1 and the
//! farthest to depth 0; the maps are cleared to 0 and drawn with
//! `GREATER_OR_EQUAL`, so a larger stored value is closer to the sun. A
//! fragment is lit when its (biased) depth is `>=` the stored one, which is
//! the comparison of the hardware sampler (`ref >= texel`).

use ash::vk;
use faset_core::glam::{Mat4, Vec3, Vec4};
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme};
use gpu_allocator::MemoryLocation;

use crate::camera::Camera;
use crate::context::Context;
use crate::cull::GpuInstance;
use crate::mesh::Vertex;
use crate::RenderError;

/// Number of cascades.
pub const CASCADE_COUNT: usize = 3;
/// Side of each cascade map, in texels.
pub const SHADOW_MAP_SIZE: u32 = 2048;
/// Blend between the logarithmic and the uniform split scheme.
pub const SPLIT_LAMBDA: f32 = 0.7;
/// Distance from the camera, in metres, up to which the sun casts shadows.
pub const SHADOW_DISTANCE: f32 = 120.0;
/// How far, in metres, a cascade volume extends towards the sun beyond its
/// bounding sphere so that tall casters outside the view still shadow it.
const CASTER_MARGIN: f32 = 256.0;

pub(crate) const SHADOW_FORMAT: vk::Format = vk::Format::D32_SFLOAT;

/// The three cascades of one frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShadowCascades {
    /// Clip-from-world of each cascade (orthographic, reverse-Z, texel
    /// snapped).
    pub matrices: [Mat4; CASCADE_COUNT],
    /// Far distance of each cascade along the camera's view axis, in metres.
    /// Strictly increasing; the last one is the shadow distance.
    pub splits: [f32; CASCADE_COUNT],
}

impl ShadowCascades {
    /// World size of one shadow texel of cascade `i`, in metres.
    pub fn texel_size(&self, i: usize) -> f32 {
        let row = self.matrices[i].row(0).truncate();
        // `row` is `right / radius`, so its length is `1 / radius`.
        2.0 / (row.length() * SHADOW_MAP_SIZE as f32)
    }
}

/// Split distances `d_1..d_3` between `near` and `far`:
/// `d_i = l n (f / n)^(i / N) + (1 - l) (n + i / N (f - n))`, `l = 0.7`;
/// the last split is exactly `far`.
pub fn cascade_splits(near: f32, far: f32) -> [f32; CASCADE_COUNT] {
    let n = near.max(1.0e-4);
    let f = far.max(n);
    let mut splits = [0.0; CASCADE_COUNT];
    for (k, split) in splits.iter_mut().enumerate() {
        let t = (k + 1) as f32 / CASCADE_COUNT as f32;
        let log = n * (f / n).powf(t);
        let uniform = n + t * (f - n);
        *split = (SPLIT_LAMBDA * log + (1.0 - SPLIT_LAMBDA) * uniform).clamp(n, f);
    }
    splits[CASCADE_COUNT - 1] = f;
    splits
}

/// Orthonormal light basis: `z` points towards the sun, so the light looks
/// along `-z` like a camera; `x` and `y` span the shadow map plane.
fn light_basis(sun_dir: Vec3) -> (Vec3, Vec3, Vec3) {
    let z = sun_dir.normalize_or(Vec3::Y);
    let up = if z.y.abs() > 0.99 { Vec3::X } else { Vec3::Y };
    let x = up.cross(z).normalize();
    let y = z.cross(x);
    (x, y, z)
}

/// Bounding sphere of the camera frustum slice between view depths `a` and
/// `b`: the distance of its centre from the camera along the view axis and
/// its radius. Depends on the projection only, never on the camera pose, so
/// the radius does not change when the camera turns.
fn slice_sphere(tan_x: f32, tan_y: f32, a: f32, b: f32) -> (f32, f32) {
    let s = tan_x * tan_x + tan_y * tan_y;
    // The centre that is equidistant from the near and far corners, or the
    // far plane's centre when that lies beyond the far plane.
    let centre = (0.5 * (a + b) * (s + 1.0)).min(b);
    let far = (b * b * s + (b - centre) * (b - centre)).sqrt();
    let near = (a * a * s + (centre - a) * (centre - a)).sqrt();
    (centre, far.max(near))
}

fn snap(v: f32, texel: f32) -> f32 {
    (v / texel).round() * texel
}

/// Fits three cascades to the view frustum of `camera` between `near` and
/// `far` metres, for light coming from `sun_dir` (towards the sun).
///
/// Each cascade is the bounding sphere of its frustum slice, so its size is
/// rotation invariant; its origin is rounded to whole shadow texels in all
/// three light-space axes, so the maps do not shimmer while the camera moves.
/// The field of view is read from the (symmetric) projection of `camera`.
pub fn fit_cascades(camera: &Camera, sun_dir: Vec3, near: f32, far: f32) -> ShadowCascades {
    let splits = cascade_splits(near, far);
    let tan_x = 1.0 / camera.proj.x_axis.x.abs().max(1.0e-6);
    let tan_y = 1.0 / camera.proj.y_axis.y.abs().max(1.0e-6);
    let world_from_view = camera.view.inverse();
    let (ax, ay, az) = light_basis(sun_dir);

    let mut matrices = [Mat4::IDENTITY; CASCADE_COUNT];
    let mut start = near.max(1.0e-4);
    for (matrix, &end) in matrices.iter_mut().zip(&splits) {
        let (distance, radius) = slice_sphere(tan_x, tan_y, start, end);
        start = end;
        let centre = world_from_view.transform_point3(Vec3::new(0.0, 0.0, -distance));
        let texel = 2.0 * radius / SHADOW_MAP_SIZE as f32;
        let sx = snap(centre.dot(ax), texel);
        let sy = snap(centre.dot(ay), texel);
        let sz = snap(centre.dot(az), texel);
        let depth_range = 2.0 * radius + CASTER_MARGIN;
        let row0 = (ax / radius).extend(-sx / radius);
        let row1 = (ay / radius).extend(-sy / radius);
        let row2 = (az / depth_range).extend((radius - sz) / depth_range);
        *matrix = Mat4::from_cols(row0, row1, row2, Vec4::W).transpose();
    }
    ShadowCascades { matrices, splits }
}

/// The depth-only pass: the array image of the three maps and the pipeline.
pub(crate) struct ShadowResources {
    pub image: vk::Image,
    allocation: Option<Allocation>,
    /// `2D_ARRAY` view of all cascades, sampled by the opaque shader.
    pub array_view: vk::ImageView,
    /// One 2D view per cascade, the depth attachment of its pass.
    pub layer_views: [vk::ImageView; CASCADE_COUNT],
    /// Nearest-filtered sampler with `GREATER_OR_EQUAL` comparison.
    pub sampler: vk::Sampler,
    pub layout: vk::PipelineLayout,
    pub pipeline: vk::Pipeline,
}

impl ShadowResources {
    pub(crate) fn new(ctx: &mut Context) -> Result<Self, RenderError> {
        let mut out = Self {
            image: vk::Image::null(),
            allocation: None,
            array_view: vk::ImageView::null(),
            layer_views: [vk::ImageView::null(); CASCADE_COUNT],
            sampler: vk::Sampler::null(),
            layout: vk::PipelineLayout::null(),
            pipeline: vk::Pipeline::null(),
        };
        if let Err(e) = out.create(ctx) {
            out.destroy(ctx);
            return Err(e);
        }
        Ok(out)
    }

    fn create(&mut self, ctx: &mut Context) -> Result<(), RenderError> {
        let device = &ctx.device;
        let info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(SHADOW_FORMAT)
            .extent(vk::Extent3D {
                width: SHADOW_MAP_SIZE,
                height: SHADOW_MAP_SIZE,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(CASCADE_COUNT as u32)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT | vk::ImageUsageFlags::SAMPLED)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        // SAFETY: valid device and create info.
        self.image = unsafe { device.create_image(&info, None) }?;
        // SAFETY: live image.
        let requirements = unsafe { device.get_image_memory_requirements(self.image) };
        let allocation = ctx.allocator.allocate(&AllocationCreateDesc {
            name: "shadow-cascades",
            requirements,
            location: MemoryLocation::GpuOnly,
            linear: false,
            allocation_scheme: AllocationScheme::GpuAllocatorManaged,
        })?;
        // SAFETY: the memory satisfies `requirements`; the image is unbound.
        let bound = unsafe {
            device.bind_image_memory(self.image, allocation.memory(), allocation.offset())
        };
        self.allocation = Some(allocation);
        bound?;

        let view = |ty, base, count| {
            let info = vk::ImageViewCreateInfo::default()
                .image(self.image)
                .view_type(ty)
                .format(SHADOW_FORMAT)
                .subresource_range(vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::DEPTH,
                    base_mip_level: 0,
                    level_count: 1,
                    base_array_layer: base,
                    layer_count: count,
                });
            // SAFETY: live, bound image with a matching format.
            unsafe { device.create_image_view(&info, None) }
        };
        self.array_view = view(vk::ImageViewType::TYPE_2D_ARRAY, 0, CASCADE_COUNT as u32)?;
        for (k, slot) in self.layer_views.iter_mut().enumerate() {
            *slot = view(vk::ImageViewType::TYPE_2D, k as u32, 1)?;
        }

        let sampler_info = vk::SamplerCreateInfo::default()
            .mag_filter(vk::Filter::NEAREST)
            .min_filter(vk::Filter::NEAREST)
            .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
            .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_BORDER)
            .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_BORDER)
            .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_BORDER)
            .compare_enable(true)
            .compare_op(vk::CompareOp::GREATER_OR_EQUAL)
            // Depth 0 is the far end: outside a map everything is lit.
            .border_color(vk::BorderColor::FLOAT_OPAQUE_BLACK);
        // SAFETY: valid device and create info.
        self.sampler = unsafe { device.create_sampler(&sampler_info, None) }?;

        // SAFETY: valid device and create info (no sets, no push constants).
        self.layout = unsafe {
            device.create_pipeline_layout(&vk::PipelineLayoutCreateInfo::default(), None)
        }?;
        self.pipeline = create_pipeline(device, self.layout)?;
        Ok(())
    }

    /// Destroys every handle that exists. The device must be idle.
    pub(crate) fn destroy(&mut self, ctx: &mut Context) {
        // SAFETY: the caller waited for the device to be idle; destroying
        // null handles is a no-op.
        unsafe {
            ctx.device.destroy_pipeline(self.pipeline, None);
            ctx.device.destroy_pipeline_layout(self.layout, None);
            ctx.device.destroy_sampler(self.sampler, None);
            for v in self.layer_views {
                ctx.device.destroy_image_view(v, None);
            }
            ctx.device.destroy_image_view(self.array_view, None);
            ctx.device.destroy_image(self.image, None);
        }
        self.pipeline = vk::Pipeline::null();
        self.layout = vk::PipelineLayout::null();
        self.sampler = vk::Sampler::null();
        self.layer_views = [vk::ImageView::null(); CASCADE_COUNT];
        self.array_view = vk::ImageView::null();
        self.image = vk::Image::null();
        if let Some(a) = self.allocation.take() {
            if let Err(e) = ctx.allocator.free(a) {
                tracing::warn!("failed to free shadow map memory: {e}");
            }
        }
    }

    /// Barrier over all cascade layers.
    ///
    /// # Safety
    /// `cmd` must be recording.
    #[allow(clippy::too_many_arguments)]
    pub(crate) unsafe fn barrier(
        &self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        src_stage: vk::PipelineStageFlags,
        dst_stage: vk::PipelineStageFlags,
        old: vk::ImageLayout,
        new: vk::ImageLayout,
        src_access: vk::AccessFlags,
        dst_access: vk::AccessFlags,
    ) {
        let barrier = vk::ImageMemoryBarrier::default()
            .src_access_mask(src_access)
            .dst_access_mask(dst_access)
            .old_layout(old)
            .new_layout(new)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(self.image)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::DEPTH,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: CASCADE_COUNT as u32,
            });
        // SAFETY: forwarded from the caller; the image is live.
        unsafe {
            device.cmd_pipeline_barrier(
                cmd,
                src_stage,
                dst_stage,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[barrier],
            );
        }
    }
}

fn create_pipeline(
    device: &ash::Device,
    layout: vk::PipelineLayout,
) -> Result<vk::Pipeline, RenderError> {
    // SAFETY: SPIR-V produced and validated by naga.
    let module = unsafe {
        device.create_shader_module(
            &vk::ShaderModuleCreateInfo::default().code(faset_shaders::SHADOW_VS),
            None,
        )
    }?;
    let stages = [vk::PipelineShaderStageCreateInfo::default()
        .stage(vk::ShaderStageFlags::VERTEX)
        .module(module)
        .name(c"vs_main")];
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
    // Position, then the four columns of the clip-from-local matrix (the
    // cascade matrix times the world matrix, see `cull_and_batch`).
    let mut attributes = vec![vk::VertexInputAttributeDescription {
        location: 0,
        binding: 0,
        format: vk::Format::R32G32B32_SFLOAT,
        offset: 0,
    }];
    for column in 0..4u32 {
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
    // Both faces are drawn: robust for open and mirrored meshes; the bias
    // lives in the lookup, not in the rasteriser.
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
    let blend = vk::PipelineColorBlendStateCreateInfo::default();
    let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
    let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
    let mut rendering =
        vk::PipelineRenderingCreateInfo::default().depth_attachment_format(SHADOW_FORMAT);
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
        unsafe { device.create_graphics_pipelines(vk::PipelineCache::null(), &[info], None) };
    // SAFETY: the module may be destroyed once pipelines are created.
    unsafe { device.destroy_shader_module(module, None) };
    let pipelines = pipelines.map_err(|(_, e)| e)?;
    Ok(pipelines[0])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::perspective_reverse_z;
    use faset_core::glam::camera::rh::view::look_at_mat4;

    const SUN: Vec3 = Vec3::new(0.4, 0.8, 0.3);

    fn camera_at(eye: Vec3, forward: Vec3) -> Camera {
        Camera::new(
            look_at_mat4(eye, eye + forward, Vec3::Y),
            perspective_reverse_z(60f32.to_radians(), 16.0 / 9.0, 0.1),
        )
    }

    /// Light-space `x` axis of cascade matrix `m`.
    fn axis_x(m: &Mat4) -> Vec3 {
        m.row(0).truncate().normalize()
    }

    #[test]
    fn splits_are_strictly_increasing_inside_near_far() {
        for (near, far) in [(0.1, 120.0), (0.5, 50.0), (0.1, 1000.0), (1.0, 4.0)] {
            let cascades = fit_cascades(&camera_at(Vec3::ZERO, Vec3::NEG_Z), SUN, near, far);
            let s = cascades.splits;
            assert!(near < s[0] && s[0] < s[1] && s[1] < s[2], "{s:?}");
            assert!(s.iter().all(|d| (near..=far).contains(d)), "{s:?}");
            assert_eq!(s[2], far);
        }
    }

    #[test]
    fn splits_follow_the_practical_scheme() {
        let (n, f) = (0.1f32, 120.0f32);
        let s = cascade_splits(n, f);
        for (i, d) in s.iter().take(2).enumerate() {
            let t = (i + 1) as f32 / 3.0;
            let expected = 0.7 * n * (f / n).powf(t) + 0.3 * (n + t * (f - n));
            assert!((d - expected).abs() < 1.0e-4, "d{} = {d}", i + 1);
        }
    }

    #[test]
    fn sphere_radius_does_not_depend_on_the_camera_rotation() {
        let eye = Vec3::new(3.0, 2.0, -5.0);
        let a = fit_cascades(&camera_at(eye, Vec3::NEG_Z), SUN, 0.1, SHADOW_DISTANCE);
        let turned = Vec3::new(0.8, -0.3, 0.5).normalize();
        let b = fit_cascades(&camera_at(eye, turned), SUN, 0.1, SHADOW_DISTANCE);
        for i in 0..CASCADE_COUNT {
            assert_eq!(a.texel_size(i), b.texel_size(i), "cascade {i}");
            assert!(a.texel_size(i) > 0.0);
        }
        // And the maps do cover different parts of the world.
        assert_ne!(a.matrices, b.matrices);
    }

    /// A camera whose cascade-0 sphere centre sits at the given fractional
    /// texel position along light-space x.
    fn camera_with_fraction(frac: f32) -> (Camera, Vec3, f32) {
        let forward = Vec3::NEG_Z;
        let probe = fit_cascades(&camera_at(Vec3::ZERO, forward), SUN, 0.1, SHADOW_DISTANCE);
        let texel = probe.texel_size(0);
        let ax = axis_x(&probe.matrices[0]);
        // Unsnapped centre of cascade 0 for a camera at the origin.
        let tan_y = 1.0 / camera_at(Vec3::ZERO, forward).proj.y_axis.y;
        let tan_x = 1.0 / camera_at(Vec3::ZERO, forward).proj.x_axis.x;
        let (distance, _) = slice_sphere(tan_x, tan_y, 0.1, probe.splits[0]);
        let centre_x = Vec3::new(0.0, 0.0, -distance).dot(ax);
        let shift = (10.0 + frac - (centre_x / texel).rem_euclid(1.0)) * texel;
        (camera_at(ax * shift, forward), ax, texel)
    }

    #[test]
    fn moving_less_than_a_texel_keeps_the_matrix() {
        let (camera, ax, texel) = camera_with_fraction(0.25);
        let base = fit_cascades(&camera, SUN, 0.1, SHADOW_DISTANCE);
        let eye = camera.view.inverse().w_axis.truncate();
        for step in [-0.2f32, -0.1, 0.05, 0.2] {
            let moved = fit_cascades(
                &camera_at(eye + ax * step * texel, Vec3::NEG_Z),
                SUN,
                0.1,
                SHADOW_DISTANCE,
            );
            assert_eq!(moved.matrices[0], base.matrices[0], "step {step}");
        }
    }

    #[test]
    fn moving_k_texels_shifts_the_matrix_by_k_texels() {
        let (camera, ax, texel) = camera_with_fraction(0.25);
        let base = fit_cascades(&camera, SUN, 0.1, SHADOW_DISTANCE);
        let eye = camera.view.inverse().w_axis.truncate();
        for k in [1.0f32, 2.0, -3.0, 7.0] {
            let moved = fit_cascades(
                &camera_at(eye + ax * k * texel, Vec3::NEG_Z),
                SUN,
                0.1,
                SHADOW_DISTANCE,
            );
            let (a, b) = (base.matrices[0], moved.matrices[0]);
            // A fixed point moves by `-k` texels in map space, which is
            // `-2 k / SIZE` in NDC x; everything else is unchanged.
            let expected = -2.0 * k / SHADOW_MAP_SIZE as f32;
            let shift = b.w_axis.x - a.w_axis.x;
            assert!((shift - expected).abs() < 1.0e-5, "k = {k}: {shift}");
            let mut rest = b;
            rest.w_axis.x = a.w_axis.x;
            for (p, q) in rest.to_cols_array().iter().zip(a.to_cols_array()) {
                assert!((p - q).abs() < 1.0e-6, "k = {k}");
            }
        }
    }

    #[test]
    fn cascades_enclose_their_frustum_slice_in_depth_zero_to_one() {
        let camera = camera_at(Vec3::new(1.0, 2.0, 3.0), Vec3::new(0.3, -0.1, -1.0));
        let cascades = fit_cascades(&camera, SUN, 0.1, SHADOW_DISTANCE);
        let world_from_view = camera.view.inverse();
        let (tan_x, tan_y) = (1.0 / camera.proj.x_axis.x, 1.0 / camera.proj.y_axis.y.abs());
        let mut start = 0.1;
        for (m, &end) in cascades.matrices.iter().zip(&cascades.splits) {
            for d in [start, end] {
                for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                    let p = world_from_view.transform_point3(Vec3::new(
                        sx * tan_x * d,
                        sy * tan_y * d,
                        -d,
                    ));
                    let ndc = m.project_point3(p);
                    assert!(ndc.x.abs() <= 1.0 + 1.0e-4 && ndc.y.abs() <= 1.0 + 1.0e-4);
                    assert!((0.0..=1.0).contains(&ndc.z), "depth {}", ndc.z);
                }
            }
            start = end;
        }
    }

    #[test]
    fn points_nearer_to_the_sun_get_larger_depth() {
        let cascades = fit_cascades(&camera_at(Vec3::ZERO, Vec3::NEG_Z), SUN, 0.1, 50.0);
        let p = Vec3::new(0.0, 0.0, -3.0);
        let toward_sun = p + SUN.normalize() * 2.0;
        let m = cascades.matrices[0];
        assert!(m.project_point3(toward_sun).z > m.project_point3(p).z);
    }
}

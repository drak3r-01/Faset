//! Instanced vegetation (lot 6.11): thousands of plants for a few draw calls.
//!
//! [`Renderer::set_foliage`](crate::Renderer::set_foliage) takes the plant
//! kinds (meshes by level of detail, wind sway, shadow casting) and every
//! plant of the map as a [`FoliageInstance`]. The plants are sorted into
//! square cells of [`FOLIAGE_CELL_M`] metres and, inside a cell, by kind;
//! their 32-byte instances go once into a static GPU buffer. Each frame the
//! CPU only looks at the cells: frustum culling of the cell bounds, a level
//! of detail per cell from the distance between the camera and the cell, and
//! one instanced draw per visible (cell, kind). The vertex shader
//! (`foliage.wgsl`) builds each plant's transform, bends it in the wind and
//! projects it; the fragment shader dissolves it with an ordered dither over
//! the last [`FADE_BAND`] of its draw distance, so plants never pop out.
//!
//! Decisions of lot 6.11:
//! - culling: chunked on the CPU (cells of 16 m), not on the GPU: a map has
//!   a few hundred cells, so the per-frame cost does not grow with the plants;
//! - levels of detail: chosen per cell with the cell's nearest point, so a
//!   plant is never drawn coarser than its own distance asks;
//! - the plants casting shadows are drawn into the sun cascades with the
//!   view's levels of detail, within the shadow distance;
//! - wind: a sway proportional to the squared height above the base, along
//!   the weather wind (a light breeze when there is none), two sines with a
//!   per-plant phase ([`sway_offset`]); motion vectors ignore the sway.

use ash::vk;
use bytemuck::{Pod, Zeroable};
use faset_core::glam::{Mat4, Vec3};

use crate::cull::{DrawBatch, Frustum};
use crate::gpu::GpuBuffer;
use crate::mesh::{Aabb, MeshId, MeshRange, Vertex};
use crate::opaque::{DEPTH_FORMAT, HDR_FORMAT, MOTION_FORMAT, NORMAL_FORMAT};
use crate::shadow::SHADOW_FORMAT;
use crate::RenderError;

/// Side of a culling cell, metres.
pub const FOLIAGE_CELL_M: f32 = 16.0;
/// Share of the draw distance over which a plant dissolves (`foliage.wgsl`).
pub const FADE_BAND: f32 = 0.2;
/// Most plants one foliage layer holds.
pub const MAX_FOLIAGE_INSTANCES: usize = 1 << 20;

/// One kind of plant: a species in one shape variant.
#[derive(Debug, Clone, PartialEq)]
pub struct FoliageKind {
    /// Meshes by level of detail, finest first, each with the distance up to
    /// which it is drawn (increasing). The last distance is where the plant
    /// has faded out.
    pub lods: Vec<(MeshId, f32)>,
    /// Wind sway: horizontal offset per squared metre of height at a
    /// nominal breeze (see [`sway_offset`]); 0 is rigid.
    pub sway: f32,
    /// Whether the plants cast sun shadows.
    pub casts_shadow: bool,
}

impl FoliageKind {
    /// Draw distance of the kind (where it has faded out).
    #[must_use]
    pub fn fade_end(&self) -> f32 {
        self.lods.last().map_or(0.0, |l| l.1)
    }

    /// Mesh to draw at `distance` metres, if any.
    #[must_use]
    pub fn lod_at(&self, distance: f32) -> Option<MeshId> {
        self.lods
            .iter()
            .find(|(_, max)| distance < *max)
            .map(|(m, _)| *m)
    }
}

/// One plant.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FoliageInstance {
    /// Index in the kinds given with the instances.
    pub kind: u32,
    /// Base of the plant, metres.
    pub position: Vec3,
    /// Rotation about +Y, radians.
    pub yaw: f32,
    /// Uniform scale.
    pub scale: f32,
}

/// Horizontal wind offset `(x, z)` of a point `height` metres above the base
/// of a plant with sway `sway` and wind phase `phase` at `time` seconds, under
/// the wind `wind` (m/s, x-z). Mirrors `sway_offset` of `foliage.wgsl`.
#[must_use]
pub fn sway_offset(height: f32, sway: f32, phase: f32, time: f32, wind: [f32; 2]) -> [f32; 2] {
    let speed = wind[0].hypot(wind[1]);
    let dir = if speed > 0.01 {
        [wind[0] / speed, wind[1] / speed]
    } else {
        [std::f32::consts::FRAC_1_SQRT_2; 2]
    };
    let strength = 1.0 + speed.min(20.0) * 0.15;
    let wave = 0.6 * (time * 1.7 + phase).sin() + 0.25 * (time * 3.1 + phase * 1.3).sin() + 0.35;
    let k = sway * strength * wave * height * height;
    [dir[0] * k, dir[1] * k]
}

/// Largest sway offset of a point `height` metres high, any time and wind.
fn sway_bound(sway: f32, height: f32) -> f32 {
    // |wave| <= 1.2, strength <= 4.
    sway.abs() * 4.0 * 1.2 * height * height
}

/// Per-plant instance data read by `foliage.wgsl` (32 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub(crate) struct GpuPlant {
    /// xyz base, w yaw.
    pub base_yaw: [f32; 4],
    /// x scale, y sway, z fade end, w wind phase.
    pub params: [f32; 4],
}

const _: () = assert!(std::mem::size_of::<GpuPlant>() == 32);

/// Consecutive instances of one kind in a cell.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Run {
    kind: u32,
    first: u32,
    count: u32,
}

/// One culling cell: world bounds of its plants and its runs, by kind.
#[derive(Debug, Clone, PartialEq)]
struct Cell {
    bounds: Aabb,
    runs: Vec<Run>,
}

/// What a frame draws of the foliage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct FoliageCount {
    pub draws: u32,
    pub instances: u32,
}

/// The CPU side of a foliage layer: kinds, cells and the instance data in
/// cell then kind order.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FoliageLayout {
    kinds: Vec<FoliageKind>,
    cells: Vec<Cell>,
    pub gpu: Vec<GpuPlant>,
}

#[allow(clippy::cast_possible_truncation)]
fn cell_of(v: f32) -> i32 {
    (v / FOLIAGE_CELL_M).floor() as i32
}

/// Wind phase of a plant from its position (deterministic).
fn phase_of(p: Vec3) -> f32 {
    let h = (p.x * 12.9898 + p.z * 78.233).sin() * 43_758.547;
    (h - h.floor()) * std::f32::consts::TAU
}

impl FoliageLayout {
    /// Sorts `instances` into cells. `meshes` are the arena's meshes (for
    /// the bounds of each kind).
    ///
    /// # Errors
    /// [`RenderError::InvalidFoliage`] for an unknown kind or mesh, a kind
    /// without levels of detail, non-increasing distances, too many plants or
    /// a non-finite plant.
    pub(crate) fn new(
        kinds: Vec<FoliageKind>,
        instances: &[FoliageInstance],
        meshes: &[MeshRange],
    ) -> Result<Self, RenderError> {
        let bad = |m: String| Err(RenderError::InvalidFoliage(m));
        if instances.len() > MAX_FOLIAGE_INSTANCES {
            return bad(format!(
                "{} plants, at most {MAX_FOLIAGE_INSTANCES}",
                instances.len()
            ));
        }
        // Local bounds of each kind: every level of detail, plus the sway.
        let mut local = Vec::with_capacity(kinds.len());
        for (k, kind) in kinds.iter().enumerate() {
            if kind.lods.is_empty() {
                return bad(format!("plant kind {k} has no level of detail"));
            }
            if kind.lods.windows(2).any(|w| w[0].1 >= w[1].1) || kind.lods[0].1 <= 0.0 {
                return bad(format!("plant kind {k}: distances must increase from > 0"));
            }
            let mut b: Option<Aabb> = None;
            for (mesh, _) in &kind.lods {
                let Some(r) = meshes.get(mesh.0 as usize) else {
                    return bad(format!("plant kind {k} uses unknown mesh {}", mesh.0));
                };
                b = Some(match b {
                    None => r.bounds,
                    Some(a) => Aabb {
                        min: a.min.min(r.bounds.min),
                        max: a.max.max(r.bounds.max),
                    },
                });
            }
            let b = b.expect("at least one level of detail");
            let m = sway_bound(kind.sway, b.max.y.max(0.0));
            local.push(Aabb {
                min: b.min - Vec3::new(m, 0.0, m),
                max: b.max + Vec3::new(m, 0.0, m),
            });
        }
        let mut order: Vec<(i32, i32, u32, u32)> = Vec::with_capacity(instances.len());
        for (i, p) in instances.iter().enumerate() {
            if p.kind as usize >= kinds.len() {
                return bad(format!("plant {i} has unknown kind {}", p.kind));
            }
            if !p.position.is_finite() || !p.yaw.is_finite() || !p.scale.is_finite() {
                return bad(format!("plant {i} is not finite"));
            }
            order.push((
                cell_of(p.position.x),
                cell_of(p.position.z),
                p.kind,
                i as u32,
            ));
        }
        order.sort_unstable();
        let mut cells: Vec<Cell> = Vec::new();
        let mut gpu = Vec::with_capacity(instances.len());
        let mut current: Option<(i32, i32)> = None;
        for &(cx, cz, kind, i) in &order {
            let p = &instances[i as usize];
            if current != Some((cx, cz)) {
                current = Some((cx, cz));
                cells.push(Cell {
                    bounds: Aabb {
                        min: Vec3::splat(f32::INFINITY),
                        max: Vec3::splat(f32::NEG_INFINITY),
                    },
                    runs: Vec::new(),
                });
            }
            let cell = cells.last_mut().expect("pushed above");
            let first = gpu.len() as u32;
            match cell.runs.last_mut() {
                Some(r) if r.kind == kind => r.count += 1,
                _ => cell.runs.push(Run {
                    kind,
                    first,
                    count: 1,
                }),
            }
            let world = Mat4::from_translation(p.position)
                * Mat4::from_rotation_y(p.yaw)
                * Mat4::from_scale(Vec3::splat(p.scale.abs()));
            let b = local[kind as usize].transformed(&world);
            cell.bounds.min = cell.bounds.min.min(b.min);
            cell.bounds.max = cell.bounds.max.max(b.max);
            let k = &kinds[kind as usize];
            gpu.push(GpuPlant {
                base_yaw: [p.position.x, p.position.y, p.position.z, p.yaw],
                params: [p.scale, k.sway, k.fade_end(), phase_of(p.position)],
            });
        }
        Ok(Self { kinds, cells, gpu })
    }

    /// Number of plants.
    pub(crate) fn len(&self) -> usize {
        self.gpu.len()
    }

    /// Appends the draws of the plants seen through `view_proj`, at the
    /// levels of detail of a camera at `eye`, to `out`. With `shadow` set
    /// (the shadow distance), only the plants casting shadows within it.
    pub(crate) fn draws(
        &self,
        view_proj: &Mat4,
        eye: Vec3,
        meshes: &[MeshRange],
        shadow: Option<f32>,
        out: &mut Vec<DrawBatch>,
    ) -> FoliageCount {
        let frustum = Frustum::from_view_proj(view_proj);
        let mut count = FoliageCount::default();
        for cell in &self.cells {
            let b = &cell.bounds;
            if !frustum.intersects_box(b.center(), b.half_extents()) {
                continue;
            }
            let distance = eye.clamp(b.min, b.max).distance(eye);
            if shadow.is_some_and(|s| distance >= s) {
                continue;
            }
            for run in &cell.runs {
                let kind = &self.kinds[run.kind as usize];
                if shadow.is_some() && !kind.casts_shadow {
                    continue;
                }
                let Some(mesh) = kind.lod_at(distance) else {
                    continue;
                };
                let Some(r) = meshes.get(mesh.0 as usize) else {
                    continue;
                };
                if r.index_count == 0 {
                    continue;
                }
                out.push(DrawBatch {
                    index_count: r.index_count,
                    first_index: r.first_index,
                    base_vertex: r.base_vertex,
                    first_instance: run.first,
                    instance_count: run.count,
                });
                count.draws += 1;
                count.instances += run.count;
            }
        }
        count
    }
}

/// A foliage layer on the GPU: the layout and its static instance buffer
/// (none when there are no plants).
pub(crate) struct FoliageLayer {
    pub layout: FoliageLayout,
    pub buffer: Option<GpuBuffer>,
}

/// The vegetation pipelines: the opaque-pass one and the shadow-cascade one.
/// Both use the opaque pipeline layout (material table, lighting block,
/// 16 bytes of immediates).
pub(crate) struct FoliagePipelines {
    pub pipeline: vk::Pipeline,
    pub shadow: vk::Pipeline,
}

impl FoliagePipelines {
    pub(crate) fn new(
        device: &ash::Device,
        layout: vk::PipelineLayout,
    ) -> Result<Self, RenderError> {
        let pipeline = create_pipeline(device, layout, false)?;
        match create_pipeline(device, layout, true) {
            Ok(shadow) => Ok(Self { pipeline, shadow }),
            Err(e) => {
                // SAFETY: unused pipeline.
                unsafe { device.destroy_pipeline(pipeline, None) };
                Err(e)
            }
        }
    }

    pub(crate) fn destroy(&mut self, device: &ash::Device) {
        // SAFETY: the device is idle.
        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline(self.shadow, None);
        }
    }
}

fn create_pipeline(
    device: &ash::Device,
    layout: vk::PipelineLayout,
    shadow: bool,
) -> Result<vk::Pipeline, RenderError> {
    let module = |words: &[u32]| {
        // SAFETY: SPIR-V produced and validated by naga.
        unsafe {
            device.create_shader_module(&vk::ShaderModuleCreateInfo::default().code(words), None)
        }
    };
    let vs = module(if shadow {
        faset_shaders::FOLIAGE_VS_SHADOW
    } else {
        faset_shaders::FOLIAGE_VS_MAIN
    })?;
    let fs = if shadow {
        None
    } else {
        match module(faset_shaders::FOLIAGE_FS) {
            Ok(m) => Some(m),
            Err(e) => {
                // SAFETY: unused module.
                unsafe { device.destroy_shader_module(vs, None) };
                return Err(e.into());
            }
        }
    };
    let mut stages = vec![vk::PipelineShaderStageCreateInfo::default()
        .stage(vk::ShaderStageFlags::VERTEX)
        .module(vs)
        .name(if shadow { c"vs_shadow" } else { c"vs_main" })];
    if let Some(fs) = fs {
        stages.push(
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .module(fs)
                .name(c"fs_main"),
        );
    }
    let bindings = [
        vk::VertexInputBindingDescription {
            binding: 0,
            stride: std::mem::size_of::<Vertex>() as u32,
            input_rate: vk::VertexInputRate::VERTEX,
        },
        vk::VertexInputBindingDescription {
            binding: 1,
            stride: std::mem::size_of::<GpuPlant>() as u32,
            input_rate: vk::VertexInputRate::INSTANCE,
        },
    ];
    let attribute = |location, binding, format, offset| vk::VertexInputAttributeDescription {
        location,
        binding,
        format,
        offset,
    };
    let attributes = [
        attribute(0, 0, vk::Format::R32G32B32_SFLOAT, 0),
        attribute(1, 0, vk::Format::R8G8B8A8_SNORM, 12),
        attribute(2, 0, vk::Format::R8G8B8A8_UNORM, 16),
        attribute(3, 1, vk::Format::R32G32B32A32_SFLOAT, 0),
        attribute(4, 1, vk::Format::R32G32B32A32_SFLOAT, 16),
    ];
    let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
        .vertex_binding_descriptions(&bindings)
        .vertex_attribute_descriptions(&attributes);
    let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
        .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
    let viewport = vk::PipelineViewportStateCreateInfo::default()
        .viewport_count(1)
        .scissor_count(1);
    // Closed meshes: back faces are culled in the view; the cascades draw
    // both faces like the static shadow casters.
    let raster = vk::PipelineRasterizationStateCreateInfo::default()
        .polygon_mode(vk::PolygonMode::FILL)
        .cull_mode(if shadow {
            vk::CullModeFlags::NONE
        } else {
            vk::CullModeFlags::BACK
        })
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
    let blend = if shadow {
        vk::PipelineColorBlendStateCreateInfo::default()
    } else {
        vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachments)
    };
    let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
    let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
    let color_formats = [HDR_FORMAT, MOTION_FORMAT, NORMAL_FORMAT];
    let mut rendering = if shadow {
        vk::PipelineRenderingCreateInfo::default().depth_attachment_format(SHADOW_FORMAT)
    } else {
        vk::PipelineRenderingCreateInfo::default()
            .color_attachment_formats(&color_formats)
            .depth_attachment_format(DEPTH_FORMAT)
    };
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
    // SAFETY: modules may be destroyed once pipelines are created.
    unsafe {
        device.destroy_shader_module(vs, None);
        if let Some(fs) = fs {
            device.destroy_shader_module(fs, None);
        }
    }
    let pipelines = pipelines.map_err(|(_, e)| e)?;
    Ok(pipelines[0])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::perspective_reverse_z;

    fn meshes() -> Vec<MeshRange> {
        // Mesh 0 and 1: a 1 m tall plant (fine, coarse); mesh 2: a tree.
        let unit = |k: u32, h: f32| MeshRange {
            first_index: k * 36,
            index_count: 36 - k * 6,
            base_vertex: (k * 24) as i32,
            bounds: Aabb {
                min: Vec3::new(-0.5, 0.0, -0.5),
                max: Vec3::new(0.5, h, 0.5),
            },
        };
        vec![unit(0, 1.0), unit(1, 1.0), unit(2, 5.0)]
    }

    fn kinds() -> Vec<FoliageKind> {
        vec![
            FoliageKind {
                lods: vec![(MeshId(0), 20.0), (MeshId(1), 50.0)],
                sway: 0.3,
                casts_shadow: false,
            },
            FoliageKind {
                lods: vec![(MeshId(2), 150.0)],
                sway: 0.01,
                casts_shadow: true,
            },
        ]
    }

    /// A field of `n` x `n` plants 1 m apart around the origin, alternating
    /// kinds, plus trees every 8 m.
    fn field(n: i32) -> Vec<FoliageInstance> {
        let mut out = Vec::new();
        for j in 0..n {
            for i in 0..n {
                let p = Vec3::new((i - n / 2) as f32, 0.0, (j - n / 2) as f32);
                let tree = i % 8 == 0 && j % 8 == 0;
                out.push(FoliageInstance {
                    kind: u32::from(tree),
                    position: p,
                    yaw: (i * 7 + j) as f32,
                    scale: 1.0,
                });
            }
        }
        out
    }

    fn camera(eye: Vec3, target: Vec3) -> Mat4 {
        perspective_reverse_z(70f32.to_radians(), 16.0 / 9.0, 0.1)
            * faset_core::glam::camera::rh::view::look_at_mat4(eye, target, Vec3::Y)
    }

    #[test]
    fn plants_are_grouped_by_cell_then_kind() {
        let plants = field(100);
        let layout = FoliageLayout::new(kinds(), &plants, &meshes()).unwrap();
        assert_eq!(layout.len(), 10_000);
        // x and z from -50 to 49 m, 16 m cells: 8 x 8 cells.
        assert_eq!(layout.cells.len(), 64);
        let mut next = 0;
        for cell in &layout.cells {
            let mut kinds_seen = Vec::new();
            for run in &cell.runs {
                assert_eq!(run.first, next, "runs are contiguous");
                next += run.count;
                assert!(!kinds_seen.contains(&run.kind), "one run per kind");
                kinds_seen.push(run.kind);
                for g in &layout.gpu[run.first as usize..(run.first + run.count) as usize] {
                    let p = Vec3::new(g.base_yaw[0], g.base_yaw[1], g.base_yaw[2]);
                    assert!(
                        p.cmpge(cell.bounds.min).all() && p.cmple(cell.bounds.max).all(),
                        "a plant lies in its cell's bounds"
                    );
                }
            }
        }
        assert_eq!(next, 10_000);
        // Instance data carries the kind's sway and fade distance.
        let tree = layout
            .gpu
            .iter()
            .find(|g| g.params[1] == 0.01)
            .expect("trees");
        assert_eq!(tree.params[2], 150.0);
    }

    #[test]
    fn ten_thousand_plants_take_a_few_draws_and_far_cells_are_culled() {
        let plants = field(100);
        let layout = FoliageLayout::new(kinds(), &plants, &meshes()).unwrap();
        let mut draws = Vec::new();
        let vp = camera(Vec3::new(0.0, 1.7, 60.0), Vec3::new(0.0, 1.0, 0.0));
        let n = layout.draws(&vp, Vec3::new(0.0, 1.7, 60.0), &meshes(), None, &mut draws);
        assert_eq!(n.draws as usize, draws.len());
        assert!(n.draws <= 2 * 64, "{} draws", n.draws);
        assert!(n.instances > 2_000, "{} plants drawn", n.instances);
        assert!(n.instances < 10_000, "cells behind and beside are culled");
        // Looking away from the field draws nothing.
        draws.clear();
        let away = camera(Vec3::new(0.0, 1.7, 60.0), Vec3::new(0.0, 1.7, 120.0));
        let n = layout.draws(
            &away,
            Vec3::new(0.0, 1.7, 60.0),
            &meshes(),
            None,
            &mut draws,
        );
        assert_eq!(n, FoliageCount::default());
    }

    #[test]
    fn levels_of_detail_follow_the_distance() {
        let k = &kinds()[0];
        assert_eq!(k.lod_at(5.0), Some(MeshId(0)));
        assert_eq!(k.lod_at(20.0), Some(MeshId(1)));
        assert_eq!(k.lod_at(49.9), Some(MeshId(1)));
        assert_eq!(k.lod_at(50.0), None);
        assert_eq!(k.fade_end(), 50.0);
        // From far away only the trees are left, with their single mesh.
        let plants = field(32);
        let layout = FoliageLayout::new(kinds(), &plants, &meshes()).unwrap();
        let eye = Vec3::new(0.0, 30.0, 120.0);
        let vp = camera(eye, Vec3::ZERO);
        let mut draws = Vec::new();
        layout.draws(&vp, eye, &meshes(), None, &mut draws);
        assert!(!draws.is_empty());
        assert!(draws.iter().all(|d| d.first_index == 72), "trees only");
        // Up close, near cells draw the fine mesh.
        let eye = Vec3::new(0.0, 1.7, 0.0);
        let vp = camera(eye, Vec3::new(0.0, 1.0, -10.0));
        draws.clear();
        layout.draws(&vp, eye, &meshes(), None, &mut draws);
        assert!(draws.iter().any(|d| d.first_index == 0));
    }

    #[test]
    fn shadow_draws_keep_casters_within_the_distance() {
        let plants = field(64);
        let layout = FoliageLayout::new(kinds(), &plants, &meshes()).unwrap();
        let eye = Vec3::new(0.0, 1.7, 0.0);
        let vp = camera(eye, Vec3::new(0.0, 1.0, -10.0));
        let mut draws = Vec::new();
        layout.draws(&vp, eye, &meshes(), Some(20.0), &mut draws);
        assert!(!draws.is_empty());
        assert!(
            draws.iter().all(|d| d.first_index == 72),
            "grass casts none"
        );
        draws.clear();
        layout.draws(&vp, eye, &meshes(), Some(0.0), &mut draws);
        assert!(draws.is_empty());
    }

    #[test]
    fn invalid_layers_are_refused() {
        let plants = field(4);
        let mut k = kinds();
        k[0].lods = vec![(MeshId(0), 30.0), (MeshId(1), 30.0)];
        assert!(FoliageLayout::new(k, &plants, &meshes()).is_err());
        let mut k = kinds();
        k[1].lods = vec![(MeshId(9), 30.0)];
        assert!(FoliageLayout::new(k, &plants, &meshes()).is_err());
        let mut p = field(2);
        p[0].kind = 5;
        assert!(FoliageLayout::new(kinds(), &p, &meshes()).is_err());
        p[0].kind = 0;
        p[1].position.x = f32::NAN;
        assert!(FoliageLayout::new(kinds(), &p, &meshes()).is_err());
        assert!(FoliageLayout::new(kinds(), &[], &meshes()).unwrap().len() == 0);
    }

    #[test]
    fn sway_grows_with_height_and_follows_the_wind() {
        let at = |h: f32, t: f32| sway_offset(h, 0.3, 1.0, t, [6.0, 0.0]);
        assert_eq!(at(0.0, 1.0), [0.0, 0.0]);
        for t in [0.0, 0.7, 2.3, 9.1] {
            let (a, b) = (at(0.5, t), at(1.0, t));
            assert!((b[0] - 4.0 * a[0]).abs() < 1e-5, "quadratic in height");
            assert_eq!(b[1], 0.0, "along the wind");
            assert!(b[0].abs() <= sway_bound(0.3, 1.0) + 1e-6);
        }
        // No wind: a diagonal breeze; rigid plants never move.
        let calm = sway_offset(1.0, 0.3, 0.0, 0.5, [0.0, 0.0]);
        assert!((calm[0] - calm[1]).abs() < 1e-6);
        assert_eq!(sway_offset(2.0, 0.0, 0.0, 0.5, [3.0, 1.0]), [0.0, 0.0]);
        // The phase desynchronises neighbours.
        assert_ne!(phase_of(Vec3::ZERO), phase_of(Vec3::new(1.0, 0.0, 0.0)));
    }
}

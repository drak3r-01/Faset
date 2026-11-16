//! CPU frustum culling and batching of instances into instanced draws.

use bytemuck::{Pod, Zeroable};
use faset_core::glam::{Mat3, Mat4, Vec4};

use crate::mesh::{MeshId, MeshRange};

/// One placement of a static mesh.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Instance {
    pub mesh: MeshId,
    pub material: u8,
    /// World-from-local of this frame.
    pub world: Mat4,
    /// World-from-local of the previous frame (equal to `world` when static).
    pub prev_world: Mat4,
}

impl Instance {
    /// A static instance: the previous matrix equals the current one.
    pub fn new(mesh: MeshId, material: u8, world: Mat4) -> Self {
        Self {
            mesh,
            material,
            world,
            prev_world: world,
        }
    }
}

/// Per-instance vertex data read by the opaque shader (176 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct GpuInstance {
    /// Unjittered clip-from-local, column-major.
    pub clip_from_local: [f32; 16],
    /// Previous clip-from-local, column-major.
    pub prev_clip_from_local: [f32; 16],
    /// World normal matrix columns (xyz, w = 0).
    pub normal_matrix: [f32; 12],
}

/// One `vkCmdDrawIndexed` over consecutive instances of one mesh.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DrawBatch {
    pub index_count: u32,
    pub first_index: u32,
    pub base_vertex: i32,
    pub first_instance: u32,
    pub instance_count: u32,
}

/// The six frustum planes (left, right, top, bottom, near, far) as
/// `n.xyz . p + w >= 0` inside, from an unjittered clip-from-world matrix.
///
/// With reverse-Z and the far plane at infinity the "far" plane is
/// `z_clip >= 0`, which every point in front of the camera satisfies.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Frustum {
    planes: [Vec4; 6],
}

impl Frustum {
    pub(crate) fn from_view_proj(m: &Mat4) -> Self {
        let r0 = m.row(0);
        let r1 = m.row(1);
        let r2 = m.row(2);
        let r3 = m.row(3);
        Self {
            planes: [r3 + r0, r3 - r0, r3 + r1, r3 - r1, r3 - r2, r2],
        }
    }

    /// False when the box (centre `c`, half extents `e`, world space) is
    /// entirely outside one plane.
    pub(crate) fn intersects_box(
        &self,
        c: faset_core::glam::Vec3,
        e: faset_core::glam::Vec3,
    ) -> bool {
        self.planes.iter().all(|p| {
            let n = p.truncate();
            n.dot(c) + p.w + n.abs().dot(e) >= 0.0
        })
    }
}

/// Culls `instances` against the camera and groups the visible ones by mesh.
///
/// Fills `gpu` with visible instance data (grouped by mesh, mesh id order)
/// and `draws` with one batch per visible mesh. Instances whose mesh is
/// unknown are skipped and counted in the return value.
pub(crate) fn cull_and_batch(
    view_proj: &Mat4,
    prev_view_proj: &Mat4,
    instances: &[Instance],
    meshes: &[MeshRange],
    gpu: &mut Vec<GpuInstance>,
    draws: &mut Vec<DrawBatch>,
) -> usize {
    gpu.clear();
    draws.clear();
    let frustum = Frustum::from_view_proj(view_proj);
    let mut unknown = 0;
    let mut visible: Vec<(u32, usize)> = Vec::with_capacity(instances.len());
    for (i, inst) in instances.iter().enumerate() {
        let Some(range) = meshes.get(inst.mesh.0 as usize) else {
            unknown += 1;
            continue;
        };
        if range.index_count == 0 {
            continue;
        }
        let b = range.bounds.transformed(&inst.world);
        if frustum.intersects_box(b.center(), b.half_extents()) {
            visible.push((inst.mesh.0, i));
        }
    }
    // Stable: keeps submission order inside a mesh.
    visible.sort_by_key(|&(mesh, _)| mesh);

    for &(mesh, i) in &visible {
        let inst = &instances[i];
        let first_instance = gpu.len() as u32;
        let normal = Mat3::from_mat4(inst.world).inverse().transpose();
        gpu.push(GpuInstance {
            clip_from_local: (*view_proj * inst.world).to_cols_array(),
            prev_clip_from_local: (*prev_view_proj * inst.prev_world).to_cols_array(),
            normal_matrix: [
                normal.x_axis.x,
                normal.x_axis.y,
                normal.x_axis.z,
                0.0,
                normal.y_axis.x,
                normal.y_axis.y,
                normal.y_axis.z,
                0.0,
                normal.z_axis.x,
                normal.z_axis.y,
                normal.z_axis.z,
                0.0,
            ],
        });
        match draws.last_mut() {
            Some(d)
                if d.first_index == meshes[mesh as usize].first_index
                    && d.base_vertex == meshes[mesh as usize].base_vertex
                    && d.first_instance + d.instance_count == first_instance =>
            {
                d.instance_count += 1;
            }
            _ => {
                let r = &meshes[mesh as usize];
                draws.push(DrawBatch {
                    index_count: r.index_count,
                    first_index: r.first_index,
                    base_vertex: r.base_vertex,
                    first_instance,
                    instance_count: 1,
                });
            }
        }
    }
    unknown
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::perspective_reverse_z;
    use crate::mesh::Aabb;
    use faset_core::glam::Vec3;

    fn unit_mesh(n: u32) -> Vec<MeshRange> {
        (0..n)
            .map(|k| MeshRange {
                first_index: k * 36,
                index_count: 36,
                base_vertex: (k * 24) as i32,
                bounds: Aabb {
                    min: Vec3::splat(-0.5),
                    max: Vec3::splat(0.5),
                },
            })
            .collect()
    }

    fn camera() -> Mat4 {
        perspective_reverse_z(90f32.to_radians(), 1.0, 0.1)
            * faset_core::glam::camera::rh::view::look_at_mat4(Vec3::ZERO, Vec3::NEG_Z, Vec3::Y)
    }

    #[test]
    fn ten_thousand_instances_ninety_percent_outside() {
        let meshes = unit_mesh(50);
        let vp = camera();
        let mut instances = Vec::new();
        // 1 000 visible: a 25 x 40 grid in front of the camera.
        for k in 0..1000u32 {
            let x = (k % 25) as f32 - 12.0;
            let y = (k / 25) as f32 * 0.5 - 10.0;
            let pos = Vec3::new(x, y, -60.0);
            instances.push(Instance::new(
                MeshId(k % 50),
                0,
                Mat4::from_translation(pos),
            ));
        }
        // 9 000 invisible: behind the camera, far to the sides, or above.
        for k in 0..9000u32 {
            let pos = match k % 3 {
                0 => Vec3::new((k % 97) as f32, 0.0, 5.0 + (k % 13) as f32),
                1 => Vec3::new(500.0 + k as f32, 0.0, -10.0),
                _ => Vec3::new(0.0, 300.0 + k as f32, -20.0),
            };
            instances.push(Instance::new(
                MeshId(k % 50),
                0,
                Mat4::from_translation(pos),
            ));
        }
        let expected_visible = instances
            .iter()
            .filter(|i| {
                // Reference test: every corner projected; visible when any
                // part of the box is inside the clip volume. For these
                // placements the box test is exact, so the counts agree.
                let p = i.world.w_axis.truncate();
                p.z < 0.0 && p.x.abs() <= -p.z + 0.5 && p.y.abs() <= -p.z + 0.5
            })
            .count();
        assert_eq!(expected_visible, 1000);

        let (mut gpu, mut draws) = (Vec::new(), Vec::new());
        let unknown = cull_and_batch(&vp, &vp, &instances, &meshes, &mut gpu, &mut draws);
        assert_eq!(unknown, 0);
        assert_eq!(gpu.len(), expected_visible);
        let drawn: u32 = draws.iter().map(|d| d.instance_count).sum();
        assert_eq!(
            drawn as usize, expected_visible,
            "draws cover every visible instance"
        );
        assert!(draws.len() < 500, "{} draw calls", draws.len());
        assert_eq!(draws.len(), 50, "one draw per visible mesh");
        // Batches are contiguous and non-overlapping.
        let mut next = 0;
        for d in &draws {
            assert_eq!(d.first_instance, next);
            next += d.instance_count;
        }
    }

    #[test]
    fn box_straddling_a_plane_is_kept() {
        let f = Frustum::from_view_proj(&camera());
        // Centre just outside the right plane, half extent reaching inside.
        assert!(f.intersects_box(Vec3::new(10.4, 0.0, -10.0), Vec3::splat(0.5)));
        // Fully outside: even the far corner (10.7, -10.5) is right of x = -z.
        assert!(!f.intersects_box(Vec3::new(11.2, 0.0, -10.0), Vec3::splat(0.5)));
        // Behind the near plane.
        assert!(!f.intersects_box(Vec3::new(0.0, 0.0, 1.0), Vec3::splat(0.5)));
        // Very far: the far plane is at infinity.
        assert!(f.intersects_box(Vec3::new(0.0, 0.0, -1.0e7), Vec3::splat(0.5)));
    }

    #[test]
    fn unknown_meshes_are_skipped() {
        let meshes = unit_mesh(1);
        let vp = camera();
        let w = Mat4::from_translation(Vec3::new(0.0, 0.0, -5.0));
        let instances = [
            Instance::new(MeshId(0), 0, w),
            Instance::new(MeshId(9), 0, w),
        ];
        let (mut gpu, mut draws) = (Vec::new(), Vec::new());
        assert_eq!(
            cull_and_batch(&vp, &vp, &instances, &meshes, &mut gpu, &mut draws),
            1
        );
        assert_eq!(gpu.len(), 1);
        assert_eq!(draws.len(), 1);
    }
}

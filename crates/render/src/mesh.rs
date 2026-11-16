//! Vertex format, mesh identifiers, bounds and the arena bookkeeping.

use bytemuck::{Pod, Zeroable};
use faset_core::glam::{Mat4, Vec3};

/// One vertex of static flat-shaded geometry: exactly 20 bytes.
///
/// Flat shading duplicates vertices per face, so the normal is the face
/// normal. Layout: position 3 x f32, normal 4 x i8 (snorm, w = 0), then
/// r, g, b, material id as u8 (colour is unorm, linear).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [i8; 4],
    /// r, g, b, material id.
    pub color_material: [u8; 4],
}

const _: () = assert!(std::mem::size_of::<Vertex>() == 20);

impl Vertex {
    /// Builds a vertex from a unit normal and a linear colour.
    pub fn new(position: Vec3, normal: Vec3, rgb: [u8; 3], material: u8) -> Self {
        Self {
            position: position.to_array(),
            normal: [snorm8(normal.x), snorm8(normal.y), snorm8(normal.z), 0],
            color_material: [rgb[0], rgb[1], rgb[2], material],
        }
    }
}

fn snorm8(v: f32) -> i8 {
    (v.clamp(-1.0, 1.0) * 127.0).round() as i8
}

/// Handle of a mesh uploaded to the renderer's arena.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MeshId(pub u32);

/// Axis-aligned bounding box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    /// Bounds of `vertices`, or a point at the origin when empty.
    pub fn from_vertices(vertices: &[Vertex]) -> Self {
        let mut iter = vertices.iter().map(|v| Vec3::from_array(v.position));
        let Some(first) = iter.next() else {
            return Self {
                min: Vec3::ZERO,
                max: Vec3::ZERO,
            };
        };
        let (min, max) = iter.fold((first, first), |(lo, hi), p| (lo.min(p), hi.max(p)));
        Self { min, max }
    }

    pub fn center(&self) -> Vec3 {
        0.5 * (self.min + self.max)
    }

    pub fn half_extents(&self) -> Vec3 {
        0.5 * (self.max - self.min)
    }

    /// Bounds of this box after `m` (centre and absolute-matrix extents).
    pub fn transformed(&self, m: &Mat4) -> Self {
        let c = m.transform_point3(self.center());
        let e = self.half_extents();
        let r = Vec3::new(
            m.x_axis.x.abs() * e.x + m.y_axis.x.abs() * e.y + m.z_axis.x.abs() * e.z,
            m.x_axis.y.abs() * e.x + m.y_axis.y.abs() * e.y + m.z_axis.y.abs() * e.z,
            m.x_axis.z.abs() * e.x + m.y_axis.z.abs() * e.y + m.z_axis.z.abs() * e.z,
        );
        Self {
            min: c - r,
            max: c + r,
        }
    }
}

/// Where a mesh lives in the arena.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct MeshRange {
    pub first_index: u32,
    pub index_count: u32,
    pub base_vertex: i32,
    pub bounds: Aabb,
}

/// Face colours of [`cube_mesh`], in order +X, -X, +Y, -Y, +Z, -Z.
pub type CubeFaceColors = [[u8; 3]; 6];

/// Default "one tint per face direction" colours of the reference scene.
pub const CUBE_FACE_TINTS: CubeFaceColors = [
    [230, 60, 50],  // +X red
    [60, 200, 220], // -X cyan
    [240, 230, 70], // +Y yellow
    [90, 60, 200],  // -Y violet
    [70, 200, 80],  // +Z green
    [220, 80, 200], // -Z magenta
];

/// Flat-shaded axis-aligned cube centred on the origin: 24 vertices,
/// 36 indices, counter-clockwise outward faces.
pub fn cube_mesh(half: f32, colors: &CubeFaceColors, material: u8) -> (Vec<Vertex>, Vec<u32>) {
    // (normal, u axis, v axis) with u x v = normal, so (0,1,2) is CCW
    // seen from outside.
    let faces = [
        (Vec3::X, Vec3::NEG_Z, Vec3::Y),
        (Vec3::NEG_X, Vec3::Z, Vec3::Y),
        (Vec3::Y, Vec3::X, Vec3::NEG_Z),
        (Vec3::NEG_Y, Vec3::X, Vec3::Z),
        (Vec3::Z, Vec3::X, Vec3::Y),
        (Vec3::NEG_Z, Vec3::NEG_X, Vec3::Y),
    ];
    let mut vertices = Vec::with_capacity(24);
    let mut indices = Vec::with_capacity(36);
    for (face, &(n, u, v)) in faces.iter().enumerate() {
        let base = vertices.len() as u32;
        for (su, sv) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            let p = half * (n + su * u + sv * v);
            vertices.push(Vertex::new(p, n, colors[face], material));
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    (vertices, indices)
}

/// Bump allocation of vertex and index ranges in the arena.
#[derive(Debug)]
pub(crate) struct ArenaLayout {
    pub vertex_capacity: u32,
    pub index_capacity: u32,
    pub vertex_count: u32,
    pub index_count: u32,
}

impl ArenaLayout {
    pub(crate) fn new(vertex_capacity: u32, index_capacity: u32) -> Self {
        Self {
            vertex_capacity,
            index_capacity,
            vertex_count: 0,
            index_count: 0,
        }
    }

    /// Reserves room for a mesh; returns (first vertex, first index).
    pub(crate) fn reserve(&mut self, vertices: usize, indices: usize) -> Option<(u32, u32)> {
        let v = u32::try_from(vertices).ok()?;
        let i = u32::try_from(indices).ok()?;
        let v_end = self.vertex_count.checked_add(v)?;
        let i_end = self.index_count.checked_add(i)?;
        if v_end > self.vertex_capacity || i_end > self.index_capacity {
            return None;
        }
        let first = (self.vertex_count, self.index_count);
        self.vertex_count = v_end;
        self.index_count = i_end;
        Some(first)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vertex_is_20_bytes() {
        assert_eq!(std::mem::size_of::<Vertex>(), 20);
        assert_eq!(std::mem::align_of::<Vertex>(), 4);
        let v = Vertex::new(Vec3::new(1.0, 2.0, 3.0), Vec3::NEG_Y, [1, 2, 3], 7);
        let bytes: &[u8] = bytemuck::bytes_of(&v);
        assert_eq!(&bytes[12..16], &[0, (-127i8) as u8, 0, 0]);
        assert_eq!(&bytes[16..20], &[1, 2, 3, 7]);
    }

    #[test]
    fn cube_faces_are_counter_clockwise_outward() {
        let (v, i) = cube_mesh(0.5, &CUBE_FACE_TINTS, 0);
        assert_eq!((v.len(), i.len()), (24, 36));
        for tri in i.chunks(3) {
            let p = |k: usize| Vec3::from_array(v[tri[k] as usize].position);
            let n = (p(1) - p(0)).cross(p(2) - p(0)).normalize();
            let stored = v[tri[0] as usize].normal;
            let stored = Vec3::new(stored[0] as f32, stored[1] as f32, stored[2] as f32) / 127.0;
            assert!(n.dot(stored) > 0.99);
            assert!(p(0).dot(n) > 0.0, "face points outward");
        }
        let b = Aabb::from_vertices(&v);
        assert_eq!(b.min, Vec3::splat(-0.5));
        assert_eq!(b.max, Vec3::splat(0.5));
    }

    #[test]
    fn transformed_bounds_contain_rotated_box() {
        let b = Aabb {
            min: Vec3::splat(-1.0),
            max: Vec3::splat(1.0),
        };
        let m = Mat4::from_translation(Vec3::new(10.0, 0.0, 0.0))
            * Mat4::from_rotation_y(std::f32::consts::FRAC_PI_4);
        let t = b.transformed(&m);
        let r = std::f32::consts::SQRT_2;
        assert!((t.min - Vec3::new(10.0 - r, -1.0, -r)).abs().max_element() < 1e-5);
        assert!((t.max - Vec3::new(10.0 + r, 1.0, r)).abs().max_element() < 1e-5);
    }

    #[test]
    fn arena_layout_bumps_and_fills() {
        let mut a = ArenaLayout::new(30, 40);
        assert_eq!(a.reserve(24, 36), Some((0, 0)));
        assert_eq!(a.reserve(6, 4), Some((24, 36)));
        assert_eq!(a.reserve(1, 0), None);
        assert_eq!(a.reserve(0, 1), None);
        assert_eq!(a.reserve(0, 0), Some((30, 40)));
    }
}

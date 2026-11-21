use std::collections::HashMap;

use faset_core::glam::{Mat3, Mat4, Vec3};

use crate::BuildError;

/// Hard cap on the vertices of one built mesh.
pub(crate) const MAX_VERTICES: usize = 200_000;

/// 20-byte vertex: position 3 x f32, normal 4 x i8 (snorm, w = 0), then
/// r, g, b, material id as u8. Same layout as the renderer's vertex.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [i8; 4],
    pub color_material: [u8; 4],
}

const _: () = assert!(std::mem::size_of::<Vertex>() == 20);

impl Vertex {
    /// The normal as a float vector (about unit length).
    pub fn normal_vec(&self) -> Vec3 {
        Vec3::new(
            f32::from(self.normal[0]),
            f32::from(self.normal[1]),
            f32::from(self.normal[2]),
        ) / 127.0
    }
}

/// Indexed triangle mesh, counter-clockwise outward, flat shaded (vertices
/// are duplicated per face unless a smoothing group merges normals).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

impl Mesh {
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// Axis-aligned bounds, or `None` for an empty mesh.
    pub fn bounds(&self) -> Option<(Vec3, Vec3)> {
        let mut it = self.vertices.iter().map(|v| Vec3::from_array(v.position));
        let first = it.next()?;
        Some(it.fold((first, first), |(lo, hi), p| (lo.min(p), hi.max(p))))
    }
}

/// Colour and material id of a primitive.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Style {
    pub rgb: [u8; 3],
    pub material: u8,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct RawVertex {
    pub p: Vec3,
    pub n: Vec3,
    pub c: [u8; 3],
    pub m: u8,
}

/// Float-precision mesh under construction.
#[derive(Debug, Clone, Default)]
pub(crate) struct Raw {
    pub v: Vec<RawVertex>,
    pub i: Vec<u32>,
}

fn key(p: Vec3) -> [i64; 3] {
    [
        (p.x * 1000.0).round() as i64,
        (p.y * 1000.0).round() as i64,
        (p.z * 1000.0).round() as i64,
    ]
}

fn snorm8(v: f32) -> i8 {
    (v.clamp(-1.0, 1.0) * 127.0).round() as i8
}

impl Raw {
    /// One flat triangle, wound so that its normal faces `dir`.
    /// Degenerate triangles are skipped.
    pub fn tri(&mut self, a: Vec3, mut b: Vec3, mut c: Vec3, dir: Vec3, s: Style) {
        let mut n = (b - a).cross(c - a);
        if n.length_squared() < 1e-14 {
            return;
        }
        if n.dot(dir) < 0.0 {
            std::mem::swap(&mut b, &mut c);
            n = -n;
        }
        let n = n.normalize();
        let base = self.v.len() as u32;
        for p in [a, b, c] {
            self.v.push(RawVertex {
                p,
                n,
                c: s.rgb,
                m: s.material,
            });
        }
        self.i.extend_from_slice(&[base, base + 1, base + 2]);
    }

    /// One flat quad (4 vertices, 2 triangles), wound to face `dir`.
    pub fn quad(&mut self, q: [Vec3; 4], dir: Vec3, s: Style) {
        let [a, mut b, c, mut d] = q;
        let mut n = (b - a).cross(c - a);
        if n.length_squared() < 1e-14 {
            n = (c - a).cross(d - a);
        }
        if n.length_squared() < 1e-14 {
            return;
        }
        if n.dot(dir) < 0.0 {
            std::mem::swap(&mut b, &mut d);
            n = -n;
        }
        let n = n.normalize();
        let base = self.v.len() as u32;
        for p in [a, b, c, d] {
            self.v.push(RawVertex {
                p,
                n,
                c: s.rgb,
                m: s.material,
            });
        }
        self.i
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    pub fn append(&mut self, other: &Raw) -> Result<(), BuildError> {
        let base = self.v.len() as u32;
        self.v.extend_from_slice(&other.v);
        self.i.extend(other.i.iter().map(|i| i + base));
        if self.v.len() > MAX_VERTICES {
            return Err(BuildError::TooLarge(MAX_VERTICES));
        }
        Ok(())
    }

    /// Applies `m` to positions and normals; mirroring matrices flip the
    /// winding so faces stay outward.
    pub fn transform(&mut self, m: &Mat4) {
        let normal_m = Mat3::from_mat4(*m).inverse().transpose();
        for v in &mut self.v {
            v.p = m.transform_point3(v.p);
            v.n = (normal_m * v.n).normalize_or_zero();
        }
        if m.determinant() < 0.0 {
            for t in self.i.as_chunks_mut::<3>().0 {
                t.swap(1, 2);
            }
        }
    }

    /// One private vertex per triangle corner (needed before moving
    /// vertices of shared quads independently).
    pub fn unshare(&mut self) {
        let mut v = Vec::with_capacity(self.i.len());
        for &i in &self.i {
            v.push(self.v[i as usize]);
        }
        self.i = (0..v.len() as u32).collect();
        self.v = v;
    }

    /// Recomputes every normal from its triangle (call after `unshare`).
    pub fn recompute_flat_normals(&mut self) {
        for t in self.i.as_chunks::<3>().0 {
            let (a, b, c) = (
                self.v[t[0] as usize].p,
                self.v[t[1] as usize].p,
                self.v[t[2] as usize].p,
            );
            let n = (b - a).cross(c - a).normalize_or_zero();
            for &i in t.iter() {
                self.v[i as usize].n = n;
            }
        }
    }

    /// Smoothing group: normals of vertices at the same position are
    /// averaged. The vertex count is unchanged.
    pub fn smooth_normals(&mut self) {
        let mut sums: HashMap<[i64; 3], Vec3> = HashMap::new();
        for v in &self.v {
            *sums.entry(key(v.p)).or_insert(Vec3::ZERO) += v.n;
        }
        for v in &mut self.v {
            let s = sums[&key(v.p)].normalize_or_zero();
            if s != Vec3::ZERO {
                v.n = s;
            }
        }
    }

    /// Stable lookup key of a position, so shared points move together.
    pub fn position_key(p: Vec3) -> [i64; 3] {
        key(p)
    }

    pub fn into_mesh(self) -> Mesh {
        Mesh {
            vertices: self
                .v
                .iter()
                .map(|v| Vertex {
                    position: v.p.to_array(),
                    normal: [snorm8(v.n.x), snorm8(v.n.y), snorm8(v.n.z), 0],
                    color_material: [v.c[0], v.c[1], v.c[2], v.m],
                })
                .collect(),
            indices: self.i,
        }
    }
}

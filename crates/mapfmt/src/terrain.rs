//! Terrain: an optional height field under the brushes (lot 6.10).
//!
//! A terrain is a regular grid of height samples in the x-z plane. Sample
//! `(i, j)` stands at `x = origin.x + i * cell`, `z = origin.z + j * cell`
//! (origin and cell on the 0.5 m grid) and its height is in centimetres.
//! Each grid cell is split into two triangles along the diagonal from
//! sample `(i, j)` to `(i + 1, j + 1)`; render mesh, collision and
//! [`Terrain::height_at`] all use that same triangulation, so what is seen is
//! what is walked on.
//!
//! Decisions of lot 6.10:
//! - resolution: [`DEFAULT_CELL`] = 4 half-metres (2 m between samples) by
//!   default, any cell from 1 to [`MAX_CELL`] half-metres is allowed; heights
//!   are `i16` centimetres (range +-327 m, 1 cm steps);
//! - encoding: heights are the base64 (standard alphabet, padded) of the
//!   little-endian `i16` samples, row by row (x fastest); the paint layer is
//!   the base64 of one byte per sample and is omitted when every sample is
//!   0 (automatic). Both stay one RON string each, so a 50 x 40 terrain adds
//!   about 5 KB to a map file;
//! - the terrain is not affected by `symmetry` or prefabs: the file stores
//!   the whole terrain;
//! - the solid under the surface goes down to [`DEPTH_M`] below the lowest
//!   sample; the render mesh closes the borders with vertical skirts to that
//!   depth.

use faset_core::glam::Vec3;
use serde::{Deserialize, Serialize};

use crate::schema::GRID_UNIT_M;

/// Default distance between samples, in half-metres (2 m).
pub const DEFAULT_CELL: u8 = 4;
/// Largest distance between samples, in half-metres (8 m).
pub const MAX_CELL: u8 = 16;
/// Most samples along one axis.
pub const MAX_SAMPLES: u16 = 1025;
/// Depth of the solid below the lowest sample, in metres.
pub const DEPTH_M: f32 = 2.0;

/// Names of the paint values: 0 lets the map palette choose, the others
/// force a ground. Steep slopes blend to rock whatever the paint.
pub const PAINTS: [&str; 6] = ["auto", "grass", "sand", "rock", "snow", "dirt"];

/// The height field of a map. Serialised as `terrain: (...)`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Terrain {
    /// Position of sample `(0, 0)` on the half-metre grid, `(x, z)`.
    pub origin: [i32; 2],
    /// Distance between samples, in half-metres.
    #[serde(default = "default_cell", skip_serializing_if = "is_default_cell")]
    pub cell: u8,
    /// Number of samples along x and z (at least 2 each).
    pub size: [u16; 2],
    /// Heights in centimetres, row by row (x fastest).
    #[serde(with = "b64_i16")]
    pub heights: Vec<i16>,
    /// Paint value per sample (index of [`PAINTS`]); empty means all 0.
    #[serde(default, skip_serializing_if = "Vec::is_empty", with = "b64_u8")]
    pub paint: Vec<u8>,
}

fn default_cell() -> u8 {
    DEFAULT_CELL
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_default_cell(c: &u8) -> bool {
    *c == DEFAULT_CELL
}

/// Corners of one triangle as sample coordinates `(i, j)`.
pub type Triangle = [(usize, usize); 3];

impl Terrain {
    /// A flat terrain of `size` samples at `height_cm`.
    #[must_use]
    pub fn flat(origin: [i32; 2], cell: u8, size: [u16; 2], height_cm: i16) -> Self {
        let n = usize::from(size[0]) * usize::from(size[1]);
        Self {
            origin,
            cell,
            size,
            heights: vec![height_cm; n],
            paint: Vec::new(),
        }
    }

    /// Samples along x and z.
    #[must_use]
    pub fn dims(&self) -> (usize, usize) {
        (usize::from(self.size[0]), usize::from(self.size[1]))
    }

    /// Distance between samples in metres.
    #[must_use]
    pub fn cell_m(&self) -> f32 {
        f32::from(self.cell) * GRID_UNIT_M
    }

    /// Index of sample `(i, j)` in [`Terrain::heights`].
    #[must_use]
    pub fn index(&self, i: usize, j: usize) -> usize {
        j * usize::from(self.size[0]) + i
    }

    /// Height of sample `(i, j)` in metres.
    #[must_use]
    pub fn height_m(&self, i: usize, j: usize) -> f32 {
        f32::from(self.heights[self.index(i, j)]) * 0.01
    }

    /// Paint value of sample `(i, j)`.
    #[must_use]
    pub fn paint_at(&self, i: usize, j: usize) -> u8 {
        self.paint.get(self.index(i, j)).copied().unwrap_or(0)
    }

    /// World position of sample `(i, j)`, metres.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn sample_pos(&self, i: usize, j: usize) -> Vec3 {
        let c = self.cell_m();
        Vec3::new(
            self.origin[0] as f32 * GRID_UNIT_M + i as f32 * c,
            self.height_m(i, j),
            self.origin[1] as f32 * GRID_UNIT_M + j as f32 * c,
        )
    }

    /// Lowest and highest corners of the x-z rectangle covered, metres.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn extent_xz(&self) -> ([f32; 2], [f32; 2]) {
        let lo = [
            self.origin[0] as f32 * GRID_UNIT_M,
            self.origin[1] as f32 * GRID_UNIT_M,
        ];
        let (nx, nz) = self.dims();
        let c = self.cell_m();
        (
            lo,
            [
                lo[0] + (nx.saturating_sub(1)) as f32 * c,
                lo[1] + (nz.saturating_sub(1)) as f32 * c,
            ],
        )
    }

    /// Lowest and highest sample, metres.
    #[must_use]
    pub fn height_range(&self) -> (f32, f32) {
        let lo = self.heights.iter().copied().min().unwrap_or(0);
        let hi = self.heights.iter().copied().max().unwrap_or(0);
        (f32::from(lo) * 0.01, f32::from(hi) * 0.01)
    }

    /// Bottom of the solid under the surface, metres.
    #[must_use]
    pub fn base_y(&self) -> f32 {
        self.height_range().0 - DEPTH_M
    }

    /// The two triangles of every cell, cell by cell (x fastest), each
    /// counter-clockwise seen from above.
    pub fn triangles(&self) -> impl Iterator<Item = Triangle> + '_ {
        let (nx, nz) = self.dims();
        (0..nz.saturating_sub(1)).flat_map(move |j| {
            (0..nx.saturating_sub(1)).flat_map(move |i| {
                [
                    [(i, j), (i, j + 1), (i + 1, j + 1)],
                    [(i, j), (i + 1, j + 1), (i + 1, j)],
                ]
            })
        })
    }

    /// Smooth normal of sample `(i, j)`: central differences of the heights
    /// (one-sided on the borders).
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn normal(&self, i: usize, j: usize) -> Vec3 {
        let (nx, nz) = self.dims();
        let (i0, i1) = (i.saturating_sub(1), (i + 1).min(nx - 1));
        let (j0, j1) = (j.saturating_sub(1), (j + 1).min(nz - 1));
        let c = self.cell_m();
        let dx = (self.height_m(i1, j) - self.height_m(i0, j)) / ((i1 - i0).max(1) as f32 * c);
        let dz = (self.height_m(i, j1) - self.height_m(i, j0)) / ((j1 - j0).max(1) as f32 * c);
        Vec3::new(-dx, 1.0, -dz).normalize()
    }

    /// Cell coordinates and fractions of a point, if it lies over the grid.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn locate(&self, x: f32, z: f32) -> Option<(usize, usize, f32, f32)> {
        let (lo, _) = self.extent_xz();
        let c = self.cell_m();
        let (u, v) = ((x - lo[0]) / c, (z - lo[1]) / c);
        let (nx, nz) = self.dims();
        let (mx, mz) = ((nx - 1) as f32, (nz - 1) as f32);
        if !(0.0..=mx).contains(&u) || !(0.0..=mz).contains(&v) {
            return None;
        }
        let i = (u.floor() as usize).min(nx - 2);
        let j = (v.floor() as usize).min(nz - 2);
        Some((i, j, u - i as f32, v - j as f32))
    }

    /// Height of the surface at `(x, z)` on the triangle under it, or `None`
    /// outside the grid.
    #[must_use]
    pub fn height_at(&self, x: f32, z: f32) -> Option<f32> {
        let (i, j, fu, fv) = self.locate(x, z)?;
        let h = |a: usize, b: usize| self.height_m(a, b);
        let (h00, h01, h10, h11) = (h(i, j), h(i, j + 1), h(i + 1, j), h(i + 1, j + 1));
        Some(if fv >= fu {
            h00 + fv * (h01 - h00) + fu * (h11 - h01)
        } else {
            h00 + fu * (h10 - h00) + fv * (h11 - h10)
        })
    }

    /// Face normal of the triangle under `(x, z)`, or `None` outside.
    #[must_use]
    pub fn face_normal_at(&self, x: f32, z: f32) -> Option<Vec3> {
        let (i, j, fu, fv) = self.locate(x, z)?;
        let t: Triangle = if fv >= fu {
            [(i, j), (i, j + 1), (i + 1, j + 1)]
        } else {
            [(i, j), (i + 1, j + 1), (i + 1, j)]
        };
        let p = t.map(|(a, b)| self.sample_pos(a, b));
        Some((p[1] - p[0]).cross(p[2] - p[0]).normalize())
    }

    /// Samples within `radius` metres of `(x, z)` (horizontally), with their
    /// distance, row by row.
    #[must_use]
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        clippy::single_range_in_vec_init
    )]
    pub fn samples_within(&self, x: f32, z: f32, radius: f32) -> Vec<(usize, usize, f32)> {
        let (lo, _) = self.extent_xz();
        let c = self.cell_m();
        let (nx, nz) = self.dims();
        // Empty range when the circle misses the grid on that axis.
        let range = |centre: f32, n: usize| {
            let a = ((centre - radius) / c).ceil().max(0.0);
            let b = ((centre + radius) / c).floor().min((n - 1) as f32);
            if b < a {
                1..1
            } else {
                a as usize..b as usize + 1
            }
        };
        let mut out = Vec::new();
        for j in range(z - lo[1], nz) {
            for i in range(x - lo[0], nx) {
                let p = self.sample_pos(i, j);
                let d = ((p.x - x).powi(2) + (p.z - z).powi(2)).sqrt();
                if d <= radius {
                    out.push((i, j, d));
                }
            }
        }
        out
    }

    /// Structural rules of a terrain, as an error message.
    ///
    /// # Errors
    /// A message naming the first broken rule.
    pub fn validate(&self) -> Result<(), String> {
        if self.cell == 0 || self.cell > MAX_CELL {
            return Err(format!(
                "terrain cell is {} half-metres, allowed 1..={MAX_CELL}",
                self.cell
            ));
        }
        let [nx, nz] = self.size;
        if !(2..=MAX_SAMPLES).contains(&nx) || !(2..=MAX_SAMPLES).contains(&nz) {
            return Err(format!(
                "terrain size ({nx}, {nz}) is outside 2..={MAX_SAMPLES} samples per axis"
            ));
        }
        let n = usize::from(nx) * usize::from(nz);
        if self.heights.len() != n {
            return Err(format!(
                "terrain has {} heights for {nx} x {nz} = {n} samples",
                self.heights.len()
            ));
        }
        if !self.paint.is_empty() && self.paint.len() != n {
            return Err(format!(
                "terrain paint has {} values for {n} samples",
                self.paint.len()
            ));
        }
        if let Some(p) = self.paint.iter().find(|p| usize::from(**p) >= PAINTS.len()) {
            return Err(format!(
                "terrain paint value {p} is unknown (0..={})",
                PAINTS.len() - 1
            ));
        }
        Ok(())
    }
}

/// Base64 (RFC 4648, standard alphabet, padded).
pub(crate) mod base64 {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    pub fn encode(bytes: &[u8]) -> String {
        let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
        for chunk in bytes.chunks(3) {
            let b = [
                chunk[0],
                chunk.get(1).copied().unwrap_or(0),
                chunk.get(2).copied().unwrap_or(0),
            ];
            let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
            for k in 0..4 {
                if k <= chunk.len() {
                    out.push(char::from(ALPHABET[((n >> (18 - 6 * k)) & 63) as usize]));
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    pub fn decode(text: &str) -> Result<Vec<u8>, String> {
        let t = text.as_bytes();
        if !t.len().is_multiple_of(4) {
            return Err("base64 length is not a multiple of 4".to_owned());
        }
        let value = |c: u8| -> Result<u32, String> {
            ALPHABET
                .iter()
                .position(|a| *a == c)
                .map(|p| p as u32)
                .ok_or_else(|| format!("invalid base64 character {:?}", char::from(c)))
        };
        let mut out = Vec::with_capacity(t.len() / 4 * 3);
        for (q, quad) in t.chunks(4).enumerate() {
            let last = q + 1 == t.len() / 4;
            let pad = quad.iter().rev().take_while(|c| **c == b'=').count();
            if pad > 2 || (pad > 0 && !last) {
                return Err("misplaced base64 padding".to_owned());
            }
            let mut n = 0u32;
            for &c in &quad[..4 - pad] {
                n = (n << 6) | value(c)?;
            }
            n <<= 6 * pad as u32;
            let bytes = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
            out.extend_from_slice(&bytes[..3 - pad]);
        }
        Ok(out)
    }
}

/// Serde adapter: `Vec<i16>` as the base64 of its little-endian bytes.
mod b64_i16 {
    use serde::{de::Error, Deserialize, Deserializer, Serializer};

    #[allow(clippy::ptr_arg)]
    pub fn serialize<S: Serializer>(v: &Vec<i16>, s: S) -> Result<S::Ok, S::Error> {
        let bytes: Vec<u8> = v.iter().flat_map(|h| h.to_le_bytes()).collect();
        s.serialize_str(&super::base64::encode(&bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<i16>, D::Error> {
        let text = String::deserialize(d)?;
        let bytes = super::base64::decode(&text).map_err(D::Error::custom)?;
        if !bytes.len().is_multiple_of(2) {
            return Err(D::Error::custom("terrain heights have an odd byte count"));
        }
        Ok(bytes
            .chunks(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect())
    }
}

/// Serde adapter: `Vec<u8>` as base64.
mod b64_u8 {
    use serde::{de::Error, Deserialize, Deserializer, Serializer};

    #[allow(clippy::ptr_arg)]
    pub fn serialize<S: Serializer>(v: &Vec<u8>, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&super::base64::encode(v))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(d)?;
        super::base64::decode(&text).map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bumpy() -> Terrain {
        let mut t = Terrain::flat([-8, -8], 4, [5, 5], 0);
        for j in 0..5 {
            for i in 0..5 {
                let k = t.index(i, j);
                t.heights[k] = (i * 37 + j * 11) as i16 - 50;
            }
        }
        t
    }

    #[test]
    fn base64_round_trips_every_length() {
        for n in 0..20u8 {
            let bytes: Vec<u8> = (0..n).map(|k| k.wrapping_mul(97).wrapping_add(3)).collect();
            let text = base64::encode(&bytes);
            assert_eq!(text.len() % 4, 0);
            assert_eq!(base64::decode(&text).unwrap(), bytes);
        }
        assert_eq!(base64::encode(b"Man"), "TWFu");
        assert_eq!(base64::encode(b"Ma"), "TWE=");
        assert!(base64::decode("TW=u").is_err());
        assert!(base64::decode("TW?u").is_err());
        assert!(base64::decode("TWE").is_err());
    }

    #[test]
    fn height_at_matches_samples_and_triangles() {
        let t = bumpy();
        for j in 0..5 {
            for i in 0..5 {
                let p = t.sample_pos(i, j);
                assert!((t.height_at(p.x, p.z).unwrap() - p.y).abs() < 1e-5);
            }
        }
        // Inside a triangle the height lies on its plane.
        for tri in t.triangles() {
            let p = tri.map(|(i, j)| t.sample_pos(i, j));
            let c = (p[0] + p[1] + p[2]) / 3.0;
            let h = t.height_at(c.x, c.z).unwrap();
            assert!((h - c.y).abs() < 1e-4, "{h} vs {}", c.y);
            let n = (p[1] - p[0]).cross(p[2] - p[0]);
            assert!(n.y > 0.0, "triangle faces down");
        }
        assert!(t.height_at(-4.1, 0.0).is_none());
        assert!(t.height_at(0.0, 4.01).is_none());
        assert_eq!(t.triangles().count(), 32);
    }

    #[test]
    fn samples_within_radius() {
        let t = bumpy();
        let s = t.samples_within(0.0, 0.0, 2.1);
        // The centre and its four neighbours, 2 m apart.
        assert_eq!(s.len(), 5);
        assert!(t.samples_within(100.0, 0.0, 3.0).is_empty());
    }

    #[test]
    fn validation_rules() {
        let mut t = bumpy();
        assert!(t.validate().is_ok());
        t.heights.pop();
        assert!(t.validate().is_err());
        let mut t = bumpy();
        t.paint = vec![0; 24];
        assert!(t.validate().is_err());
        t.paint = vec![9; 25];
        assert!(t.validate().is_err());
        let mut t = bumpy();
        t.cell = 0;
        assert!(t.validate().is_err());
        let t = Terrain::flat([0, 0], 4, [1, 5], 0);
        assert!(t.validate().is_err());
    }
}

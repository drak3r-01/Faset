//! World: the large terrain a map sits in (format version 5, lot 8.1).
//!
//! A map may sit on a 1 to 2 km landscape while only its playable rectangle
//! is simulated. The optional [`World`] block holds the world extent, the
//! playable rectangle and a coarse far height field ([`FarField`]) covering
//! the whole extent. The sculpted high-resolution grid of the playable zone is
//! the map's existing [`Terrain`] block (lot 6.10): the simulation, the bots
//! and the editor keep reading it unchanged, and it is the only terrain they
//! read. Everything else in this block is scenery, drawn by the client.
//!
//! [`World::height_at`] is the world surface: the playable terrain where it
//! exists (exactly, on its own triangles, so what is seen there is what is
//! walked on), the far field elsewhere (on its own triangles, split like the
//! terrain's, so a mesh through its samples is exact and a renderer only
//! needs finer levels near the playable zone; the client smooths the far
//! normals instead), and a smooth blend over [`BLEND_M`] metres from the
//! border of the terrain grid outward.
//!
//! Decisions of lot 8.1:
//! - the far field is **stored** (not regenerated from a seed) so a sculpted
//!   world (lots 8.3, 8.9) survives a round trip; samples are decimetres,
//!   written as the base64 of the zigzag LEB128 varints of the differences
//!   between consecutive samples (row by row, x fastest). Smooth ground
//!   costs one or two bytes per sample: a 1 km world with 8 m cells (126 x
//!   126 samples) adds about 25 KB to the map file;
//! - the far paint (optional, [`crate::terrain::PAINTS`] values) is the
//!   base64 of `(value, run length varint)` pairs; empty means all `auto`;
//! - the far field covers exactly `extent`: its origin is the extent's low
//!   corner and `extent` spans `(size - 1) * cell` metres on each axis;
//! - `playable` equals the `surroundings` rectangle when a map has both.

use serde::{Deserialize, Serialize};

use crate::terrain::{Terrain, PAINTS};

/// Width of the blend from the playable terrain to the far field, metres.
pub const BLEND_M: f32 = 16.0;
/// Most far samples along one axis.
pub const MAX_FAR_SAMPLES: u16 = 1025;
/// Largest far cell, metres.
pub const MAX_FAR_CELL_M: u16 = 64;
/// Largest world side, metres.
pub const MAX_EXTENT_M: f32 = 4096.0;
/// Largest absolute far height, decimetres (10 km).
pub const MAX_FAR_HEIGHT_DM: i32 = 100_000;

/// The coarse height field of the whole world. Serialised as `far: (...)`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FarField {
    /// Distance between samples, metres.
    pub cell: u16,
    /// Number of samples along x and z (at least 2 each).
    pub size: [u16; 2],
    /// Heights in decimetres, row by row (x fastest).
    #[serde(with = "b64_delta")]
    pub heights: Vec<i32>,
    /// Paint value per sample (index of [`PAINTS`]); empty means all 0.
    #[serde(default, skip_serializing_if = "Vec::is_empty", with = "b64_rle")]
    pub paint: Vec<u8>,
}

impl FarField {
    /// Samples along x and z.
    #[must_use]
    pub fn dims(&self) -> (usize, usize) {
        (usize::from(self.size[0]), usize::from(self.size[1]))
    }

    /// Height of sample `(i, j)` in metres (indices clamped to the grid).
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn height_m(&self, i: usize, j: usize) -> f32 {
        let (nx, nz) = self.dims();
        let k = j.min(nz - 1) * nx + i.min(nx - 1);
        self.heights[k] as f32 * 0.1
    }

    /// Paint value of sample `(i, j)` (indices clamped to the grid).
    #[must_use]
    pub fn paint_at(&self, i: usize, j: usize) -> u8 {
        let (nx, nz) = self.dims();
        self.paint
            .get(j.min(nz - 1) * nx + i.min(nx - 1))
            .copied()
            .unwrap_or(0)
    }
}

/// The world block of a map. Serialised as `world: (...)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct World {
    /// World rectangle in metres: `(min_x, min_z, max_x, max_z)`.
    pub extent: [f32; 4],
    /// Simulated rectangle in metres, inside `extent`.
    pub playable: [f32; 4],
    /// Coarse heights over the whole extent.
    pub far: FarField,
    /// Height above which the ground turns to snow, metres; `None` for none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snow_line: Option<f32>,
}

fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Horizontal distance from `(x, z)` to a rectangle `(lo, hi)`, 0 inside.
fn distance_to_rect(x: f32, z: f32, lo: [f32; 2], hi: [f32; 2]) -> f32 {
    let dx = (lo[0] - x).max(x - hi[0]).max(0.0);
    let dz = (lo[1] - z).max(z - hi[1]).max(0.0);
    (dx * dx + dz * dz).sqrt()
}

impl World {
    /// Width and depth of the world, metres.
    #[must_use]
    pub fn size_m(&self) -> [f32; 2] {
        let [x0, z0, x1, z1] = self.extent;
        [x1 - x0, z1 - z0]
    }

    /// Whether `(x, z)` (metres) lies inside the playable rectangle.
    #[must_use]
    pub fn in_playable(&self, x: f32, z: f32) -> bool {
        let [x0, z0, x1, z1] = self.playable;
        (x0..=x1).contains(&x) && (z0..=z1).contains(&z)
    }

    /// Far grid coordinates of a point: the cell and its fractions, clamped
    /// to the grid.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    fn far_locate(&self, x: f32, z: f32) -> (usize, usize, f32, f32) {
        let (nx, nz) = self.far.dims();
        let c = f32::from(self.far.cell);
        let u = ((x - self.extent[0]) / c).clamp(0.0, (nx - 1) as f32);
        let v = ((z - self.extent[1]) / c).clamp(0.0, (nz - 1) as f32);
        let i = (u.floor() as usize).min(nx - 2);
        let j = (v.floor() as usize).min(nz - 2);
        (i, j, u - i as f32, v - j as f32)
    }

    /// Height of the far field at `(x, z)`, clamped to the extent: on the
    /// triangle under the point, each cell split along the diagonal from
    /// sample `(i, j)` to `(i + 1, j + 1)` like the terrain.
    #[must_use]
    pub fn far_height_at(&self, x: f32, z: f32) -> f32 {
        let (i, j, fu, fv) = self.far_locate(x, z);
        let h = |a: usize, b: usize| self.far.height_m(a, b);
        let (h00, h01, h10, h11) = (h(i, j), h(i, j + 1), h(i + 1, j), h(i + 1, j + 1));
        if fv >= fu {
            h00 + fv * (h01 - h00) + fu * (h11 - h01)
        } else {
            h00 + fu * (h10 - h00) + fv * (h11 - h10)
        }
    }

    /// Weights of the [`PAINTS`] of the far field at `(x, z)`: bilinear
    /// between the samples. They sum to 1.
    #[must_use]
    pub fn far_paint_at(&self, x: f32, z: f32) -> [f32; PAINTS.len()] {
        let mut w = [0.0; PAINTS.len()];
        if self.far.paint.is_empty() {
            w[0] = 1.0;
            return w;
        }
        let (i, j, fu, fv) = self.far_locate(x, z);
        for (di, dj, k) in [
            (0, 0, (1.0 - fu) * (1.0 - fv)),
            (1, 0, fu * (1.0 - fv)),
            (0, 1, (1.0 - fu) * fv),
            (1, 1, fu * fv),
        ] {
            w[usize::from(self.far.paint_at(i + di, j + dj)).min(PAINTS.len() - 1)] += k;
        }
        w
    }

    /// Blend from the terrain (0) to the far field (1) at `(x, z)`, and the
    /// point clamped onto the terrain grid. Without a terrain: 1.
    fn blend(terrain: Option<&Terrain>, x: f32, z: f32) -> (f32, f32, f32) {
        let Some(t) = terrain else {
            return (1.0, x, z);
        };
        let (lo, hi) = t.extent_xz();
        let d = distance_to_rect(x, z, lo, hi);
        (
            smoothstep(d / BLEND_M),
            x.clamp(lo[0], hi[0]),
            z.clamp(lo[1], hi[1]),
        )
    }

    /// Height of the world surface at `(x, z)`: the playable `terrain` on its
    /// own grid, the far field away from it, blended over [`BLEND_M`].
    #[must_use]
    pub fn height_at(&self, terrain: Option<&Terrain>, x: f32, z: f32) -> f32 {
        let (t, cx, cz) = Self::blend(terrain, x, z);
        if t >= 1.0 {
            return self.far_height_at(x, z);
        }
        // The clamped point lies on the grid, so the terrain answers.
        let near = terrain
            .and_then(|tr| tr.height_at(cx, cz))
            .unwrap_or_else(|| self.far_height_at(x, z));
        if t <= 0.0 {
            near
        } else {
            near + (self.far_height_at(x, z) - near) * t
        }
    }

    /// Weights of the [`PAINTS`] at `(x, z)`, blended like [`World::height_at`]
    /// (the terrain's paint is the paint of its nearest sample). They sum
    /// to 1.
    #[must_use]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn paint_at(&self, terrain: Option<&Terrain>, x: f32, z: f32) -> [f32; PAINTS.len()] {
        let (t, cx, cz) = Self::blend(terrain, x, z);
        let far = if t > 0.0 {
            self.far_paint_at(x, z)
        } else {
            [0.0; PAINTS.len()]
        };
        let Some(tr) = terrain.filter(|_| t < 1.0) else {
            return far;
        };
        let (lo, _) = tr.extent_xz();
        let c = tr.cell_m();
        let (nx, nz) = tr.dims();
        let i = (((cx - lo[0]) / c).round().max(0.0) as usize).min(nx - 1);
        let j = (((cz - lo[1]) / c).round().max(0.0) as usize).min(nz - 1);
        let mut w = far.map(|v| v * t);
        w[usize::from(tr.paint_at(i, j)).min(PAINTS.len() - 1)] += 1.0 - t;
        w
    }

    /// Structural rules of a world, as an error message. `terrain` is the
    /// map's playable terrain, which must lie inside the extent.
    ///
    /// # Errors
    /// A message naming the first broken rule.
    #[allow(clippy::cast_precision_loss)]
    pub fn validate(&self, terrain: Option<&Terrain>) -> Result<(), String> {
        let numbers = self.extent.iter().chain(&self.playable);
        if !numbers.copied().all(f32::is_finite) || self.snow_line.is_some_and(|s| !s.is_finite()) {
            return Err("world: a number is NaN or infinite".to_owned());
        }
        let f = &self.far;
        if f.cell == 0 || f.cell > MAX_FAR_CELL_M {
            return Err(format!(
                "world: far cell is {} m, allowed 1..={MAX_FAR_CELL_M}",
                f.cell
            ));
        }
        let [nx, nz] = f.size;
        if !(2..=MAX_FAR_SAMPLES).contains(&nx) || !(2..=MAX_FAR_SAMPLES).contains(&nz) {
            return Err(format!(
                "world: far size ({nx}, {nz}) is outside 2..={MAX_FAR_SAMPLES} samples per axis"
            ));
        }
        let n = usize::from(nx) * usize::from(nz);
        if f.heights.len() != n {
            return Err(format!(
                "world: far field has {} heights for {nx} x {nz} = {n} samples",
                f.heights.len()
            ));
        }
        if f.heights.iter().any(|h| h.abs() > MAX_FAR_HEIGHT_DM) {
            return Err(format!(
                "world: a far height is beyond +-{MAX_FAR_HEIGHT_DM} dm"
            ));
        }
        if !f.paint.is_empty() && f.paint.len() != n {
            return Err(format!(
                "world: far paint has {} values for {n} samples",
                f.paint.len()
            ));
        }
        if let Some(p) = f.paint.iter().find(|p| usize::from(**p) >= PAINTS.len()) {
            return Err(format!(
                "world: far paint value {p} is unknown (0..={})",
                PAINTS.len() - 1
            ));
        }
        let [x0, z0, x1, z1] = self.extent;
        let cell = f32::from(f.cell);
        let want = [f32::from(nx - 1) * cell, f32::from(nz - 1) * cell];
        if ((x1 - x0) - want[0]).abs() > 1e-3 || ((z1 - z0) - want[1]).abs() > 1e-3 {
            return Err(format!(
                "world: extent {:?} does not span the far field ({} x {} m)",
                self.extent, want[0], want[1]
            ));
        }
        if want[0] > MAX_EXTENT_M || want[1] > MAX_EXTENT_M {
            return Err(format!(
                "world: extent is larger than {MAX_EXTENT_M} m on an axis"
            ));
        }
        let [p0, q0, p1, q1] = self.playable;
        if p1 - p0 < 8.0 || q1 - q0 < 8.0 {
            return Err(format!(
                "world: playable {:?} is smaller than 8 m on an axis",
                self.playable
            ));
        }
        if p0 < x0 || q0 < z0 || p1 > x1 || q1 > z1 {
            return Err(format!(
                "world: playable {:?} is not inside the extent {:?}",
                self.playable, self.extent
            ));
        }
        if let Some(t) = terrain {
            let (lo, hi) = t.extent_xz();
            if lo[0] < x0 || lo[1] < z0 || hi[0] > x1 || hi[1] > z1 {
                return Err(format!(
                    "world: the terrain ({lo:?} to {hi:?}) is not inside the extent {:?}",
                    self.extent
                ));
            }
        }
        Ok(())
    }
}

/// LEB128 varints.
mod varint {
    pub fn push(out: &mut Vec<u8>, mut v: u32) {
        loop {
            #[allow(clippy::cast_possible_truncation)]
            let byte = (v & 0x7F) as u8;
            v >>= 7;
            if v == 0 {
                out.push(byte);
                return;
            }
            out.push(byte | 0x80);
        }
    }

    /// Reads one varint at `*at`, advancing it.
    pub fn read(bytes: &[u8], at: &mut usize) -> Result<u32, String> {
        let mut v: u32 = 0;
        for shift in (0..35).step_by(7) {
            let byte = *bytes.get(*at).ok_or("truncated varint")?;
            *at += 1;
            let bits = u32::from(byte & 0x7F);
            if shift == 28 && bits > 0x0F {
                return Err("varint overflows 32 bits".to_owned());
            }
            v |= bits << shift;
            if byte & 0x80 == 0 {
                return Ok(v);
            }
        }
        Err("varint longer than 5 bytes".to_owned())
    }
}

#[allow(clippy::cast_sign_loss)]
fn zigzag(v: i32) -> u32 {
    ((v << 1) ^ (v >> 31)) as u32
}

#[allow(clippy::cast_possible_wrap)]
fn unzigzag(v: u32) -> i32 {
    ((v >> 1) as i32) ^ -((v & 1) as i32)
}

/// Delta, zigzag and varint coding of heights (before base64).
#[must_use]
pub fn encode_heights(heights: &[i32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(heights.len() * 2);
    let mut prev = 0i32;
    for &h in heights {
        varint::push(&mut out, zigzag(h.wrapping_sub(prev)));
        prev = h;
    }
    out
}

/// Inverse of [`encode_heights`].
///
/// # Errors
/// A message for a truncated or overlong varint.
pub fn decode_heights(bytes: &[u8]) -> Result<Vec<i32>, String> {
    let mut out = Vec::with_capacity(bytes.len());
    let (mut at, mut prev) = (0, 0i32);
    while at < bytes.len() {
        prev = prev.wrapping_add(unzigzag(varint::read(bytes, &mut at)?));
        out.push(prev);
    }
    Ok(out)
}

/// Run-length coding of paint values: `(value, run varint)` pairs.
#[must_use]
pub fn encode_runs(values: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut k = 0;
    while k < values.len() {
        let v = values[k];
        let run = values[k..].iter().take_while(|x| **x == v).count();
        out.push(v);
        varint::push(&mut out, u32::try_from(run).unwrap_or(u32::MAX));
        k += run;
    }
    out
}

/// Inverse of [`encode_runs`].
///
/// # Errors
/// A message for a truncated pair, an empty run or more than `limit` values.
pub fn decode_runs(bytes: &[u8], limit: usize) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let v = bytes[at];
        at += 1;
        let run = varint::read(bytes, &mut at)? as usize;
        if run == 0 || out.len() + run > limit {
            return Err("bad paint run length".to_owned());
        }
        out.resize(out.len() + run, v);
    }
    Ok(out)
}

/// Most values a run-length paint may expand to.
const MAX_PAINT_VALUES: usize = MAX_FAR_SAMPLES as usize * MAX_FAR_SAMPLES as usize;

/// Serde adapter: heights as base64 of [`encode_heights`].
mod b64_delta {
    use serde::{de::Error, Deserialize, Deserializer, Serializer};

    #[allow(clippy::ptr_arg)]
    pub fn serialize<S: Serializer>(v: &Vec<i32>, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&crate::terrain::base64::encode(&super::encode_heights(v)))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<i32>, D::Error> {
        let text = String::deserialize(d)?;
        let bytes = crate::terrain::base64::decode(&text).map_err(D::Error::custom)?;
        super::decode_heights(&bytes).map_err(D::Error::custom)
    }
}

/// Serde adapter: paint as base64 of [`encode_runs`].
mod b64_rle {
    use serde::{de::Error, Deserialize, Deserializer, Serializer};

    #[allow(clippy::ptr_arg)]
    pub fn serialize<S: Serializer>(v: &Vec<u8>, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&crate::terrain::base64::encode(&super::encode_runs(v)))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(d)?;
        let bytes = crate::terrain::base64::decode(&text).map_err(D::Error::custom)?;
        super::decode_runs(&bytes, super::MAX_PAINT_VALUES).map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 64 x 48 m world, 8 m cells, a slope along x.
    pub(crate) fn world() -> World {
        let (nx, nz) = (9u16, 7u16);
        let heights = (0..nz)
            .flat_map(|_| (0..nx).map(|i| i32::from(i) * 10))
            .collect();
        World {
            extent: [-32.0, -24.0, 32.0, 24.0],
            playable: [-16.0, -12.0, 16.0, 12.0],
            far: FarField {
                cell: 8,
                size: [nx, nz],
                heights,
                paint: Vec::new(),
            },
            snow_line: None,
        }
    }

    #[test]
    fn varints_and_runs_round_trip() {
        let h = vec![
            0,
            1,
            -1,
            300,
            -70_000,
            100_000,
            100_000,
            i32::MIN,
            i32::MAX,
            5,
        ];
        assert_eq!(decode_heights(&encode_heights(&h)).unwrap(), h);
        // Smooth ground costs one byte per sample.
        let smooth: Vec<i32> = (0..100).map(|k| 120 + k / 3).collect();
        assert!(encode_heights(&smooth).len() <= 101);
        assert!(decode_heights(&[0x80]).is_err());
        assert!(decode_heights(&[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x01]).is_err());
        let p = vec![0, 0, 0, 3, 3, 1, 0, 0];
        let runs = encode_runs(&p);
        assert_eq!(runs.len(), 8);
        assert_eq!(decode_runs(&runs, 100).unwrap(), p);
        assert!(decode_runs(&runs, 7).is_err());
        assert!(decode_runs(&[1, 0], 10).is_err());
        assert!(decode_runs(&[1], 10).is_err());
    }

    #[test]
    fn far_field_interpolates_its_samples() {
        let w = world();
        // On a sample: the sample. A linear slope stays linear between the
        // samples, clamped beyond the extent.
        assert!((w.far_height_at(-32.0, 0.0) - 0.0).abs() < 1e-5);
        assert!((w.far_height_at(-24.0, 0.0) - 1.0).abs() < 1e-5);
        assert!((w.far_height_at(-20.0, 3.0) - 1.5).abs() < 1e-4);
        assert!((w.far_height_at(0.0, 0.0) - 4.0).abs() < 1e-4);
        assert!((w.far_height_at(100.0, 0.0) - 8.0).abs() < 1e-4);
        assert!((w.far_height_at(-100.0, -100.0)).abs() < 1e-4);
    }

    #[test]
    fn the_terrain_wins_inside_its_grid_and_blends_out() {
        let w = world();
        let t = Terrain::flat([-16, -16], 4, [9, 9], 500);
        // Inside the 16 x 16 m grid: exactly the terrain.
        assert!((w.height_at(Some(&t), 0.0, 0.0) - 5.0).abs() < 1e-5);
        assert!((w.height_at(Some(&t), 8.0, -8.0) - 5.0).abs() < 1e-5);
        // Just outside: still close; beyond the blend band: the far field.
        assert!((w.height_at(Some(&t), 8.5, 0.0) - 5.0).abs() < 0.1);
        let far = w.far_height_at(8.0 + BLEND_M + 1.0, 0.0);
        assert!((w.height_at(Some(&t), 8.0 + BLEND_M + 1.0, 0.0) - far).abs() < 1e-5);
        // Without a terrain: the far field everywhere.
        assert!((w.height_at(None, 0.0, 0.0) - 4.0).abs() < 1e-4);
        // Paint: the terrain's inside, the far field's outside.
        let mut t = t;
        t.paint = vec![3; 81];
        assert_eq!(w.paint_at(Some(&t), 0.0, 0.0)[3], 1.0);
        assert_eq!(w.paint_at(Some(&t), 30.0, 0.0)[0], 1.0);
        let mid = w.paint_at(Some(&t), 8.0 + BLEND_M * 0.5, 0.0);
        assert!((mid.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        assert!(mid[0] > 0.0 && mid[3] > 0.0);
    }

    #[test]
    fn far_paint_blends_between_samples() {
        let mut w = world();
        w.far.paint = vec![0; 63];
        w.far.paint[1] = 2;
        let p = w.far_paint_at(-28.0, -24.0);
        assert!((p[0] - 0.5).abs() < 1e-5 && (p[2] - 0.5).abs() < 1e-5);
    }

    #[test]
    fn validation_rules() {
        let w = world();
        assert!(w.validate(None).is_ok());
        let mut bad = w.clone();
        bad.extent[2] = 40.0;
        assert!(bad.validate(None).is_err(), "extent off the far field");
        let mut bad = w.clone();
        bad.playable = [-40.0, 0.0, 0.0, 10.0];
        assert!(bad.validate(None).is_err(), "playable outside");
        let mut bad = w.clone();
        bad.far.heights.pop();
        assert!(bad.validate(None).is_err());
        let mut bad = w.clone();
        bad.far.paint = vec![9; 63];
        assert!(bad.validate(None).is_err());
        let mut bad = w.clone();
        bad.far.cell = 0;
        assert!(bad.validate(None).is_err());
        let mut bad = w.clone();
        bad.snow_line = Some(f32::NAN);
        assert!(bad.validate(None).is_err());
        let t = Terrain::flat([-80, 0], 4, [3, 3], 0);
        assert!(w.validate(Some(&t)).is_err(), "terrain outside the extent");
    }
}

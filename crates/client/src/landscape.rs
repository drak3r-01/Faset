//! The landscape beyond the playable area (lot 6.12): far terrain, horizon
//! silhouette and the woods at the edge, generated from the map's
//! [`Surroundings`] block.
//!
//! The far terrain is one low-detail mesh: concentric rectangular rings
//! around the playable area whose spacing grows with the distance (3 m near
//! the edge, hundreds of metres at the horizon), every ring with the same
//! number of vertices so the rings stitch into a strip. The height and the
//! ground of a point come from the border of the nearest side (blended at
//! the corners), then the horizon silhouette is added far away. The mesh uses
//! the terrain materials of the map (no new asset) and has no collision: the
//! simulation's walls and kill volumes still hold the players. The border
//! profiles are made so the ground never invites walking out (a cliff, the
//! sea, a steep slope, dense woods).
//!
//! Trees on forest borders are plain vegetation instances
//! ([`PlacedPlant`]) for the renderer's instanced foliage layer, which already
//! culls and uses levels of detail. Everything here is deterministic and
//! needs no GPU.
//!
//! Decisions of lot 6.12:
//! - 19 rings, out to 1300 m; 40 segments per side (3 040 vertices, about
//!   5 800 triangles in all);
//! - the woods are a band of at most [`WOODS_BAND_M`] metres from the edge,
//!   about 1.2 trees per 100 m^2, capped at [`MAX_TREES`] over the map;
//! - the horizon starts to rise 120 m (treeline, dunes) or 180 m (mountains)
//!   from the edge, so it never blocks the playable area's own sky.

use faset_mapfmt::geometry::{terrain_material, TERRAIN_MATERIALS};
use faset_mapfmt::vegetation::PlacedPlant;
use faset_mapfmt::{Border, Horizon, MapFile, Species, Surroundings};
use faset_render::glam::Vec3;
use faset_render::Vertex;

/// Distance of each ring from the playable rectangle, metres.
pub const RING_OFFSETS: [f32; 19] = [
    0.0, 1.5, 3.0, 5.0, 8.0, 12.0, 18.0, 26.0, 38.0, 55.0, 80.0, 115.0, 160.0, 225.0, 320.0, 450.0,
    650.0, 900.0, 1300.0,
];
/// Segments along each side of a ring.
pub const SEGMENTS: usize = 40;
/// Width of the band of trees beside a forest border, metres.
pub const WOODS_BAND_M: f32 = 90.0;
/// Most trees over a whole map.
pub const MAX_TREES: usize = 2400;
/// Spacing of the tree grid, metres (about 1.2 trees per 100 m^2).
const TREE_CELL_M: f32 = 9.0;

const GRASS: u8 = TERRAIN_MATERIALS[0];
const SAND: u8 = TERRAIN_MATERIALS[1];
const ROCK: u8 = TERRAIN_MATERIALS[2];
const SNOW: u8 = TERRAIN_MATERIALS[3];

/// The far terrain mesh and the woods of one map.
#[derive(Debug, Clone, Default)]
pub struct Landscape {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    /// Trees to add to the map's vegetation layer.
    pub plants: Vec<PlacedPlant>,
}

/// Builds the landscape of a map; `None` when it has no surroundings.
///
/// A map with a `world` block (lot 8.1) draws its own far terrain (see
/// [`crate::world_terrain`]): its landscape has no mesh, and the woods stand
/// on the world's ground.
#[must_use]
pub fn build(map: &MapFile) -> Option<Landscape> {
    let s = map.surroundings.as_ref()?;
    let field = Field {
        s,
        palette: &map.theme.palette,
    };
    if let Some(world) = &map.world {
        let mut plants = field.woods();
        for p in &mut plants {
            p.position.y = world.height_at(map.terrain.as_ref(), p.position.x, p.position.z) - 0.15;
        }
        return Some(Landscape {
            vertices: Vec::new(),
            indices: Vec::new(),
            plants,
        });
    }
    let (vertices, indices) = field.mesh();
    Some(Landscape {
        vertices,
        indices,
        plants: field.woods(),
    })
}

fn hash(seed: u32, a: i32, b: i32, salt: u32) -> u32 {
    let mut h = seed ^ salt.wrapping_mul(0x9E37_79B9);
    for v in [a as u32, b as u32] {
        h ^= v.wrapping_mul(0x85EB_CA6B);
        h = h.rotate_left(13).wrapping_mul(0xC2B2_AE35);
        h ^= h >> 16;
    }
    h
}

#[allow(clippy::cast_precision_loss)]
fn unit(h: u32) -> f32 {
    (h >> 8) as f32 / (1u32 << 24) as f32
}

fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Smooth value noise in `[-1, 1]` with features of about `scale` metres.
#[allow(clippy::cast_possible_truncation)]
fn noise(seed: u32, x: f32, z: f32, scale: f32, salt: u32) -> f32 {
    let (u, v) = (x / scale, z / scale);
    let (i, j) = (u.floor(), v.floor());
    let (fu, fv) = (smooth(u - i), smooth(v - j));
    let at = |di: i32, dj: i32| unit(hash(seed, i as i32 + di, j as i32 + dj, salt)) * 2.0 - 1.0;
    lerp(
        lerp(at(0, 0), at(1, 0), fu),
        lerp(at(0, 1), at(1, 1), fu),
        fv,
    )
}

/// The generator for one map.
struct Field<'a> {
    s: &'a Surroundings,
    palette: &'a str,
}

impl Field<'_> {
    /// Metres outside each side of the rectangle, `[north, east, south,
    /// west]`, at least 0.
    fn outward(&self, x: f32, z: f32) -> [f32; 4] {
        let [x0, z0, x1, z1] = self.s.area;
        [z0 - z, x - x1, z - z1, x0 - x].map(|d| d.max(0.0))
    }

    /// The sea height of the map, or a level well under the ground.
    fn sea(&self) -> f32 {
        self.s.sea_level.unwrap_or(self.s.ground - 12.0)
    }

    /// The ground the map's palette gives.
    fn base_ground(&self) -> u8 {
        terrain_material(self.palette, 0)
    }

    /// Height and material of one border at `d` metres from the edge.
    fn border(&self, b: Border, d: f32, x: f32, z: f32) -> (f32, u8) {
        let g = self.s.ground;
        let seed = self.s.seed;
        let n1 = noise(seed, x, z, 40.0, 1);
        let n2 = noise(seed, x, z, 170.0, 2);
        let rise = d / (d + 25.0);
        let sea = self.sea();
        match b {
            Border::Forest => {
                let ground = if self.palette == "snow" { SNOW } else { GRASS };
                (
                    g + 0.06 * d.min(400.0) + (4.0 + 3.5 * n1 + 8.0 * n2) * rise,
                    ground,
                )
            }
            Border::Cliff => {
                let face = smooth((d - 1.5) / 3.5);
                if self.s.sea_level.is_some() {
                    // Face down to a beach, then the seabed.
                    let foot = sea + 0.8;
                    let beach = smooth((d - 5.0) / 40.0);
                    let bank = smooth((d - 90.0) / 230.0);
                    let low = lerp(foot, sea - 4.0, beach);
                    let h = lerp(g, low, face);
                    let h = lerp(h, sea - 0.3, bank);
                    let ground = if d < 5.0 { ROCK } else { SAND };
                    (h, ground)
                } else {
                    let floor = g - 28.0 + 2.0 * n1 * rise;
                    (
                        lerp(g, floor, face),
                        if d < 5.0 { ROCK } else { self.base_ground() },
                    )
                }
            }
            Border::Sea => {
                let drop = smooth((d - 1.0) / 8.0);
                let bed = sea - 6.0;
                let bank = smooth((d - 60.0) / 260.0);
                let h = lerp(lerp(g, bed, drop), sea - 0.3, bank);
                (h, if d < 6.0 { ROCK } else { SAND })
            }
            Border::Slope => {
                let climb = (d * 0.85).min(30.0) + (d - 35.0).max(0.0) * 0.25;
                let h = g + climb + n1 * 4.0 * rise;
                let ground = if d < 40.0 {
                    ROCK
                } else if self.palette == "snow" {
                    SNOW
                } else {
                    self.base_ground()
                };
                (h, ground)
            }
            Border::Dunes => {
                let h = g
                    + (2.0 + 5.0 * (0.5 + 0.5 * n1)) * smooth(d / 20.0)
                    + 0.03 * d.min(400.0)
                    + 4.0 * n2 * rise;
                (h, SAND)
            }
            Border::Rocks => {
                let h = g
                    + 0.25 * d.min(300.0)
                    + (n1.abs() * 14.0 + n2.abs() * 16.0) * smooth(d / 30.0);
                (h, ROCK)
            }
        }
    }

    /// Height added by the horizon silhouette, and its material when it
    /// takes over.
    fn horizon(&self, d: f32, x: f32, z: f32, h: f32) -> (f32, Option<u8>) {
        let seed = self.s.seed;
        let n1 = noise(seed, x, z, 90.0, 3);
        match self.s.horizon {
            Horizon::Flat => (0.0, None),
            Horizon::Mountains => {
                let m = smooth((d - 180.0) / 470.0);
                let ridge = 1.0 - noise(seed, x, z, 260.0, 4).abs();
                let add = m * (40.0 + 110.0 * ridge.powf(1.5));
                let material = if self.palette == "snow" && h + add - self.s.ground > 110.0 {
                    Some(SNOW)
                } else if m > 0.25 {
                    Some(ROCK)
                } else {
                    None
                };
                (add, material)
            }
            Horizon::Treeline => {
                let t = smooth((d - 120.0) / 300.0);
                (t * (16.0 + 10.0 * n1), (t > 0.3).then_some(GRASS))
            }
            Horizon::Dunes => {
                let t = smooth((d - 120.0) / 380.0);
                (
                    t * (14.0 + 10.0 * (0.5 + 0.5 * n1)),
                    (t > 0.3).then_some(SAND),
                )
            }
        }
    }

    /// Weights of the four sides at a point: 1 beside a side, 0.5 each in a
    /// corner. All 0 inside the rectangle.
    fn weights(out: [f32; 4]) -> [f32; 4] {
        let d = out.iter().copied().fold(0.0, f32::max);
        if d <= 0.0 {
            return [0.0; 4];
        }
        let w = out.map(|o| (o / d).powi(8));
        let sum: f32 = w.iter().sum();
        w.map(|v| v / sum)
    }

    /// Height and ground material of the far terrain at `(x, z)`.
    fn sample(&self, x: f32, z: f32) -> (f32, u8) {
        let out = self.outward(x, z);
        let d = out.iter().copied().fold(0.0, f32::max);
        let w = Self::weights(out);
        let borders = self.s.borders();
        let mut h = 0.0;
        let mut best = (0.0, self.base_ground());
        for (k, border) in borders.iter().enumerate() {
            if w[k] <= 0.0 {
                continue;
            }
            let (hk, mk) = self.border(*border, d, x, z);
            h += w[k] * hk;
            if w[k] > best.0 {
                best = (w[k], mk);
            }
        }
        if d <= 0.0 {
            return (self.s.ground, best.1);
        }
        let (add, material) = self.horizon(d, x, z, h);
        (h + add, material.unwrap_or(best.1))
    }

    /// Vertex of ring `k` at index `m` around it.
    #[allow(clippy::cast_precision_loss)]
    fn ring_point(&self, k: usize, m: usize) -> (f32, f32) {
        let [x0, z0, x1, z1] = self.s.area;
        let d = RING_OFFSETS[k];
        let (side, i) = (m / SEGMENTS, m % SEGMENTS);
        let u = i as f32 / SEGMENTS as f32;
        let (lx, hx, lz, hz) = (x0 - d, x1 + d, z0 - d, z1 + d);
        match side {
            0 => (lerp(lx, hx, u), lz),
            1 => (hx, lerp(lz, hz, u)),
            2 => (lerp(hx, lx, u), hz),
            _ => (lx, lerp(hz, lz, u)),
        }
    }

    fn mesh(&self) -> (Vec<Vertex>, Vec<u32>) {
        let n = 4 * SEGMENTS;
        let rings = RING_OFFSETS.len();
        let mut pos = Vec::with_capacity(rings * n);
        let mut material = Vec::with_capacity(rings * n);
        for k in 0..rings {
            for m in 0..n {
                let (x, z) = self.ring_point(k, m);
                let (y, mat) = self.sample(x, z);
                pos.push(Vec3::new(x, y, z));
                material.push(mat);
            }
        }
        let at = |k: usize, m: usize| (k * n + m % n) as u32;
        let mut indices = Vec::with_capacity((rings - 1) * n * 6);
        for k in 0..rings - 1 {
            for m in 0..n {
                let (a, b) = (at(k, m), at(k, m + 1));
                let (c, d) = (at(k + 1, m), at(k + 1, m + 1));
                indices.extend([a, b, c, b, d, c]);
            }
        }
        let mut normals = vec![Vec3::ZERO; pos.len()];
        for t in indices.as_chunks::<3>().0 {
            let p = |i: usize| pos[t[i] as usize];
            let face = (p(1) - p(0)).cross(p(2) - p(0));
            for &i in t {
                normals[i as usize] += face;
            }
        }
        let vertices = pos
            .iter()
            .zip(&normals)
            .zip(&material)
            .map(|((p, n), m)| Vertex::new(*p, n.try_normalize().unwrap_or(Vec3::Y), [255; 3], *m))
            .collect();
        (vertices, indices)
    }

    /// The trees along the forest borders.
    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    fn woods(&self) -> Vec<PlacedPlant> {
        if !self.s.borders().contains(&Border::Forest) {
            return Vec::new();
        }
        let [x0, z0, x1, z1] = self.s.area;
        let reach = WOODS_BAND_M;
        let (i0, i1) = (
            ((x0 - reach) / TREE_CELL_M).floor() as i32,
            ((x1 + reach) / TREE_CELL_M).ceil() as i32,
        );
        let (j0, j1) = (
            ((z0 - reach) / TREE_CELL_M).floor() as i32,
            ((z1 + reach) / TREE_CELL_M).ceil() as i32,
        );
        let seed = self.s.seed;
        let mut out = Vec::new();
        for j in j0..j1 {
            for i in i0..i1 {
                let x = (i as f32 + unit(hash(seed, i, j, 10))) * TREE_CELL_M;
                let z = (j as f32 + unit(hash(seed, i, j, 11))) * TREE_CELL_M;
                let outward = self.outward(x, z);
                let d = outward.iter().copied().fold(0.0, f32::max);
                if !(1.0..=reach).contains(&d) {
                    continue;
                }
                let forest: f32 = Self::weights(outward)
                    .iter()
                    .zip(self.s.borders())
                    .filter(|(_, b)| *b == Border::Forest)
                    .map(|(w, _)| w)
                    .sum();
                if forest < 0.8 {
                    continue;
                }
                let (y, _) = self.sample(x, z);
                let pick = unit(hash(seed, i, j, 12));
                let species = if self.palette == "snow" || pick < 0.4 {
                    Species::Pine
                } else {
                    Species::Oak
                };
                let (lo, hi) = species.info().scale;
                out.push(PlacedPlant {
                    species,
                    position: Vec3::new(x, y - 0.15, z),
                    yaw: unit(hash(seed, i, j, 13)) * std::f32::consts::TAU,
                    scale: lerp(lo, hi, unit(hash(seed, i, j, 14))) * 1.4,
                    variant: hash(seed, i, j, 15) % faset_mapfmt::vegetation::VARIANTS,
                });
                if out.len() >= MAX_TREES {
                    return out;
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn surroundings(n: Border, e: Border, s: Border, w: Border, h: Horizon) -> Surroundings {
        Surroundings {
            area: [-40.0, -30.0, 40.0, 30.0],
            ground: 1.0,
            sea_level: Some(-1.0),
            north: n,
            east: e,
            south: s,
            west: w,
            horizon: h,
            haze: 1.0,
            seed: 5,
        }
    }

    fn field(s: &Surroundings) -> Field<'_> {
        Field {
            s,
            palette: "nature",
        }
    }

    #[test]
    fn the_mesh_is_a_valid_strip_with_up_facing_triangles() {
        let s = surroundings(
            Border::Forest,
            Border::Cliff,
            Border::Sea,
            Border::Slope,
            Horizon::Mountains,
        );
        let (v, idx) = field(&s).mesh();
        assert_eq!(v.len(), RING_OFFSETS.len() * 4 * SEGMENTS);
        assert_eq!(idx.len(), (RING_OFFSETS.len() - 1) * 4 * SEGMENTS * 6);
        assert!(idx.iter().all(|&i| (i as usize) < v.len()));
        assert!(v.iter().all(|v| v.position.iter().all(|c| c.is_finite())));
        // Seen from above every triangle is counter-clockwise, except where
        // a cliff or a slope stands vertical (a degenerate or steep face is
        // still never upside down by a margin).
        let mut up = 0;
        for t in idx.as_chunks::<3>().0 {
            let p = |i: usize| Vec3::from_array(v[t[i] as usize].position);
            let n = (p(1) - p(0)).cross(p(2) - p(0));
            assert!(n.y > -1e-3, "a triangle faces down: {n:?}");
            up += usize::from(n.y > 0.0);
        }
        assert!(up > idx.len() / 6);
    }

    #[test]
    fn the_edge_ring_meets_the_playable_floor() {
        let s = surroundings(
            Border::Sea,
            Border::Sea,
            Border::Sea,
            Border::Sea,
            Horizon::Flat,
        );
        let (v, _) = field(&s).mesh();
        for vert in &v[..4 * SEGMENTS] {
            assert!((vert.position[1] - s.ground).abs() < 1e-4);
        }
    }

    #[test]
    fn borders_look_like_what_they_are_called() {
        let mk = |b| surroundings(b, b, b, b, Horizon::Flat);
        let h = |b: Border, d: f32| field(&mk(b)).sample(0.0, -30.0 - d).0;
        // The sea drops under the water line; the cliff falls to the beach.
        assert!(h(Border::Sea, 12.0) < -1.0 - 4.0);
        assert!(h(Border::Cliff, 8.0) < 0.0 && h(Border::Cliff, 0.0) > 0.9);
        // A slope climbs steeply; forests roll gently up; rocks rise.
        assert!(h(Border::Slope, 30.0) > 20.0);
        let mean = |b: Border| {
            (0..9)
                .map(|k| field(&mk(b)).sample(-40.0 + 10.0 * k as f32, -80.0).0)
                .sum::<f32>()
                / 9.0
        };
        assert!(mean(Border::Forest) > 2.0, "{}", mean(Border::Forest));
        assert!(h(Border::Rocks, 100.0) > 15.0);
        // Seen from the middle of a side, a corner blends two borders.
        let s = surroundings(
            Border::Slope,
            Border::Sea,
            Border::Sea,
            Border::Sea,
            Horizon::Flat,
        );
        let f = field(&s);
        let side = f.sample(0.0, -30.0 - 40.0).0;
        let corner = f.sample(40.0 + 40.0, -30.0 - 40.0).0;

        assert!(side > corner, "{side} {corner}");
    }

    #[test]
    fn the_horizon_rises_far_away_only() {
        let mk = |h| {
            surroundings(
                Border::Forest,
                Border::Forest,
                Border::Forest,
                Border::Forest,
                h,
            )
        };
        let near = |h| field(&mk(h)).sample(0.0, -30.0 - 100.0).0;
        let far = |h| field(&mk(h)).sample(0.0, -30.0 - 1200.0).0;
        for h in [Horizon::Mountains, Horizon::Treeline, Horizon::Dunes] {
            assert!(
                far(h) > far(Horizon::Flat) + 5.0,
                "{h:?} stands up far away"
            );
            assert!((near(h) - near(Horizon::Flat)).abs() < 1e-3, "{h:?} near");
        }
        assert!(far(Horizon::Mountains) > far(Horizon::Treeline));
    }

    #[test]
    fn woods_stand_only_beside_forest_borders_and_are_deterministic() {
        let s = surroundings(
            Border::Forest,
            Border::Sea,
            Border::Sea,
            Border::Sea,
            Horizon::Flat,
        );
        let a = field(&s).woods();
        let b = field(&s).woods();
        assert_eq!(a, b);
        assert!(a.len() > 100 && a.len() <= MAX_TREES, "{} trees", a.len());
        for p in &a {
            assert!(!s.contains(p.position.x, p.position.z), "tree in the arena");
            assert!(p.position.z < -30.0, "tree off the forest side");
            assert!(p.position.is_finite());
        }
        let none = surroundings(
            Border::Sea,
            Border::Sea,
            Border::Sea,
            Border::Sea,
            Horizon::Flat,
        );
        assert!(field(&none).woods().is_empty());
    }

    #[test]
    fn a_map_without_surroundings_has_no_landscape() {
        let map =
            faset_mapfmt::parse(include_bytes!("../../../assets/testdata/example.ron")).unwrap();
        assert!(build(&map).is_none());
    }
}

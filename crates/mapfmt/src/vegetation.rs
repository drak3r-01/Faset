//! Vegetation (lot 6.11): density rules per ground material and hand-placed
//! plants, scattered deterministically from a seed.
//!
//! A map's optional `vegetation` block holds a seed, density rules ("so many
//! plants of a species per 100 m^2 of this ground") and hand-placed plants.
//! [`scatter`] turns it into the list of plants of the map: the same map
//! always gives the same plants, on every machine, so the client draws
//! exactly what the bots' sight is blocked by.
//!
//! Grounds are named like materials: the terrain grounds (`grass`, `sand`,
//! `rock`, `snow`, `dirt`; a terrain sample painted `auto` takes the ground
//! of the map palette, and slopes steeper than [`STEEP_DEG`] are `rock`) and
//! the material of the flat top face of a box or prism brush (`grass`, `sand`,
//! `leaves`...). A ground written `terrain:<name>` only matches the terrain.
//!
//! Decisions of lot 6.11:
//! - species: eight code-generated species ([`Species`]), each in
//!   [`VARIANTS`] shape variants; their look comes from `faset-assets`;
//! - placement: one candidate per cell of a jittered grid whose cell holds
//!   on average one plant (`sqrt(100 / density)` metres), kept when the ground
//!   under it has the rule's material, is not covered by a brush, is not
//!   steeper than the species allows, is not under water and is at least
//!   [`CLEARANCE_M`] from a spawn or a pickup;
//! - symmetry: on a symmetric map, rules scatter one half and mirror it, a
//!   candidate being kept only when its twin is valid too, so both teams see
//!   the same cover; hand-placed plants are mirrored by
//!   [`crate::expand_symmetry`] like props. The expansion records the
//!   symmetry in [`Vegetation::symmetry`] so that an expanded map scatters
//!   the same plants as its file;
//! - gameplay: plants have no collision (players and bullets go through
//!   them); the larger ones block sight ([`SpeciesInfo::sight`]);
//! - limits: at most [`MAX_DENSITY`] plants per 100 m^2 per rule and
//!   [`MAX_PLANTS`] plants per map (the rest is dropped, deterministically).

use std::collections::HashMap;

use faset_core::glam::Vec3;
use serde::{Deserialize, Serialize};

use crate::geometry::brush_hull_points;
use crate::schema::{Brush, MapFile, Pos, Symmetry, Zone, ZoneKind, GRID_UNIT_M};
use crate::terrain::{Terrain, PAINTS};

/// Shape variants of each species.
pub const VARIANTS: u32 = 3;
/// Largest density of one rule, plants per 100 m^2 (4 per m^2).
pub const MAX_DENSITY: f32 = 400.0;
/// Most plants one map can hold, rules and hand-placed plants together.
pub const MAX_PLANTS: usize = 100_000;
/// Terrain steeper than this, in degrees, is `rock` ground.
pub const STEEP_DEG: f32 = 35.0;
/// No plant grows closer than this to a spawn or a pickup, metres.
pub const CLEARANCE_M: f32 = 1.5;
/// Prefix of a rule ground that only matches the terrain (`terrain:snow`
/// leaves the brushes of `snow` material bare).
pub const TERRAIN_ONLY: &str = "terrain:";

/// A code-generated plant species.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Species {
    /// Broadleaf tree with a round crown.
    Oak,
    /// Conifer: stacked cones.
    Pine,
    /// Slim pale tree with a tall crown.
    Birch,
    /// Curved trunk and drooping fronds.
    Palm,
    /// Round shrub.
    Bush,
    /// Ground boulder.
    Rock,
    /// Tuft of grass blades.
    Grass,
    /// Rosette of arching fronds.
    Fern,
}

/// A vertical cylinder that blocks sight, in the plant's local frame (base
/// at the origin, scale 1), metres.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SightCylinder {
    pub radius: f32,
    pub bottom: f32,
    pub top: f32,
}

/// Placement and gameplay data of a species.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpeciesInfo {
    /// Uniform scale range of scattered plants.
    pub scale: (f32, f32),
    /// Steepest ground the species grows on, degrees.
    pub max_slope_deg: f32,
    /// What blocks the bots' sight (empty: nothing, e.g. grass).
    pub sight: &'static [SightCylinder],
}

const fn cyl(radius: f32, bottom: f32, top: f32) -> SightCylinder {
    SightCylinder {
        radius,
        bottom,
        top,
    }
}

impl Species {
    /// Every species, in id order.
    pub const ALL: [Self; 8] = [
        Self::Oak,
        Self::Pine,
        Self::Birch,
        Self::Palm,
        Self::Bush,
        Self::Rock,
        Self::Grass,
        Self::Fern,
    ];

    /// Lower-case name, as written in the editor.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Oak => "oak",
            Self::Pine => "pine",
            Self::Birch => "birch",
            Self::Palm => "palm",
            Self::Bush => "bush",
            Self::Rock => "rock",
            Self::Grass => "grass",
            Self::Fern => "fern",
        }
    }

    /// Index in [`Species::ALL`].
    #[must_use]
    pub fn index(self) -> usize {
        self as usize
    }

    /// Placement and gameplay data. The sight cylinders follow the meshes of
    /// `faset-assets` (checked by its tests).
    #[must_use]
    pub fn info(self) -> SpeciesInfo {
        const OAK: [SightCylinder; 2] = [cyl(0.25, 0.0, 1.6), cyl(1.35, 1.6, 4.1)];
        const PINE: [SightCylinder; 2] = [cyl(0.2, 0.0, 1.0), cyl(1.1, 1.0, 5.6)];
        const BIRCH: [SightCylinder; 2] = [cyl(0.15, 0.0, 2.0), cyl(0.9, 2.0, 5.0)];
        const PALM: [SightCylinder; 2] = [cyl(0.2, 0.0, 3.8), cyl(1.4, 3.8, 4.4)];
        const BUSH: [SightCylinder; 1] = [cyl(0.75, 0.0, 1.2)];
        const ROCK: [SightCylinder; 1] = [cyl(0.55, 0.0, 0.6)];
        match self {
            Self::Oak => SpeciesInfo {
                scale: (0.85, 1.2),
                max_slope_deg: 30.0,
                sight: &OAK,
            },
            Self::Pine => SpeciesInfo {
                scale: (0.8, 1.3),
                max_slope_deg: 32.0,
                sight: &PINE,
            },
            Self::Birch => SpeciesInfo {
                scale: (0.85, 1.15),
                max_slope_deg: 30.0,
                sight: &BIRCH,
            },
            Self::Palm => SpeciesInfo {
                scale: (0.85, 1.15),
                max_slope_deg: 25.0,
                sight: &PALM,
            },
            Self::Bush => SpeciesInfo {
                scale: (0.75, 1.25),
                max_slope_deg: 35.0,
                sight: &BUSH,
            },
            Self::Rock => SpeciesInfo {
                scale: (0.6, 1.6),
                max_slope_deg: 60.0,
                sight: &ROCK,
            },
            Self::Grass => SpeciesInfo {
                scale: (0.7, 1.3),
                max_slope_deg: 40.0,
                sight: &[],
            },
            Self::Fern => SpeciesInfo {
                scale: (0.7, 1.2),
                max_slope_deg: 38.0,
                sight: &[],
            },
        }
    }
}

/// The vegetation block of a map. Serialised as `vegetation: (...)`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Vegetation {
    /// Seed of the rules' scattering.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub seed: u64,
    /// Density rules, scattered in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<DensityRule>,
    /// Hand-placed plants.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plants: Vec<Plant>,
    /// Symmetry the rules scatter with when the map itself declares none:
    /// written by [`crate::expand_symmetry`], left out of hand-written files.
    #[serde(default, skip_serializing_if = "is_no_symmetry")]
    pub symmetry: Symmetry,
}

/// "`density` plants of `species` per 100 m^2 of the ground `on`".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DensityRule {
    /// Ground material: a terrain ground or a brush material.
    pub on: String,
    pub species: Species,
    /// Plants per 100 m^2, `0..=MAX_DENSITY`.
    pub density: f32,
}

/// A hand-placed plant, standing on `at`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plant {
    pub species: Species,
    pub at: Pos,
    /// Rotation about +Y in degrees.
    #[serde(default, skip_serializing_if = "is_zero_f32")]
    pub yaw: f32,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub scale: f32,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero_u64(v: &u64) -> bool {
    *v == 0
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero_f32(v: &f32) -> bool {
    *v == 0.0
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_one(v: &f32) -> bool {
    *v == 1.0
}

fn one() -> f32 {
    1.0
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_no_symmetry(s: &Symmetry) -> bool {
    *s == Symmetry::None
}

impl Vegetation {
    /// Structural rules, as an error message.
    ///
    /// # Errors
    /// A message naming the first broken rule.
    pub fn validate(&self) -> Result<(), String> {
        for r in &self.rules {
            if r.on.is_empty() {
                return Err("vegetation rule has an empty ground".to_owned());
            }
            if !(0.0..=MAX_DENSITY).contains(&r.density) {
                return Err(format!(
                    "vegetation density {} is outside 0..={MAX_DENSITY} per 100 m^2",
                    r.density
                ));
            }
        }
        if self.plants.len() > MAX_PLANTS {
            return Err(format!(
                "{} hand-placed plants, at most {MAX_PLANTS}",
                self.plants.len()
            ));
        }
        let finite = self.plants.iter().all(|p| {
            p.at.iter().all(|v| v.is_finite())
                && p.yaw.is_finite()
                && p.scale.is_finite()
                && p.scale > 0.0
        });
        if !finite {
            return Err("a plant has a non-finite position, yaw or scale".to_owned());
        }
        Ok(())
    }
}

/// One plant of the map, ready to draw and to block sight.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlacedPlant {
    pub species: Species,
    /// Base of the plant, metres.
    pub position: Vec3,
    /// Rotation about +Y, radians (counter-clockwise seen from above).
    pub yaw: f32,
    pub scale: f32,
    /// Shape variant, `0..VARIANTS`.
    pub variant: u32,
}

impl PlacedPlant {
    /// The sight blockers of this plant in world space: `(centre x, centre
    /// z, radius, bottom y, top y)`.
    pub fn sight(&self) -> impl Iterator<Item = (f32, f32, f32, f32, f32)> + '_ {
        self.species.info().sight.iter().map(|c| {
            (
                self.position.x,
                self.position.z,
                c.radius * self.scale,
                self.position.y + c.bottom * self.scale,
                self.position.y + c.top * self.scale,
            )
        })
    }
}

fn splitmix(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Deterministic hash of a rule's cell and a stream.
#[allow(clippy::cast_sign_loss)]
fn cell_hash(seed: u64, rule: usize, i: i64, j: i64, stream: u64) -> u64 {
    let mut h = splitmix(seed ^ 0x6c62_272e_07bb_0142);
    h = splitmix(h ^ rule as u64);
    h = splitmix(h ^ i as u64);
    h = splitmix(h ^ (j as u64).rotate_left(29));
    splitmix(h ^ stream)
}

/// Uniform value in `[0, 1)` from a hash.
#[allow(clippy::cast_precision_loss)]
fn unit(h: u64) -> f32 {
    (h >> 40) as f32 / (1u64 << 24) as f32
}

/// Mirrors a point `(x, z)` and a yaw (radians) like the map's symmetry.
#[must_use]
pub fn mirror(symmetry: Symmetry, x: f32, z: f32, yaw: f32) -> (f32, f32, f32) {
    use std::f32::consts::PI;
    match symmetry {
        Symmetry::None => (x, z, yaw),
        Symmetry::MirrorX => (-x + 0.0, z, -yaw + 0.0),
        Symmetry::MirrorZ => (x, -z + 0.0, PI - yaw),
        Symmetry::Rotate180 => (-x + 0.0, -z + 0.0, yaw + PI),
    }
}

/// The half of a symmetric map that the rules scatter (the other half is
/// its mirror). Points within 5 cm of the mirror plane are left out.
fn in_scattered_half(symmetry: Symmetry, x: f32, z: f32) -> bool {
    match symmetry {
        Symmetry::None => true,
        Symmetry::MirrorX | Symmetry::Rotate180 => x < -0.05,
        Symmetry::MirrorZ => z < -0.05,
    }
}

/// Axis-aligned box in metres.
#[derive(Debug, Clone, Copy)]
struct Bounds {
    min: Vec3,
    max: Vec3,
}

impl Bounds {
    fn contains(&self, p: Vec3) -> bool {
        p.cmpgt(self.min).all() && p.cmplt(self.max).all()
    }
}

/// Flat top face of a box or prism brush.
#[derive(Debug, Clone)]
struct Top {
    min: [f32; 2],
    max: [f32; 2],
    y: f32,
    material: String,
    /// Footprint `(x, z)` of a prism (convex); `None` for a box.
    polygon: Option<Vec<[f32; 2]>>,
}

impl Top {
    fn contains(&self, x: f32, z: f32) -> bool {
        if !((self.min[0]..=self.max[0]).contains(&x) && (self.min[1]..=self.max[1]).contains(&z)) {
            return false;
        }
        let Some(poly) = &self.polygon else {
            return true;
        };
        // Inside a convex polygon: on the same side of every edge.
        let (mut pos, mut neg) = (false, false);
        for (k, a) in poly.iter().enumerate() {
            let b = poly[(k + 1) % poly.len()];
            let cross = (b[0] - a[0]) * (z - a[1]) - (b[1] - a[1]) * (x - a[0]);
            pos |= cross > 1e-6;
            neg |= cross < -1e-6;
        }
        !(pos && neg)
    }
}

/// Side of a cell of the ground index, metres.
const INDEX_CELL_M: f32 = 8.0;

#[allow(clippy::cast_possible_truncation)]
fn index_cell(v: f32) -> i32 {
    (v / INDEX_CELL_M).floor() as i32
}

/// The grounds plants can stand on, with a coarse grid over x-z.
struct Ground<'a> {
    terrain: Option<&'a Terrain>,
    /// Ground of a terrain sample painted `auto`.
    auto: &'static str,
    tops: Vec<Top>,
    solids: Vec<Bounds>,
    /// Grid cell -> (tops, solids) overlapping it.
    index: HashMap<(i32, i32), (Vec<u32>, Vec<u32>)>,
    water: Vec<Bounds>,
    clear: Vec<[f32; 2]>,
}

impl<'a> Ground<'a> {
    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    fn new(map: &'a MapFile) -> Self {
        let mut tops = Vec::new();
        let mut solids = Vec::new();
        for b in &map.brushes {
            let pts = brush_hull_points(b);
            let Some(first) = pts.first().copied() else {
                continue;
            };
            let (min, max) = pts
                .iter()
                .fold((first, first), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
            solids.push(Bounds { min, max });
            match b {
                Brush::Box { material, .. } => tops.push(Top {
                    min: [min.x, min.z],
                    max: [max.x, max.z],
                    y: max.y,
                    material: material.clone(),
                    polygon: None,
                }),
                Brush::Prism {
                    points, material, ..
                } if points.len() >= 3 => tops.push(Top {
                    min: [min.x, min.z],
                    max: [max.x, max.z],
                    y: max.y,
                    material: material.clone(),
                    polygon: Some(
                        points
                            .iter()
                            .map(|p| [p[0] as f32 * GRID_UNIT_M, p[1] as f32 * GRID_UNIT_M])
                            .collect(),
                    ),
                }),
                _ => {}
            }
        }
        let mut index: HashMap<(i32, i32), (Vec<u32>, Vec<u32>)> = HashMap::new();
        let mut insert = |lo: [f32; 2], hi: [f32; 2], k: u32, top: bool| {
            for i in index_cell(lo[0])..=index_cell(hi[0]) {
                for j in index_cell(lo[1])..=index_cell(hi[1]) {
                    let e = index.entry((i, j)).or_default();
                    if top {
                        e.0.push(k);
                    } else {
                        e.1.push(k);
                    }
                }
            }
        };
        for (k, t) in tops.iter().enumerate() {
            insert(t.min, t.max, k as u32, true);
        }
        for (k, s) in solids.iter().enumerate() {
            insert([s.min.x, s.min.z], [s.max.x, s.max.z], k as u32, false);
        }
        let water = map
            .zones
            .iter()
            .filter_map(|z| {
                let Zone::Box { min, max, kind } = z;
                matches!(kind, ZoneKind::Water).then(|| Bounds {
                    min: crate::grid_to_metres(*min),
                    max: crate::grid_to_metres(*max),
                })
            })
            .collect();
        let clear = map
            .spawns
            .iter()
            .map(|s| [s.at[0], s.at[2]])
            .chain(map.pickups.iter().map(|p| [p.at[0], p.at[2]]))
            .collect();
        let auto = match map.theme.palette.as_str() {
            "desert" => "sand",
            "snow" => "snow",
            "industrial" | "urban" => "dirt",
            _ => "grass",
        };
        Self {
            terrain: map.terrain.as_ref(),
            auto,
            tops,
            solids,
            index,
            water,
            clear,
        }
    }

    /// Bounding rectangle `(min, max)` of every ground of `material`.
    fn extent(&self, material: &str) -> Option<([f32; 2], [f32; 2])> {
        let mut out: Option<([f32; 2], [f32; 2])> = None;
        let mut add = |lo: [f32; 2], hi: [f32; 2]| {
            out = Some(match out {
                None => (lo, hi),
                Some((a, b)) => (
                    [a[0].min(lo[0]), a[1].min(lo[1])],
                    [b[0].max(hi[0]), b[1].max(hi[1])],
                ),
            });
        };
        if let Some(t) = self.terrain {
            // Any ground name may match some terrain sample (paint, slope).
            let (lo, hi) = t.extent_xz();
            add(lo, hi);
        }
        if material.starts_with(TERRAIN_ONLY) {
            return out;
        }
        for t in self.tops.iter().filter(|t| t.material == material) {
            add(t.min, t.max);
        }
        out
    }

    /// Ground name of the terrain at `(x, z)` with its face normal.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    fn terrain_ground(&self, t: &Terrain, x: f32, z: f32) -> Option<(f32, Vec3, &'static str)> {
        let h = t.height_at(x, z)?;
        let n = t.face_normal_at(x, z)?;
        if n.y < STEEP_DEG.to_radians().cos() {
            return Some((h, n, "rock"));
        }
        let (lo, _) = t.extent_xz();
        let (nx, nz) = t.dims();
        let c = t.cell_m();
        let i = (((x - lo[0]) / c).round().max(0.0) as usize).min(nx - 1);
        let j = (((z - lo[1]) / c).round().max(0.0) as usize).min(nz - 1);
        let name = match t.paint_at(i, j) {
            0 => self.auto,
            p => PAINTS[usize::from(p).min(PAINTS.len() - 1)],
        };
        Some((h, n, name))
    }

    fn covered(&self, p: Vec3) -> bool {
        let probe = p + Vec3::Y * 0.05;
        self.index
            .get(&(index_cell(p.x), index_cell(p.z)))
            .is_some_and(|(_, solids)| {
                solids
                    .iter()
                    .any(|&k| self.solids[k as usize].contains(probe))
            })
    }

    /// The highest uncovered ground of `material` at `(x, z)`: height and
    /// normal.
    fn at(&self, x: f32, z: f32, material: &str) -> Option<(f32, Vec3)> {
        let mut best: Option<(f32, Vec3)> = None;
        let mut offer = |y: f32, n: Vec3| {
            if best.is_none_or(|(b, _)| y > b) && !self.covered(Vec3::new(x, y, z)) {
                best = Some((y, n));
            }
        };
        let (terrain_only, material) = match material.strip_prefix(TERRAIN_ONLY) {
            Some(name) => (true, name),
            None => (false, material),
        };
        if let Some(t) = self.terrain {
            if let Some((h, n, name)) = self.terrain_ground(t, x, z) {
                if name == material {
                    offer(h, n);
                }
            }
        }
        if terrain_only {
            return best;
        }
        if let Some((tops, _)) = self.index.get(&(index_cell(x), index_cell(z))) {
            for &k in tops {
                let t = &self.tops[k as usize];
                if t.material == material && t.contains(x, z) {
                    offer(t.y, Vec3::Y);
                }
            }
        }
        best
    }

    /// Whether a plant may stand at `p` (ground normal `n`).
    fn allowed(&self, p: Vec3, n: Vec3, species: Species) -> bool {
        let max_slope = species.info().max_slope_deg.to_radians().cos();
        n.y >= max_slope
            && !self
                .water
                .iter()
                .any(|w| w.contains(p + Vec3::Y * 0.05) || w.contains(p - Vec3::Y * 0.05))
            && !self
                .clear
                .iter()
                .any(|c| (c[0] - p.x).powi(2) + (c[1] - p.z).powi(2) < CLEARANCE_M * CLEARANCE_M)
    }
}

/// Every plant of a map: its density rules scattered from its seed, then its
/// hand-placed plants. Accepts a map file or its expansion
/// ([`crate::expand`]); both give the same plants.
#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_possible_wrap
)]
pub fn scatter(map: &MapFile) -> Vec<PlacedPlant> {
    let Some(veg) = &map.vegetation else {
        return Vec::new();
    };
    let symmetry = if map.symmetry == Symmetry::None {
        veg.symmetry
    } else {
        map.symmetry
    };
    let expanded = crate::expand(map);
    let ground = Ground::new(&expanded);
    let mut out = Vec::new();
    'rules: for (r, rule) in veg.rules.iter().enumerate() {
        if !(rule.density > 0.0 && rule.density <= MAX_DENSITY) {
            continue;
        }
        let Some((lo, hi)) = ground.extent(&rule.on) else {
            continue;
        };
        let info = rule.species.info();
        let cell = (100.0 / rule.density).sqrt();
        let (i0, i1) = ((lo[0] / cell).floor() as i64, (hi[0] / cell).ceil() as i64);
        let (j0, j1) = ((lo[1] / cell).floor() as i64, (hi[1] / cell).ceil() as i64);
        for j in j0..j1 {
            for i in i0..i1 {
                let h = |s| cell_hash(veg.seed, r, i, j, s);
                let x = (i as f32 + unit(h(0))) * cell;
                let z = (j as f32 + unit(h(1))) * cell;
                if !in_scattered_half(symmetry, x, z) {
                    continue;
                }
                let yaw = unit(h(2)) * std::f32::consts::TAU;
                let scale = info.scale.0 + (info.scale.1 - info.scale.0) * unit(h(3));
                let variant = (h(4) % u64::from(VARIANTS)) as u32;
                let Some((y, n)) = ground.at(x, z, &rule.on) else {
                    continue;
                };
                let p = Vec3::new(x, y, z);
                if !ground.allowed(p, n, rule.species) {
                    continue;
                }
                let plant = PlacedPlant {
                    species: rule.species,
                    position: p,
                    yaw,
                    scale,
                    variant,
                };
                if symmetry == Symmetry::None {
                    out.push(plant);
                } else {
                    let (tx, tz, tyaw) = mirror(symmetry, x, z, yaw);
                    let Some((ty, tn)) = ground.at(tx, tz, &rule.on) else {
                        continue;
                    };
                    let tp = Vec3::new(tx, ty, tz);
                    if !ground.allowed(tp, tn, rule.species) {
                        continue;
                    }
                    out.push(plant);
                    out.push(PlacedPlant {
                        position: tp,
                        yaw: tyaw,
                        ..plant
                    });
                }
                if out.len() >= MAX_PLANTS {
                    break 'rules;
                }
            }
        }
    }
    let placed = expanded
        .vegetation
        .as_ref()
        .map_or(&[][..], |v| &v.plants[..]);
    for (k, p) in placed.iter().enumerate() {
        if out.len() >= MAX_PLANTS {
            break;
        }
        out.push(PlacedPlant {
            species: p.species,
            position: Vec3::from_array(p.at),
            yaw: p.yaw.to_radians(),
            scale: p.scale,
            variant: (splitmix(k as u64 ^ veg.seed) % u64::from(VARIANTS)) as u32,
        });
    }
    out.truncate(MAX_PLANTS);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{Meta, Spawn, Team, Theme};

    fn map() -> MapFile {
        MapFile {
            format_version: crate::FORMAT_VERSION,
            meta: Meta {
                name: "veg".into(),
                author: String::new(),
                description: String::new(),
                modes: vec!["fps".into()],
                players: (2, 8),
            },
            theme: Theme {
                ambience: "day".into(),
                sky: String::new(),
                weather: Vec::new(),
                palette: "nature".into(),
            },
            physics: None,
            symmetry: Symmetry::None,
            prefabs: Vec::new(),
            brushes: vec![Brush::Box {
                min: [-40, -2, -40],
                max: [40, 0, 40],
                material: "grass".into(),
            }],
            props: Vec::new(),
            spawns: Vec::new(),
            pickups: Vec::new(),
            zones: Vec::new(),
            lights: Vec::new(),
            terrain: None,
            vegetation: Some(Vegetation {
                seed: 7,
                rules: vec![DensityRule {
                    on: "grass".into(),
                    species: Species::Grass,
                    density: 50.0,
                }],
                plants: Vec::new(),
                symmetry: Symmetry::None,
            }),
        }
    }

    #[test]
    fn scatter_is_deterministic_and_follows_the_density() {
        let m = map();
        let a = scatter(&m);
        assert_eq!(a, scatter(&m));
        // 40 m x 40 m of grass at 50 per 100 m^2: about 800 plants.
        assert!((700..=900).contains(&a.len()), "{} plants", a.len());
        for p in &a {
            assert!(p.position.y.abs() < 1e-6);
            assert!(p.position.x.abs() <= 20.0 && p.position.z.abs() <= 20.0);
            assert!(p.variant < VARIANTS);
        }
        let mut other = m.clone();
        other.vegetation.as_mut().unwrap().seed = 8;
        assert_ne!(scatter(&other), a);
    }

    #[test]
    fn covered_and_foreign_grounds_get_nothing() {
        let mut m = map();
        // A slab on the grass covers its middle.
        m.brushes.push(Brush::Box {
            min: [-10, 0, -10],
            max: [10, 2, 10],
            material: "stone".into(),
        });
        m.spawns.push(Spawn {
            at: [15.0, 0.0, 15.0],
            yaw: 0.0,
            team: Team::A,
        });
        for p in scatter(&m) {
            let (x, z) = (p.position.x, p.position.z);
            assert!(
                !(x > -5.0 && x < 5.0 && z > -5.0 && z < 5.0),
                "under the slab"
            );
            assert!((x - 15.0).hypot(z - 15.0) >= CLEARANCE_M);
        }
        m.vegetation.as_mut().unwrap().rules[0].on = "sand".into();
        assert!(scatter(&m).is_empty());
    }

    #[test]
    fn symmetric_maps_get_mirrored_plants() {
        let mut m = map();
        m.symmetry = Symmetry::MirrorX;
        let plants = scatter(&m);
        assert!(!plants.is_empty());
        for pair in plants.chunks(2) {
            let (a, b) = (pair[0].position, pair[1].position);
            assert!((a.x + b.x).abs() < 1e-5 && (a.z - b.z).abs() < 1e-5);
        }
        // The expansion scatters the same plants.
        assert_eq!(scatter(&crate::expand(&m)), plants);
    }

    #[test]
    fn prism_tops_are_grounds_inside_their_footprint() {
        let mut m = map();
        // A raised grass triangle (x + z <= 10 m in the first quadrant) on
        // stone.
        m.brushes = vec![
            Brush::Box {
                min: [-40, -2, -40],
                max: [40, 0, 40],
                material: "stone".into(),
            },
            Brush::Prism {
                points: vec![[0, 0], [20, 0], [0, 20]],
                y_min: 0,
                y_max: 1,
                material: "grass".into(),
            },
        ];
        m.vegetation.as_mut().unwrap().rules[0].density = 400.0;
        let plants = scatter(&m);
        assert!(plants.len() > 20, "{} plants", plants.len());
        for p in &plants {
            let (x, z) = (p.position.x, p.position.z);
            assert!(x >= 0.0 && z >= 0.0 && x + z <= 10.0 + 1e-4, "{x} {z}");
            assert!((p.position.y - 0.5).abs() < 1e-6);
        }
    }

    #[test]
    fn hand_placed_plants_come_last() {
        let mut m = map();
        let v = m.vegetation.as_mut().unwrap();
        v.rules.clear();
        v.plants.push(Plant {
            species: Species::Oak,
            at: [1.0, 0.0, 2.0],
            yaw: 90.0,
            scale: 1.5,
        });
        let p = scatter(&m);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].species, Species::Oak);
        assert!((p[0].yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        let sight: Vec<_> = p[0].sight().collect();
        assert_eq!(sight.len(), 2);
        assert!((sight[1].2 - 1.35 * 1.5).abs() < 1e-5);
    }

    #[test]
    fn terrain_grounds_follow_paint_and_slope() {
        let mut m = map();
        m.brushes.clear();
        let mut t = Terrain::flat([-20, -20], 4, [11, 11], 0);
        // A steep ridge along x = 0 (2.5 m over 2 m: 51 degrees).
        for j in 0..11 {
            let k = t.index(5, j);
            t.heights[k] = 250;
        }
        m.terrain = Some(t);
        let plants = scatter(&m);
        assert!(!plants.is_empty());
        for p in &plants {
            // Not on the ridge's steep flanks.
            assert!(p.position.x.abs() >= 2.0 - 1e-3, "{:?}", p.position);
            let h = m
                .terrain
                .as_ref()
                .unwrap()
                .height_at(p.position.x, p.position.z);
            assert!((h.unwrap() - p.position.y).abs() < 1e-4);
        }
        m.vegetation.as_mut().unwrap().rules[0].on = "rock".into();
        m.vegetation.as_mut().unwrap().rules[0].species = Species::Rock;
        let rocks = scatter(&m);
        assert!(!rocks.is_empty());
        assert!(rocks.iter().all(|p| p.position.x.abs() < 2.0));

        // `terrain:grass` leaves a grass slab standing on the terrain bare.
        let rule = &mut m.vegetation.as_mut().unwrap().rules[0];
        rule.species = Species::Grass;
        rule.on = "grass".into();
        m.brushes.push(Brush::Box {
            min: [-36, 0, -36],
            max: [-20, 4, -20],
            material: "grass".into(),
        });
        let on_slab = |plants: &[PlacedPlant]| {
            plants
                .iter()
                .filter(|p| (p.position.y - 2.0).abs() < 1e-6)
                .count()
        };
        assert!(on_slab(&scatter(&m)) > 0);
        m.vegetation.as_mut().unwrap().rules[0].on = "terrain:grass".into();
        let terrain_only = scatter(&m);
        assert!(!terrain_only.is_empty());
        assert_eq!(on_slab(&terrain_only), 0);
    }

    #[test]
    fn validation_rejects_bad_densities() {
        let mut v = map().vegetation.unwrap();
        assert!(v.validate().is_ok());
        v.rules[0].density = MAX_DENSITY + 1.0;
        assert!(v.validate().is_err());
        v.rules[0].density = f32::NAN;
        assert!(v.validate().is_err());
    }
}

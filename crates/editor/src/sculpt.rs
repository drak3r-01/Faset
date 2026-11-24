//! Terrain sculpting (lot 6.10): raise, lower, smooth, flatten and paint
//! the map's height field with a round brush, as undoable commands.
//!
//! A stroke (button down, drag, release) edits the document live and is
//! recorded as one [`TerrainEdit`] on release, so undo restores the terrain
//! of before the stroke exactly.
//!
//! Decisions of lot 6.10:
//! - brush falloff `(1 - (d / r)^2)^2`, `d` the horizontal distance to the
//!   centre and `r` the radius;
//! - each dab moves a sample by at most `strength` metres (raise, lower),
//!   or by `strength` times the falloff toward its target (smooth: the mean
//!   of its 3 x 3 neighbourhood; flatten: the height under the first dab of
//!   the stroke), strength clamped to 0..=1 for these two;
//! - paint sets the paint value of every sample within the radius;
//! - a new terrain covers the brushes' footprint (64 x 64 m around the
//!   origin for an empty map) at height 0 with the default 2 m cell.

use faset_core::glam::Vec3;
use faset_mapfmt::terrain::{DEFAULT_CELL, PAINTS};
use faset_mapfmt::{MapFile, Terrain, GRID_UNIT_M};

use crate::command::Command;
use crate::element::brush_aabb;
use crate::math::{Aabb, Ray};

/// What a sculpt dab does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SculptOp {
    #[default]
    Raise,
    Lower,
    /// Moves samples toward the mean of their neighbours.
    Smooth,
    /// Moves samples toward the height under the first dab of the stroke.
    Flatten,
    /// Sets the paint value (index of `faset_mapfmt::terrain::PAINTS`).
    Paint(u8),
}

impl SculptOp {
    /// The operations shown in the panel (paint takes each ground).
    #[must_use]
    #[allow(clippy::cast_possible_truncation)]
    pub fn all() -> Vec<Self> {
        let mut v = vec![Self::Raise, Self::Lower, Self::Smooth, Self::Flatten];
        v.extend((0..PAINTS.len() as u8).map(Self::Paint));
        v
    }

    /// Label for the panel.
    #[must_use]
    pub fn label(self) -> String {
        match self {
            Self::Raise => "raise".to_owned(),
            Self::Lower => "lower".to_owned(),
            Self::Smooth => "smooth".to_owned(),
            Self::Flatten => "flatten".to_owned(),
            Self::Paint(p) => format!(
                "paint {}",
                PAINTS.get(usize::from(p)).copied().unwrap_or("?")
            ),
        }
    }
}

/// Parameters of the sculpt tool.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SculptSettings {
    pub op: SculptOp,
    /// Brush radius, metres.
    pub radius_m: f32,
    /// Raise and lower: metres per dab at the centre; smooth and flatten:
    /// fraction of the way to the target per dab (0..=1).
    pub strength: f32,
}

impl Default for SculptSettings {
    fn default() -> Self {
        Self {
            op: SculptOp::Raise,
            radius_m: 4.0,
            strength: 0.25,
        }
    }
}

/// Brush weight of a sample at distance `d` from the centre.
fn falloff(d: f32, r: f32) -> f32 {
    if r <= 0.0 || d >= r {
        return 0.0;
    }
    let k = 1.0 - (d / r) * (d / r);
    k * k
}

#[allow(clippy::cast_possible_truncation)]
fn to_cm(h: f32) -> i16 {
    (h * 100.0)
        .round()
        .clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16
}

/// One dab of `settings` centred on `(x, z)`. `flatten_to` is the flatten
/// target in metres. Returns whether a sample changed.
pub fn dab(t: &mut Terrain, x: f32, z: f32, settings: &SculptSettings, flatten_to: f32) -> bool {
    let r = settings.radius_m;
    let before_h = t.heights.clone();
    let before_p = t.paint.clone();
    let samples = t.samples_within(x, z, r);
    for &(i, j, d) in &samples {
        let w = falloff(d, r);
        let k = t.index(i, j);
        let h = t.height_m(i, j);
        match settings.op {
            SculptOp::Raise => t.heights[k] = to_cm(h + settings.strength * w),
            SculptOp::Lower => t.heights[k] = to_cm(h - settings.strength * w),
            SculptOp::Smooth => {
                let (nx, nz) = t.dims();
                let mut sum = 0.0;
                let mut n = 0.0;
                for jj in j.saturating_sub(1)..=(j + 1).min(nz - 1) {
                    for ii in i.saturating_sub(1)..=(i + 1).min(nx - 1) {
                        sum += f32::from(before_h[t.index(ii, jj)]) * 0.01;
                        n += 1.0;
                    }
                }
                let a = settings.strength.clamp(0.0, 1.0) * w;
                t.heights[k] = to_cm(h + (sum / n - h) * a);
            }
            SculptOp::Flatten => {
                let a = settings.strength.clamp(0.0, 1.0) * w;
                t.heights[k] = to_cm(h + (flatten_to - h) * a);
            }
            SculptOp::Paint(p) => {
                if t.paint.is_empty() {
                    t.paint = vec![0; t.heights.len()];
                }
                t.paint[k] = p;
            }
        }
    }
    if t.paint.iter().all(|p| *p == 0) {
        t.paint.clear();
    }
    t.heights != before_h || t.paint != before_p
}

/// First point where `ray` meets the terrain surface, marching in quarter
/// cells up to 1 km then refining by bisection.
#[must_use]
pub fn ray_terrain(t: &Terrain, ray: &Ray) -> Option<Vec3> {
    let step = t.cell_m() * 0.25;
    let above = |p: Vec3| t.height_at(p.x, p.z).map(|h| p.y >= h);
    let mut prev = 0.0f32;
    let mut s = step;
    while s < 1000.0 {
        if above(ray.at(s)) == Some(false) && above(ray.at(prev)) != Some(false) {
            let (mut lo, mut hi) = (prev, s);
            for _ in 0..24 {
                let mid = 0.5 * (lo + hi);
                if above(ray.at(mid)) == Some(false) {
                    hi = mid;
                } else {
                    lo = mid;
                }
            }
            let p = ray.at(hi);
            return Some(Vec3::new(p.x, t.height_at(p.x, p.z)?, p.z));
        }
        prev = s;
        s += step;
    }
    None
}

/// A flat terrain at height 0 covering the footprint of the map's brushes
/// (64 x 64 m around the origin for a map without brushes).
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn terrain_for(map: &MapFile) -> Terrain {
    let mut bounds: Option<Aabb> = None;
    for b in &map.brushes {
        let a = brush_aabb(b);
        bounds = Some(bounds.map_or(a, |o| o.union(&a)));
    }
    let a = bounds.unwrap_or(Aabb {
        min: Vec3::new(-32.0, 0.0, -32.0),
        max: Vec3::new(32.0, 0.0, 32.0),
    });
    let cell = f32::from(DEFAULT_CELL) * GRID_UNIT_M;
    let lo = [(a.min.x / cell).floor(), (a.min.z / cell).floor()];
    let hi = [(a.max.x / cell).ceil(), (a.max.z / cell).ceil()];
    let size = [0, 1].map(|k| ((hi[k] - lo[k]) as u16 + 1).clamp(2, 1025));
    let origin = lo.map(|v| v as i32 * i32::from(DEFAULT_CELL));
    Terrain::flat(origin, DEFAULT_CELL, size, 0)
}

/// Region of the world a terrain occupies (its whole height range).
fn terrain_box(t: &Terrain) -> Aabb {
    let (lo, hi) = t.extent_xz();
    let (_, top) = t.height_range();
    Aabb {
        min: Vec3::new(lo[0], t.base_y(), lo[1]),
        max: Vec3::new(hi[0], top, hi[1]),
    }
}

/// Region where two terrains differ, grown by one cell (the triangles and
/// normals around a changed sample change too). A different base or grid
/// changes every border skirt: the whole of both terrains then.
fn changed_region(a: &Terrain, b: &Terrain) -> Vec<Aabb> {
    let same_grid = a.origin == b.origin && a.cell == b.cell && a.size == b.size;
    if !same_grid || a.base_y() != b.base_y() {
        return vec![terrain_box(a), terrain_box(b)];
    }
    let (nx, nz) = a.dims();
    let mut out: Option<Aabb> = None;
    for j in 0..nz {
        for i in 0..nx {
            let k = a.index(i, j);
            if a.heights[k] == b.heights[k] && a.paint_at(i, j) == b.paint_at(i, j) {
                continue;
            }
            let (pa, pb) = (a.sample_pos(i, j), b.sample_pos(i, j));
            let r = Aabb::from_corners(pa, pb);
            out = Some(out.map_or(r, |o| o.union(&r)));
        }
    }
    let Some(r) = out else { return Vec::new() };
    // Neighbouring triangles reach one cell around, up to their own heights.
    let top = a.height_range().1.max(b.height_range().1);
    let c = a.cell_m();
    vec![Aabb {
        min: Vec3::new(r.min.x - c, a.base_y().min(b.base_y()), r.min.z - c),
        max: Vec3::new(r.max.x + c, top, r.max.z + c),
    }]
}

/// Replaces the map's terrain (add, remove, a sculpt stroke).
pub struct TerrainEdit {
    name: String,
    old: Option<Terrain>,
    new: Option<Terrain>,
}

impl TerrainEdit {
    #[must_use]
    pub fn new(name: &str, old: Option<Terrain>, new: Option<Terrain>) -> Self {
        Self {
            name: name.to_owned(),
            old,
            new,
        }
    }
}

impl Command for TerrainEdit {
    fn name(&self) -> &str {
        &self.name
    }
    fn apply(&mut self, map: &mut MapFile) {
        map.terrain.clone_from(&self.new);
    }
    fn revert(&mut self, map: &mut MapFile) {
        map.terrain.clone_from(&self.old);
    }
    fn touched(&self) -> Vec<Aabb> {
        match (&self.old, &self.new) {
            (Some(a), Some(b)) => changed_region(a, b),
            (Some(t), None) | (None, Some(t)) => vec![terrain_box(t)],
            (None, None) => Vec::new(),
        }
    }
}

/// A stroke in progress: the terrain before it and the flatten target.
#[derive(Debug, Clone)]
pub struct Stroke {
    pub(crate) before: Terrain,
    pub(crate) flatten_to: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn falloff_is_one_at_the_centre_and_zero_at_the_rim() {
        assert_eq!(falloff(0.0, 4.0), 1.0);
        assert_eq!(falloff(4.0, 4.0), 0.0);
        assert!(falloff(2.0, 4.0) > 0.5 && falloff(2.0, 4.0) < 0.6);
    }

    #[test]
    fn raise_smooth_flatten_paint() {
        let mut t = Terrain::flat([-20, -20], 4, [11, 11], 0);
        let s = SculptSettings {
            op: SculptOp::Raise,
            radius_m: 5.0,
            strength: 1.0,
        };
        assert!(dab(&mut t, 0.0, 0.0, &s, 0.0));
        assert_eq!(t.height_at(0.0, 0.0), Some(1.0));
        assert!(t.height_at(4.0, 0.0).unwrap() > 0.0);
        assert_eq!(t.height_at(-8.0, 0.0), Some(0.0));
        let peak = t.height_at(0.0, 0.0).unwrap();
        let smooth = SculptSettings {
            op: SculptOp::Smooth,
            ..s
        };
        dab(&mut t, 0.0, 0.0, &smooth, 0.0);
        assert!(t.height_at(0.0, 0.0).unwrap() < peak);
        let flat = SculptSettings {
            op: SculptOp::Flatten,
            ..s
        };
        dab(&mut t, 0.0, 0.0, &flat, 0.25);
        assert_eq!(t.height_at(0.0, 0.0), Some(0.25));
        let paint = SculptSettings {
            op: SculptOp::Paint(3),
            ..s
        };
        dab(&mut t, 0.0, 0.0, &paint, 0.0);
        assert_eq!(t.paint_at(5, 5), 3);
        assert_eq!(t.paint_at(0, 0), 0);
        // Painting everything back to auto empties the layer.
        let auto = SculptSettings {
            op: SculptOp::Paint(0),
            ..s
        };
        dab(&mut t, 0.0, 0.0, &auto, 0.0);
        assert!(t.paint.is_empty());
    }

    #[test]
    fn ray_meets_the_surface() {
        let mut t = Terrain::flat([-20, -20], 4, [11, 11], 150);
        let k = t.index(5, 5);
        t.heights[k] = 300;
        let ray = Ray::new(Vec3::new(0.0, 20.0, 15.0), Vec3::new(0.0, -1.0, -0.75));
        let p = ray_terrain(&t, &ray).unwrap();
        assert!((p.y - t.height_at(p.x, p.z).unwrap()).abs() < 1e-4);
        assert!(ray_terrain(&t, &Ray::new(Vec3::new(0.0, 20.0, 0.0), Vec3::Y)).is_none());
    }
}

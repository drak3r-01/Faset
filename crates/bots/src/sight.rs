//! Vegetation that blocks sight (lot 6.11).
//!
//! Plants have no collision, so the static world's raycast goes through
//! them; their crowns, bushes and rocks still hide a player. [`SightBlockers`]
//! holds the sight cylinders of every plant of a map
//! ([`faset_mapfmt::vegetation::scatter`], the same plants the client draws)
//! in a uniform grid, and [`SightBlockers::blocks`] tells whether a line of
//! sight crosses one of them.
//!
//! Decision: a line of sight is blocked when it runs through a blocker over
//! at least [`MIN_CHORD_M`] (a ray grazing the edge of a crown still sees);
//! a bot standing inside a bush sees no more out of it than in.

use std::collections::HashMap;

use faset_core::glam::Vec3;
use faset_mapfmt::MapFile;

/// Shortest stretch of a blocker that hides what is behind it, metres.
pub const MIN_CHORD_M: f32 = 0.3;

/// Side of a grid cell, metres.
const CELL_M: f32 = 4.0;

/// A vertical cylinder that blocks sight, world space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Blocker {
    pub x: f32,
    pub z: f32,
    pub radius: f32,
    pub bottom: f32,
    pub top: f32,
}

impl Blocker {
    /// Parameter interval `[t0, t1]` of the segment `from + t (to - from)`,
    /// `t` in `[0, 1]`, inside the cylinder.
    fn interval(&self, from: Vec3, to: Vec3) -> Option<(f32, f32)> {
        let d = to - from;
        let (ox, oz) = (from.x - self.x, from.z - self.z);
        let a = d.x * d.x + d.z * d.z;
        let b = 2.0 * (ox * d.x + oz * d.z);
        let c = ox * ox + oz * oz - self.radius * self.radius;
        let (mut t0, mut t1) = if a < 1e-12 {
            // Vertical segment: inside or outside for its whole length.
            if c > 0.0 {
                return None;
            }
            (0.0, 1.0)
        } else {
            let disc = b * b - 4.0 * a * c;
            if disc < 0.0 {
                return None;
            }
            let s = disc.sqrt();
            ((-b - s) / (2.0 * a), (-b + s) / (2.0 * a))
        };
        // Clip by the height range.
        if d.y.abs() < 1e-9 {
            if from.y < self.bottom || from.y > self.top {
                return None;
            }
        } else {
            let (ya, yb) = ((self.bottom - from.y) / d.y, (self.top - from.y) / d.y);
            t0 = t0.max(ya.min(yb));
            t1 = t1.min(ya.max(yb));
        }
        let (t0, t1) = (t0.max(0.0), t1.min(1.0));
        (t1 > t0).then_some((t0, t1))
    }
}

/// The sight blockers of a map in a uniform x-z grid.
#[derive(Debug, Clone, Default)]
pub struct SightBlockers {
    blockers: Vec<Blocker>,
    cells: HashMap<(i32, i32), Vec<u32>>,
}

#[allow(clippy::cast_possible_truncation)]
fn cell(v: f32) -> i32 {
    (v / CELL_M).floor() as i32
}

impl SightBlockers {
    /// A grid over `blockers`.
    #[must_use]
    #[allow(clippy::cast_possible_truncation)]
    pub fn new(blockers: Vec<Blocker>) -> Self {
        let mut cells: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
        for (k, b) in blockers.iter().enumerate() {
            for i in cell(b.x - b.radius)..=cell(b.x + b.radius) {
                for j in cell(b.z - b.radius)..=cell(b.z + b.radius) {
                    cells.entry((i, j)).or_default().push(k as u32);
                }
            }
        }
        Self { blockers, cells }
    }

    /// The blockers of every plant of `map` (a map file or its expansion).
    #[must_use]
    pub fn from_map(map: &MapFile) -> Self {
        let blockers = faset_mapfmt::vegetation::scatter(map)
            .iter()
            .flat_map(|p| {
                p.sight()
                    .map(|(x, z, radius, bottom, top)| Blocker {
                        x,
                        z,
                        radius,
                        bottom,
                        top,
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        Self::new(blockers)
    }

    /// Number of blockers.
    #[must_use]
    pub fn len(&self) -> usize {
        self.blockers.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.blockers.is_empty()
    }

    /// Whether the line of sight from `from` to `to` runs through a blocker
    /// over at least [`MIN_CHORD_M`].
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn blocks(&self, from: Vec3, to: Vec3) -> bool {
        if self.blockers.is_empty() {
            return false;
        }
        let length = from.distance(to);
        let min_t = if length > 0.0 {
            MIN_CHORD_M / length
        } else {
            return false;
        };
        let hit = |k: u32| {
            self.blockers[k as usize]
                .interval(from, to)
                .is_some_and(|(t0, t1)| t1 - t0 >= min_t)
        };
        // Grid traversal of the x-z projection (Amanatides and Woo).
        let (dx, dz) = (to.x - from.x, to.z - from.z);
        let (mut i, mut j) = (cell(from.x), cell(from.z));
        let end = (cell(to.x), cell(to.z));
        let axis = |p: f32, d: f32, c: i32| -> (i32, f32, f32) {
            if d > 0.0 {
                (1, ((c + 1) as f32 * CELL_M - p) / d, CELL_M / d)
            } else if d < 0.0 {
                (-1, (c as f32 * CELL_M - p) / d, -CELL_M / d)
            } else {
                (0, f32::INFINITY, f32::INFINITY)
            }
        };
        let (si, mut ti, di) = axis(from.x, dx, i);
        let (sj, mut tj, dj) = axis(from.z, dz, j);
        let steps = (end.0 - i).unsigned_abs() + (end.1 - j).unsigned_abs() + 1;
        for _ in 0..=steps {
            if self
                .cells
                .get(&(i, j))
                .is_some_and(|list| list.iter().any(|&k| hit(k)))
            {
                return true;
            }
            if (i, j) == end {
                break;
            }
            if ti < tj {
                i += si;
                ti += di;
            } else {
                j += sj;
                tj += dj;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bush(x: f32, z: f32) -> Blocker {
        Blocker {
            x,
            z,
            radius: 0.75,
            bottom: 0.0,
            top: 1.2,
        }
    }

    #[test]
    fn a_bush_hides_low_lines_but_not_high_ones() {
        let s = SightBlockers::new(vec![bush(0.0, -10.0)]);
        let eye = Vec3::new(0.0, 0.8, 0.0);
        assert!(s.blocks(eye, Vec3::new(0.0, 0.8, -20.0)));
        // Over the bush.
        assert!(!s.blocks(Vec3::new(0.0, 1.6, 0.0), Vec3::new(0.0, 1.6, -20.0)));
        // Beside it.
        assert!(!s.blocks(eye, Vec3::new(3.0, 0.8, -20.0)));
        // A ray grazing its edge sees.
        assert!(!s.blocks(Vec3::new(0.74, 0.8, 0.0), Vec3::new(0.74, 0.8, -20.0)));
        // Stopping short of it.
        assert!(!s.blocks(eye, Vec3::new(0.0, 0.8, -8.0)));
    }

    #[test]
    fn traversal_finds_blockers_in_every_direction() {
        let s = SightBlockers::new(vec![bush(17.3, -9.1)]);
        let target = Vec3::new(17.3, 0.6, -9.1);
        for k in 0..16 {
            let a = k as f32 * std::f32::consts::TAU / 16.0;
            let from = target + Vec3::new(a.cos(), 0.0, a.sin()) * 30.0;
            let to = target - Vec3::new(a.cos(), 0.0, a.sin()) * 30.0;
            assert!(s.blocks(from, to), "direction {k}");
            assert!(!s.blocks(from + Vec3::Y * 3.0, to + Vec3::Y * 3.0));
        }
    }
}

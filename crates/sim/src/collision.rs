//! Static collision world and the three base queries: ray cast, shape cast
//! (sweep) and intersection test, built on `parry3d`.
//!
//! Static volumes are indexed by a parry [`Bvh`] built once when the world is
//! created. Every query runs in two phases:
//!
//! 1. Broad phase: the BVH returns the volumes whose AABB can be touched by
//!    the query (ray, swept capsule bounds, capsule bounds).
//! 2. Narrow phase: the candidates are sorted by volume index and tested
//!    exactly, in that order, with strict `<` comparisons.
//!
//! Decision: sorting the candidates makes the result independent of the BVH
//! traversal order, so ties are always resolved toward the lowest volume
//! index, exactly as the former linear scan did. Results are therefore
//! bit-identical to a brute-force scan (checked by a property test). The BVH
//! is built with the PLOC strategy (best tree quality for static scenes); its
//! construction is sequential and uses no hash maps, so it is deterministic.

use faset_core::glam::Vec3;
use parry3d::bounding_volume::{Aabb, BoundingVolume};
use parry3d::math::{Pose, Vector};
use parry3d::partitioning::{Bvh, BvhBuildStrategy};
use parry3d::query::{self, Ray, ShapeCastOptions};
use parry3d::shape::{Capsule, Shape, SharedShape};

use crate::components::{CAPSULE_RADIUS, STANDING_HEIGHT};

/// Margin added to query bounds in the broad phase, in metres. It only makes
/// the broad phase more conservative; exact tests decide the result.
const BROAD_PHASE_MARGIN: f32 = 1e-3;

/// One convex static volume (box, ramp, cylinder, prism, ...).
#[derive(Clone)]
pub struct Volume {
    pub shape: SharedShape,
    pub pose: Pose,
}

impl Volume {
    /// Axis-aligned box from its centre and half extents.
    #[must_use]
    pub fn cuboid(center: Vec3, half: Vec3) -> Self {
        Self {
            shape: SharedShape::cuboid(half.x, half.y, half.z),
            pose: Pose::translation(center.x, center.y, center.z),
        }
    }

    fn aabb(&self) -> Aabb {
        self.shape.compute_aabb(&self.pose)
    }
}

/// Result of a sweep: fraction of the motion travelled and the surface normal.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SweepHit {
    /// In [0, 1]: fraction of the requested motion that is free.
    pub toi: f32,
    pub normal: Vec3,
    pub volume: usize,
}

/// Closest ray hit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RayHit {
    pub distance: f32,
    pub normal: Vec3,
    pub volume: usize,
}

/// Immutable set of static volumes with a BVH over their bounds.
#[derive(Clone)]
pub struct StaticWorld {
    volumes: Vec<Volume>,
    bvh: Bvh,
}

impl Default for StaticWorld {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

fn v3(v: Vec3) -> Vector {
    Vector::new(v.x, v.y, v.z)
}

fn to_glam(v: Vector) -> Vec3 {
    Vec3::new(v.x, v.y, v.z)
}

fn translation(p: Vec3) -> Pose {
    Pose::translation(p.x, p.y, p.z)
}

/// Exact overlap test between one volume and a capsule.
///
/// Decision: uses `query::distance` instead of `query::intersection_test`.
/// In parry 0.31 the boolean GJK test reports false positives for a capsule
/// resting about 1 cm above a box face (seen in `sweep_stops_at_wall`), while
/// the distance query is exact there. Touching (distance 0) counts as overlap.
fn overlaps(v: &Volume, pose: &Pose, cap: &Capsule) -> bool {
    query::distance(&v.pose, v.shape.as_ref(), pose, cap).is_ok_and(|d| d.distance <= 0.0)
}

/// Vertical capsule of the given total height (radius 0.35 m).
#[must_use]
pub fn player_capsule(height: f32) -> Capsule {
    let half = (height * 0.5 - CAPSULE_RADIUS).max(0.0);
    Capsule::new_y(half, CAPSULE_RADIUS)
}

impl StaticWorld {
    #[must_use]
    pub fn new(volumes: Vec<Volume>) -> Self {
        let bvh = if volumes.is_empty() {
            Bvh::new()
        } else {
            let leaves: Vec<Aabb> = volumes.iter().map(Volume::aabb).collect();
            Bvh::from_leaves(BvhBuildStrategy::Ploc, &leaves)
        };
        Self { volumes, bvh }
    }

    #[must_use]
    pub fn volumes(&self) -> &[Volume] {
        &self.volumes
    }

    /// Test arena: 100 m floor with top at y = 0 and a wall at x = 10.
    #[must_use]
    pub fn test_arena() -> Self {
        Self::new(vec![
            Volume::cuboid(Vec3::new(0.0, -0.5, 0.0), Vec3::new(50.0, 0.5, 50.0)),
            Volume::cuboid(Vec3::new(10.5, 2.0, 0.0), Vec3::new(0.5, 2.0, 50.0)),
        ])
    }

    /// Volume indices whose AABB overlaps `aabb`, in ascending order.
    fn candidates_in(&self, aabb: &Aabb) -> Vec<u32> {
        let mut out: Vec<u32> = self.bvh.intersect_aabb(aabb).collect();
        out.sort_unstable();
        out
    }

    /// Closest hit of a ray; `dir` must be normalised.
    #[must_use]
    pub fn ray_cast(&self, origin: Vec3, dir: Vec3, max_dist: f32) -> Option<RayHit> {
        let ray = Ray::new(v3(origin), v3(dir));
        let mut candidates: Vec<u32> = self
            .bvh
            .leaves(|node| node.cast_ray(&ray, max_dist) <= max_dist)
            .collect();
        candidates.sort_unstable();
        let mut best: Option<RayHit> = None;
        for i in candidates {
            let vol = &self.volumes[i as usize];
            if let Some(h) = vol
                .shape
                .cast_ray_and_get_normal(&vol.pose, &ray, max_dist, true)
            {
                if best.is_none_or(|b| h.time_of_impact < b.distance) {
                    best = Some(RayHit {
                        distance: h.time_of_impact,
                        normal: to_glam(h.normal),
                        volume: i as usize,
                    });
                }
            }
        }
        best
    }

    /// Sweeps a vertical capsule from `from` (centre) by `delta`. Returns the
    /// earliest blocking hit, or `None` if the whole motion is free.
    #[must_use]
    pub fn shape_cast(&self, from: Vec3, delta: Vec3, height: f32) -> Option<SweepHit> {
        let cap = player_capsule(height);
        let pose = translation(from);
        let vel = v3(delta);
        // The swept capsule is contained in the union of its start and end bounds.
        let swept = cap
            .compute_aabb(&pose)
            .merged(&cap.compute_aabb(&translation(from + delta)))
            .loosened(BROAD_PHASE_MARGIN);
        let opts = ShapeCastOptions {
            max_time_of_impact: 1.0,
            target_distance: 0.0,
            stop_at_penetration: true,
            compute_impact_geometry_on_penetration: true,
        };
        let mut best: Option<SweepHit> = None;
        for i in self.candidates_in(&swept) {
            let vol = &self.volumes[i as usize];
            let hit = query::cast_shapes(
                &vol.pose,
                Vector::ZERO,
                vol.shape.as_ref(),
                &pose,
                vel,
                &cap,
                opts,
            );
            if let Ok(Some(h)) = hit {
                let normal = to_glam(vol.pose.rotation * h.normal1);
                // Ignore surfaces the capsule is moving away from or along.
                if normal.dot(delta) >= 0.0 {
                    continue;
                }
                if best.is_none_or(|b| h.time_of_impact < b.toi) {
                    best = Some(SweepHit {
                        toi: h.time_of_impact.clamp(0.0, 1.0),
                        normal,
                        volume: i as usize,
                    });
                }
            }
        }
        best
    }

    /// Whether a vertical capsule at `center` overlaps any volume.
    #[must_use]
    pub fn intersects(&self, center: Vec3, height: f32) -> bool {
        let cap = player_capsule(height);
        let pose = translation(center);
        let bounds = cap.compute_aabb(&pose).loosened(BROAD_PHASE_MARGIN);
        // A boolean "any" does not depend on visiting order: no sort needed.
        let hit = self.bvh.intersect_aabb(&bounds).any(|i| {
            let v = &self.volumes[i as usize];
            overlaps(v, &pose, &cap)
        });
        hit
    }

    /// Moves a capsule by `delta`, stopping at the first blocker.
    #[must_use]
    pub fn move_capsule(&self, from: Vec3, delta: Vec3, height: f32) -> Vec3 {
        match self.shape_cast(from, delta, height) {
            Some(h) => from + delta * h.toi,
            None => from + delta,
        }
    }
}

/// Default standing height, re-exported for callers.
pub const STANDING: f32 = STANDING_HEIGHT;

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn ray_hits_wall() {
        let w = StaticWorld::test_arena();
        let h = w
            .ray_cast(Vec3::new(0.0, 1.0, 0.0), Vec3::X, 100.0)
            .unwrap();
        assert!((h.distance - 10.0).abs() < 1e-3);
        assert!((h.normal - Vec3::NEG_X).length() < 1e-3);
        assert!(w
            .ray_cast(Vec3::new(0.0, 1.0, 0.0), Vec3::NEG_X, 5.0)
            .is_none());
    }

    #[test]
    fn sweep_stops_at_wall() {
        let w = StaticWorld::test_arena();
        let p = w.move_capsule(
            Vec3::new(5.0, 0.91, 0.0),
            Vec3::new(10.0, 0.0, 0.0),
            STANDING,
        );
        assert!((p.x - (10.0 - CAPSULE_RADIUS)).abs() < 1e-2, "{p}");
        assert!(!w.intersects(p, STANDING));
    }

    #[test]
    fn overlap_detected() {
        let w = StaticWorld::test_arena();
        assert!(w.intersects(Vec3::new(10.0, 1.0, 0.0), STANDING));
        assert!(!w.intersects(Vec3::new(0.0, 0.91, 0.0), STANDING));
    }

    #[test]
    fn empty_world_has_no_hits() {
        let w = StaticWorld::default();
        assert!(w.ray_cast(Vec3::ZERO, Vec3::X, 100.0).is_none());
        assert!(w.shape_cast(Vec3::ZERO, Vec3::X, STANDING).is_none());
        assert!(!w.intersects(Vec3::ZERO, STANDING));
    }

    /// Brute-force reference: the former linear scan, kept to check that the
    /// BVH broad phase never changes a result.
    fn brute_ray(w: &StaticWorld, origin: Vec3, dir: Vec3, max: f32) -> Option<RayHit> {
        let ray = Ray::new(v3(origin), v3(dir));
        let mut best: Option<RayHit> = None;
        for (i, vol) in w.volumes().iter().enumerate() {
            if let Some(h) = vol
                .shape
                .cast_ray_and_get_normal(&vol.pose, &ray, max, true)
            {
                if best.is_none_or(|b| h.time_of_impact < b.distance) {
                    best = Some(RayHit {
                        distance: h.time_of_impact,
                        normal: to_glam(h.normal),
                        volume: i,
                    });
                }
            }
        }
        best
    }

    fn brute_sweep(w: &StaticWorld, from: Vec3, delta: Vec3) -> Option<SweepHit> {
        let cap = player_capsule(STANDING);
        let pose = translation(from);
        let opts = ShapeCastOptions {
            max_time_of_impact: 1.0,
            target_distance: 0.0,
            stop_at_penetration: true,
            compute_impact_geometry_on_penetration: true,
        };
        let mut best: Option<SweepHit> = None;
        for (i, vol) in w.volumes().iter().enumerate() {
            let hit = query::cast_shapes(
                &vol.pose,
                Vector::ZERO,
                vol.shape.as_ref(),
                &pose,
                v3(delta),
                &cap,
                opts,
            );
            if let Ok(Some(h)) = hit {
                let normal = to_glam(vol.pose.rotation * h.normal1);
                if normal.dot(delta) >= 0.0 {
                    continue;
                }
                if best.is_none_or(|b| h.time_of_impact < b.toi) {
                    best = Some(SweepHit {
                        toi: h.time_of_impact.clamp(0.0, 1.0),
                        normal,
                        volume: i,
                    });
                }
            }
        }
        best
    }

    fn brute_intersects(w: &StaticWorld, center: Vec3) -> bool {
        let cap = player_capsule(STANDING);
        let pose = translation(center);
        w.volumes().iter().any(|v| overlaps(v, &pose, &cap))
    }

    /// Maximum tolerated penetration after a sweep, in metres (documented).
    const EPS: f32 = 1e-3;

    proptest! {
        #[test]
        fn sweep_never_penetrates_box(
            half in (0.2f32..3.0, 0.2f32..3.0, 0.2f32..3.0),
            start in (-8.0f32..8.0, -4.0f32..6.0, -8.0f32..8.0),
            delta in (-12.0f32..12.0, -8.0f32..8.0, -12.0f32..12.0),
        ) {
            let half = Vec3::new(half.0, half.1, half.2);
            let w = StaticWorld::new(vec![Volume::cuboid(Vec3::ZERO, half)]);
            let from = Vec3::new(start.0, start.1, start.2);
            prop_assume!(!w.intersects(from, STANDING));
            let delta = Vec3::new(delta.0, delta.1, delta.2);
            let end = w.move_capsule(from, delta, STANDING);
            let vol = &w.volumes()[0];
            let cap = player_capsule(STANDING);
            let pen = query::contact(&vol.pose, vol.shape.as_ref(), &translation(end), &cap, 0.0)
                .unwrap()
                .map_or(0.0, |c| -c.dist);
            prop_assert!(pen <= EPS, "penetration {pen}");
        }

        #[test]
        fn bvh_matches_brute_force(
            boxes in prop::collection::vec(
                ((-20.0f32..20.0, -5.0f32..5.0, -20.0f32..20.0), (0.2f32..3.0, 0.2f32..3.0, 0.2f32..3.0)),
                0..40,
            ),
            start in (-20.0f32..20.0, -5.0f32..5.0, -20.0f32..20.0),
            delta in (-15.0f32..15.0, -8.0f32..8.0, -15.0f32..15.0),
            dir in (-1.0f32..1.0, -1.0f32..1.0, -1.0f32..1.0),
        ) {
            let vols = boxes
                .iter()
                .map(|(c, h)| Volume::cuboid(Vec3::new(c.0, c.1, c.2), Vec3::new(h.0, h.1, h.2)))
                .collect();
            let w = StaticWorld::new(vols);
            let from = Vec3::new(start.0, start.1, start.2);
            let delta = Vec3::new(delta.0, delta.1, delta.2);
            prop_assert_eq!(w.intersects(from, STANDING), brute_intersects(&w, from));
            prop_assert_eq!(w.shape_cast(from, delta, STANDING), brute_sweep(&w, from, delta));
            let dir = Vec3::new(dir.0, dir.1, dir.2);
            prop_assume!(dir.length() > 1e-3);
            let dir = dir.normalize();
            prop_assert_eq!(w.ray_cast(from, dir, 50.0), brute_ray(&w, from, dir, 50.0));
        }
    }
}

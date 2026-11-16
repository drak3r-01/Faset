//! Static collision world and the three base queries: ray cast, shape cast
//! (sweep) and intersection test, built on `parry3d`.
//!
//! Decision: volumes live in a flat list instead of a parry BVH; arenas hold
//! tens to hundreds of convex volumes and a linear scan keeps iteration order
//! (and results) deterministic. The query API is unchanged if a BVH tree is
//! introduced later.

use faset_core::glam::Vec3;
use parry3d::math::{Isometry, Point, Vector};
use parry3d::query::{self, Ray, ShapeCastOptions};
use parry3d::shape::{Capsule, SharedShape};

use crate::components::{CAPSULE_RADIUS, STANDING_HEIGHT};

/// One convex static volume (box, ramp, cylinder, prism, ...).
#[derive(Clone)]
pub struct Volume {
    pub shape: SharedShape,
    pub pose: Isometry<f32>,
}

impl Volume {
    /// Axis-aligned box from its centre and half extents.
    #[must_use]
    pub fn cuboid(center: Vec3, half: Vec3) -> Self {
        Self {
            shape: SharedShape::cuboid(half.x, half.y, half.z),
            pose: Isometry::translation(center.x, center.y, center.z),
        }
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

#[derive(Clone, Default)]
pub struct StaticWorld {
    volumes: Vec<Volume>,
}

fn v3(v: Vec3) -> Vector<f32> {
    Vector::new(v.x, v.y, v.z)
}

fn to_glam(v: &Vector<f32>) -> Vec3 {
    Vec3::new(v.x, v.y, v.z)
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
        Self { volumes }
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

    /// Closest hit of a ray; `dir` must be normalised.
    #[must_use]
    pub fn ray_cast(&self, origin: Vec3, dir: Vec3, max_dist: f32) -> Option<RayHit> {
        let ray = Ray::new(Point::new(origin.x, origin.y, origin.z), v3(dir));
        let mut best: Option<RayHit> = None;
        for (i, vol) in self.volumes.iter().enumerate() {
            if let Some(h) = vol
                .shape
                .cast_ray_and_get_normal(&vol.pose, &ray, max_dist, true)
            {
                if best.is_none_or(|b| h.time_of_impact < b.distance) {
                    best = Some(RayHit {
                        distance: h.time_of_impact,
                        normal: to_glam(&h.normal),
                        volume: i,
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
        let pose = Isometry::translation(from.x, from.y, from.z);
        let vel = v3(delta);
        let opts = ShapeCastOptions {
            max_time_of_impact: 1.0,
            target_distance: 0.0,
            stop_at_penetration: true,
            compute_impact_geometry_on_penetration: true,
        };
        let mut best: Option<SweepHit> = None;
        for (i, vol) in self.volumes.iter().enumerate() {
            let hit = query::cast_shapes(
                &vol.pose,
                &Vector::zeros(),
                vol.shape.as_ref(),
                &pose,
                &vel,
                &cap,
                opts,
            );
            if let Ok(Some(h)) = hit {
                let normal = to_glam(&(vol.pose.rotation * h.normal1.into_inner()));
                // Ignore surfaces the capsule is moving away from or along.
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

    /// Whether a vertical capsule at `center` overlaps any volume.
    #[must_use]
    pub fn intersects(&self, center: Vec3, height: f32) -> bool {
        let cap = player_capsule(height);
        let pose = Isometry::translation(center.x, center.y, center.z);
        self.volumes.iter().any(|v| {
            query::intersection_test(&v.pose, v.shape.as_ref(), &pose, &cap).unwrap_or(false)
        })
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
            let pose = Isometry::translation(end.x, end.y, end.z);
            let cap = player_capsule(STANDING);
            let pen = query::contact(&vol.pose, vol.shape.as_ref(), &pose, &cap, 0.0)
                .unwrap()
                .map_or(0.0, |c| -c.dist);
            prop_assert!(pen <= EPS, "penetration {pen}");
        }
    }
}

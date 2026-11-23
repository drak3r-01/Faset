//! Collision world of a map: one convex volume per brush, queried through
//! the [`StaticWorld`] BVH.
//!
//! The map must be expanded already (`faset_mapfmt::expand`). Boxes are
//! cuboids; wedges, cylinders, prisms are convex hulls of their vertices and
//! stairs give one ramp volume (their bounding wedge), as in plan section 5.
//!
//! Decision: until recipes declare prop collision (lot 2.13 data is not read
//! here), each prop is a 1 m cube standing on its position, scaled by `scale`
//! and turned by `yaw`.

use faset_core::glam::Vec3;
use faset_mapfmt::geometry::brush_hull_points;
use faset_mapfmt::{grid_to_metres, Brush, MapFile};
use parry3d::math::{Pose, Rotation, Vector};
use parry3d::shape::SharedShape;

use crate::collision::{RayHit, StaticWorld, SweepHit, Volume};
use crate::components::{CAPSULE_RADIUS, STANDING_HEIGHT};

/// Player capsule radius in metres.
pub const PLAYER_RADIUS: f32 = CAPSULE_RADIUS;
/// Standing capsule height in metres.
pub const PLAYER_HEIGHT: f32 = STANDING_HEIGHT;
/// Crouching capsule height in metres.
pub const PLAYER_CROUCH_HEIGHT: f32 = 1.0;

/// Edge of the placeholder prop box, in metres.
const PROP_BOX_M: f32 = 1.0;

/// Static collision of one map.
#[derive(Clone, Default)]
pub struct MapCollision {
    world: StaticWorld,
}

fn v3(v: Vec3) -> Vector {
    Vector::new(v.x, v.y, v.z)
}

impl MapCollision {
    /// Builds the collision of an expanded map.
    #[must_use]
    pub fn build(map: &MapFile) -> Self {
        let mut volumes = Vec::with_capacity(map.brushes.len() + map.props.len());
        for brush in &map.brushes {
            if let Some(v) = brush_volume(brush) {
                volumes.push(v);
            }
        }
        for p in &map.props {
            let half = 0.5 * PROP_BOX_M * p.scale;
            let centre = Vec3::from_array(p.at) + Vec3::new(0.0, half, 0.0);
            let rot = Rotation::from_rotation_y(p.yaw.to_radians());
            volumes.push(Volume {
                shape: SharedShape::cuboid(half, half, half),
                pose: Pose::from_parts(v3(centre), rot),
            });
        }
        Self {
            world: StaticWorld::new(volumes),
        }
    }

    /// The underlying static world.
    #[must_use]
    pub fn world(&self) -> &StaticWorld {
        &self.world
    }

    /// Number of convex volumes.
    #[must_use]
    pub fn volume_count(&self) -> usize {
        self.world.volumes().len()
    }

    /// Distance to the closest surface along `dir` (any non-zero length), if
    /// closer than `max_dist`.
    #[must_use]
    pub fn ray_cast(&self, origin: Vec3, dir: Vec3, max_dist: f32) -> Option<f32> {
        self.ray_cast_hit(origin, dir, max_dist).map(|h| h.distance)
    }

    /// Like [`Self::ray_cast`] with the surface normal and volume index.
    #[must_use]
    pub fn ray_cast_hit(&self, origin: Vec3, dir: Vec3, max_dist: f32) -> Option<RayHit> {
        let dir = dir.try_normalize()?;
        self.world.ray_cast(origin, dir, max_dist)
    }

    /// Sweeps a vertical capsule of total `height` (centre `from`) by `delta`;
    /// the earliest blocking hit, or `None` if the motion is free.
    #[must_use]
    pub fn sweep_capsule(&self, from: Vec3, delta: Vec3, height: f32) -> Option<SweepHit> {
        self.world.shape_cast(from, delta, height)
    }

    /// Whether a vertical capsule of total `height` centred at `center`
    /// overlaps the map.
    #[must_use]
    pub fn overlaps_capsule(&self, center: Vec3, height: f32) -> bool {
        self.world.intersects(center, height)
    }
}

fn brush_volume(brush: &Brush) -> Option<Volume> {
    if let Brush::Box { min, max, .. } = brush {
        let (lo, hi) = (grid_to_metres(*min), grid_to_metres(*max));
        return Some(Volume::cuboid((lo + hi) * 0.5, (hi - lo) * 0.5));
    }
    let points: Vec<Vector> = brush_hull_points(brush).into_iter().map(v3).collect();
    SharedShape::convex_hull(&points).map(|shape| Volume {
        shape,
        pose: Pose::IDENTITY,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example() -> (MapFile, MapCollision) {
        let text = include_str!("../../../assets/testdata/example.ron");
        let map = faset_mapfmt::expand(&faset_mapfmt::parse(text.as_bytes()).unwrap());
        let col = MapCollision::build(&map);
        (map, col)
    }

    #[test]
    fn one_volume_per_brush_and_prop() {
        let (map, col) = example();
        assert_eq!(col.volume_count(), map.brushes.len() + map.props.len());
    }

    #[test]
    fn spawn_stands_on_the_floor() {
        let (map, col) = example();
        for s in &map.spawns {
            let feet = Vec3::from_array(s.at);
            let d = col
                .ray_cast(feet + Vec3::Y, Vec3::NEG_Y, 5.0)
                .expect("floor below the spawn");
            assert!((d - 1.0).abs() < 0.05, "floor at {d} below the spawn");
            let centre = feet + Vec3::new(0.0, PLAYER_HEIGHT * 0.5 + 0.01, 0.0);
            assert!(!col.overlaps_capsule(centre, PLAYER_HEIGHT), "{s:?}");
            // Falling by 1 m stops on the floor, not below.
            let hit = col
                .sweep_capsule(centre, Vec3::new(0.0, -1.0, 0.0), PLAYER_HEIGHT)
                .expect("the floor stops the fall");
            assert!(hit.toi < 0.05, "{}", hit.toi);
        }
    }

    #[test]
    fn sweep_toward_a_wall_stops_before_it() {
        let (_, col) = example();
        // The red container spans x 2..5, z -3..-1.5, up to y = 1.5.
        let from = Vec3::new(-2.0, 0.95, -2.25);
        let hit = col
            .sweep_capsule(from, Vec3::new(10.0, 0.0, 0.0), PLAYER_HEIGHT)
            .expect("the container blocks the way");
        let stop = from + Vec3::new(10.0, 0.0, 0.0) * hit.toi;
        assert!((stop.x - (2.0 - PLAYER_RADIUS)).abs() < 0.02, "{stop}");
        // The sweep stops in contact; a hair back is free.
        assert!(!col.overlaps_capsule(stop - Vec3::X * 1e-3, PLAYER_HEIGHT));
    }

    #[test]
    fn ray_cast_normalises_its_direction() {
        let (_, col) = example();
        let a = col.ray_cast(Vec3::new(0.0, 1.0, -2.0), Vec3::new(0.0, -3.0, 0.0), 5.0);
        let b = col.ray_cast(Vec3::new(0.0, 1.0, -2.0), Vec3::NEG_Y, 5.0);
        assert_eq!(a, b);
        assert!(col.ray_cast(Vec3::ZERO, Vec3::ZERO, 5.0).is_none());
    }
}

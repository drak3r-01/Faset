//! Gameplay zones (lot 4.3, plan sections 13 and 15).
//!
//! [`Zones::apply`] runs at the end of a player tick, after
//! [`crate::controller::step_player`]. It compares the zones the player
//! overlaps with those of the previous tick ([`ZoneTracker`]) and emits
//! [`ZoneEvent`]s: `Left` for each zone left, then for each zone entered
//! `Entered` followed by `Triggered(kind)`, in ascending zone order.
//!
//! Effects on entry:
//! - `JumpPad`: the player's velocity becomes the zone velocity (capped at
//!   [`MoveParams::max_speed`]) and the player leaves the ground.
//! - `Teleporter`: the player's feet move to `target`; see decisions.
//! - `KillVolume`, `Water`, `Objective`: event only. Killing is applied by
//!   whoever owns health ([`crate::World`] sets it to 0); water physics and
//!   objective rules belong to later lots.
//!
//! `LowGravity` is not an entry effect: while the player overlaps it, the
//! gravity of the next ticks is multiplied by its scale
//! ([`PlayerState::gravity_scale`]).
//!
//! Decisions (unspecified by the plan):
//! - The player is inside a zone when its capsule's bounding box overlaps
//!   the zone box (Quake-style trigger touch), open intervals.
//! - Triggers fire once per entry; staying inside does nothing more.
//! - Overlapping low-gravity zones multiply their scales.
//! - Teleporter `target` is the destination of the feet (as spawns); the
//!   horizontal speed is kept and redirected along the target yaw (degrees,
//!   0 faces -Z, positive turns left as the player yaw), vertical speed is
//!   zeroed. The view yaw itself is the client's: it comes with the
//!   `Triggered(Teleporter { .. })` event.
//! - The zone of an event is its index in [`Zones::volumes`] (map order).

use faset_core::glam::Vec3;
use faset_mapfmt::{grid_to_metres, MapFile, Zone, ZoneKind};

use crate::controller::{sin_cos_q, MoveParams, PlayerState, SKIN};
use crate::map_collision::PLAYER_RADIUS;

/// What a zone did to a player on one tick.
#[derive(Debug, Clone, PartialEq)]
pub enum ZoneEvent {
    /// The player entered zone `.0` (index in [`Zones::volumes`]).
    Entered(usize),
    /// The player left zone `.0`.
    Left(usize),
    /// The effect of an entered zone fired.
    Triggered(ZoneKind),
}

/// One zone box, in metres.
#[derive(Debug, Clone, PartialEq)]
pub struct ZoneVolume {
    pub min: Vec3,
    pub max: Vec3,
    pub kind: ZoneKind,
}

impl ZoneVolume {
    fn overlaps(&self, lo: Vec3, hi: Vec3) -> bool {
        lo.x < self.max.x
            && hi.x > self.min.x
            && lo.y < self.max.y
            && hi.y > self.min.y
            && lo.z < self.max.z
            && hi.z > self.min.z
    }
}

/// The zones of a map.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Zones {
    volumes: Vec<ZoneVolume>,
}

/// Zones a player was in at the end of its last tick (ascending indices).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ZoneTracker {
    inside: Vec<usize>,
}

impl ZoneTracker {
    /// Indices of the zones the player is in.
    #[must_use]
    pub fn inside(&self) -> &[usize] {
        &self.inside
    }
}

impl Zones {
    #[must_use]
    pub fn new(volumes: Vec<ZoneVolume>) -> Self {
        Self { volumes }
    }

    /// The zones of an expanded map (`faset_mapfmt::expand`).
    #[must_use]
    pub fn from_map(map: &MapFile) -> Self {
        let volumes = map
            .zones
            .iter()
            .map(|z| match z {
                Zone::Box { min, max, kind } => {
                    let (a, b) = (grid_to_metres(*min), grid_to_metres(*max));
                    ZoneVolume {
                        min: a.min(b),
                        max: a.max(b),
                        kind: kind.clone(),
                    }
                }
            })
            .collect();
        Self { volumes }
    }

    #[must_use]
    pub fn volumes(&self) -> &[ZoneVolume] {
        &self.volumes
    }

    /// Updates zone membership and applies the zone effects; see the module
    /// docs. Events are appended to `events`.
    pub fn apply(
        &self,
        s: &mut PlayerState,
        tracker: &mut ZoneTracker,
        p: &MoveParams,
        events: &mut Vec<ZoneEvent>,
    ) {
        let half = Vec3::new(PLAYER_RADIUS, 0.5 * s.height(), PLAYER_RADIUS);
        let (lo, hi) = (s.pos - half, s.pos + half);
        let now: Vec<usize> = (0..self.volumes.len())
            .filter(|&i| self.volumes[i].overlaps(lo, hi))
            .collect();
        for &i in &tracker.inside {
            if !now.contains(&i) {
                events.push(ZoneEvent::Left(i));
            }
        }
        s.gravity_scale = now
            .iter()
            .filter_map(|&i| match self.volumes[i].kind {
                ZoneKind::LowGravity { scale } => Some(scale),
                _ => None,
            })
            .product();
        for &i in &now {
            if tracker.inside.contains(&i) {
                continue;
            }
            let kind = &self.volumes[i].kind;
            events.push(ZoneEvent::Entered(i));
            events.push(ZoneEvent::Triggered(kind.clone()));
            trigger(kind, s, p);
        }
        tracker.inside = now;
    }
}

fn leave_ground(s: &mut PlayerState) {
    s.on_ground = false;
    s.slide_timer = 0.0;
    s.coyote_timer = 0.0;
}

fn trigger(kind: &ZoneKind, s: &mut PlayerState, p: &MoveParams) {
    match kind {
        ZoneKind::JumpPad { velocity } => {
            s.vel = Vec3::from_array(*velocity).clamp_length_max(p.max_speed);
            leave_ground(s);
        }
        ZoneKind::Teleporter { target, yaw } => {
            let speed = Vec3::new(s.vel.x, 0.0, s.vel.z).length();
            let q = ((yaw / 360.0).rem_euclid(1.0) * 65536.0).round() as u32 as u16;
            let (sin, cos) = sin_cos_q(q);
            s.vel = Vec3::new(-sin, 0.0, -cos) * speed;
            s.pos = Vec3::from_array(*target) + Vec3::Y * (0.5 * s.height() + SKIN);
            leave_ground(s);
        }
        ZoneKind::KillVolume
        | ZoneKind::Water
        | ZoneKind::LowGravity { .. }
        | ZoneKind::Objective { .. } => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zone(min: Vec3, max: Vec3, kind: ZoneKind) -> ZoneVolume {
        ZoneVolume { min, max, kind }
    }

    #[test]
    fn example_map_zones_are_read_in_metres() {
        let text = include_str!("../../../assets/testdata/example.ron");
        let map = faset_mapfmt::expand(&faset_mapfmt::parse(text.as_bytes()).unwrap());
        let zones = Zones::from_map(&map);
        assert_eq!(zones.volumes().len(), map.zones.len());
        assert!(zones
            .volumes()
            .iter()
            .all(|z| z.min.x < z.max.x && z.min.y < z.max.y && z.min.z < z.max.z));
    }

    #[test]
    fn enter_stay_leave_emit_events_once() {
        let p = MoveParams::default();
        let zones = Zones::new(vec![
            zone(
                Vec3::new(1.0, 0.0, -1.0),
                Vec3::new(2.0, 2.0, 1.0),
                ZoneKind::Water,
            ),
            zone(
                Vec3::new(1.5, 0.0, -1.0),
                Vec3::new(3.0, 2.0, 1.0),
                ZoneKind::LowGravity { scale: 0.5 },
            ),
        ]);
        let mut s = PlayerState::spawn(Vec3::new(0.0, 0.91, 0.0));
        let mut t = ZoneTracker::default();
        let mut ev = Vec::new();
        let mut at = |x: f32, s: &mut PlayerState, ev: &mut Vec<ZoneEvent>| {
            s.pos.x = x;
            ev.clear();
            zones.apply(s, &mut t, &p, ev);
        };
        at(0.0, &mut s, &mut ev);
        assert!(ev.is_empty());
        at(1.0, &mut s, &mut ev); // capsule spans 0.65..1.35
        assert_eq!(
            ev,
            vec![ZoneEvent::Entered(0), ZoneEvent::Triggered(ZoneKind::Water)]
        );
        at(1.1, &mut s, &mut ev);
        assert!(ev.is_empty());
        assert_eq!(s.gravity_scale, 1.0);
        at(2.5, &mut s, &mut ev);
        assert_eq!(ev[0], ZoneEvent::Left(0));
        assert_eq!(ev[1], ZoneEvent::Entered(1));
        assert_eq!(s.gravity_scale, 0.5);
        at(5.0, &mut s, &mut ev);
        assert_eq!(ev, vec![ZoneEvent::Left(1)]);
        assert_eq!(s.gravity_scale, 1.0);
    }

    #[test]
    fn teleporter_keeps_speed_along_the_target_yaw() {
        let p = MoveParams::default();
        let kind = ZoneKind::Teleporter {
            target: [10.0, 2.0, 0.0],
            yaw: 90.0,
        };
        let zones = Zones::new(vec![zone(Vec3::splat(-1.0), Vec3::splat(1.0), kind)]);
        let mut s = PlayerState::spawn(Vec3::ZERO);
        s.vel = Vec3::new(0.0, -3.0, -6.0);
        s.on_ground = true;
        zones.apply(&mut s, &mut ZoneTracker::default(), &p, &mut Vec::new());
        assert!((s.feet() - Vec3::new(10.0, 2.0 + SKIN, 0.0)).length() < 1e-5);
        // Yaw 90° faces -X.
        assert!(
            (s.vel - Vec3::new(-6.0, 0.0, 0.0)).length() < 1e-4,
            "{}",
            s.vel
        );
        assert!(!s.on_ground);
    }
}

//! Collision history and lag compensation of hitscan shots.
//!
//! The server records every player's hitboxes each tick and keeps
//! [`HISTORY_TICKS`] of them (one second). A shot names the instant the
//! shooter saw (the [`FireCommand`]'s view tick and fraction, the client's
//! interpolation render time); [`LagCompensation::rewind_and_cast`]:
//!
//! 1. checks that instant against the shooter's measured round trip and
//!    clamps it to at most [`MAX_REWIND`] in the past;
//! 2. places every other player at that instant, interpolating linearly
//!    between the two history entries around it (60 Hz entries: linear is
//!    enough, decision);
//! 3. casts the ray from the shooter's authoritative (newest) eye position
//!    against the placed hitboxes and the static map;
//! 4. puts every hitbox back at its authoritative position.
//!
//! Slow projectiles are not compensated: they never go through this module.
//!
//! Times are in ticks of the history labels: an entry recorded during the
//! step of tick `n` is labelled `n + 1`, the world tick after the step, the
//! same label as the snapshot describing that state. A client's render time
//! is therefore directly a history time.
//!
//! Hitboxes: a vertical capsule of radius 0.35 m for the body, 1.8 m high
//! standing and 1.0 m sliding, standing on the same feet (decision: the sim
//! position is the centre of the standing capsule, so the feet are 0.9 m
//! below it), and a sphere for the head. Decisions: head radius
//! [`HEAD_RADIUS`] (0.2 m) with its top level with the top of the body
//! capsule; eye [`EYE_HEIGHT`] (1.6 m) above the feet, lowered by the same
//! 0.8 m when sliding.
//!
//! Consistency (decision): the claimed instant may not be in the future (it
//! is then taken as now) nor older than the shooter's round trip plus
//! [`CONSISTENCY_SLACK`], which covers the interpolation delay `d` of the
//! client (66 ms at 30 Hz snapshots, more with jitter); an older claim is
//! moved to that bound rather than rejected, so a shot is never dropped.

use std::collections::{BTreeMap, VecDeque};

use faset_core::glam::Vec3;
use faset_core::{NetId, Tick, DT, TICK_RATE};
use faset_sim::collision::player_capsule;
use faset_sim::components::{SLIDING_HEIGHT, STANDING_HEIGHT};
use faset_sim::StaticWorld;
use parry3d::math::{Pose, Vector};
use parry3d::query::{Ray, RayCast};
use parry3d::shape::Ball;

/// History entries kept per player: one second of ticks plus the current
/// one, so an instant exactly one second old is still bracketed.
pub const HISTORY_TICKS: usize = TICK_RATE as usize + 1;
/// Furthest a shot is rewound, in seconds.
pub const MAX_REWIND: f32 = 0.25;
/// Allowance over the shooter's round trip for its interpolation delay,
/// in seconds (decision).
pub const CONSISTENCY_SLACK: f32 = 0.15;
/// Head sphere radius in metres (decision).
pub const HEAD_RADIUS: f32 = 0.2;
/// Eye height above the feet when standing, metres (decision).
pub const EYE_HEIGHT: f32 = 1.6;

/// One player's hitboxes at one instant.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hitbox {
    /// The sim position: centre of the standing capsule, metres.
    pub position: Vec3,
    /// Sliding players have a 1.0 m body capsule.
    pub sliding: bool,
}

impl Hitbox {
    /// A standing player's hitbox at sim position `position`.
    #[must_use]
    pub fn standing(position: Vec3) -> Self {
        Self {
            position,
            sliding: false,
        }
    }

    /// Bottom of the body capsule.
    #[must_use]
    pub fn feet(&self) -> Vec3 {
        self.position - Vec3::Y * (STANDING_HEIGHT * 0.5)
    }

    /// Total body capsule height.
    #[must_use]
    pub fn body_height(&self) -> f32 {
        if self.sliding {
            SLIDING_HEIGHT
        } else {
            STANDING_HEIGHT
        }
    }

    /// Centre of the body capsule.
    #[must_use]
    pub fn body_center(&self) -> Vec3 {
        self.feet() + Vec3::Y * (self.body_height() * 0.5)
    }

    /// Centre of the head sphere.
    #[must_use]
    pub fn head_center(&self) -> Vec3 {
        self.feet() + Vec3::Y * (self.body_height() - HEAD_RADIUS)
    }

    /// Where shots leave from.
    #[must_use]
    pub fn eye(&self) -> Vec3 {
        self.feet() + Vec3::Y * (EYE_HEIGHT - (STANDING_HEIGHT - self.body_height()))
    }

    /// Linear blend; the posture is the nearer entry's.
    #[must_use]
    pub fn lerp(self, other: Self, s: f32) -> Self {
        Self {
            position: self.position.lerp(other.position, s),
            sliding: if s < 0.5 { self.sliding } else { other.sliding },
        }
    }
}

/// What the server needs from a hitscan fire command.
///
/// Decision: the wire form belongs to the protocol (weapons, lot 4.4); this
/// is its decoded content.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FireCommand {
    /// Interpolation tick the shooter saw (its render time, floored).
    pub view_tick: Tick,
    /// Fraction from `view_tick` toward the next tick, in [0, 1).
    pub view_fraction: f32,
    /// Aim direction; normalised by the server.
    pub direction: Vec3,
    /// Weapon range in metres.
    pub range: f32,
}

impl FireCommand {
    /// The claimed instant in ticks.
    #[must_use]
    pub fn view_time(&self) -> f64 {
        f64::from(self.view_tick.0) + f64::from(self.view_fraction.clamp(0.0, 1.0))
    }
}

/// Part of a hitbox a shot touched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyPart {
    Head,
    Body,
}

/// The first player a shot touched.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hit {
    pub target: NetId,
    pub part: BodyPart,
    /// From the shooter's eye, metres.
    pub distance: f32,
    pub point: Vec3,
}

/// Result of [`LagCompensation::rewind_and_cast`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShotOutcome {
    /// The instant the other players were placed at, in ticks.
    pub rewound_to: f64,
    /// Whether the claimed instant was moved (inconsistent or too old).
    pub clamped: bool,
    /// `None` for a miss or a shot stopped by the map first.
    pub hit: Option<Hit>,
}

/// Per-player hitbox history and the hitboxes currently placed for queries.
#[derive(Debug, Clone, Default)]
pub struct LagCompensation {
    /// Oldest first, at most [`HISTORY_TICKS`] entries each.
    history: BTreeMap<NetId, VecDeque<(Tick, Hitbox)>>,
    /// Where each hitbox is for queries: authoritative except during a cast.
    placed: BTreeMap<NetId, Hitbox>,
    now: Option<Tick>,
}

fn v3(v: Vec3) -> Vector {
    Vector::new(v.x, v.y, v.z)
}

fn at(p: Vec3) -> Pose {
    Pose::translation(p.x, p.y, p.z)
}

impl LagCompensation {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records every player's hitboxes for history label `tick`. Players not
    /// listed are dropped with their history (they left).
    pub fn record(&mut self, tick: Tick, hitboxes: impl IntoIterator<Item = (NetId, Hitbox)>) {
        let mut placed = BTreeMap::new();
        for (id, hb) in hitboxes {
            let ring = self.history.entry(id).or_default();
            ring.push_back((tick, hb));
            while ring.len() > HISTORY_TICKS {
                ring.pop_front();
            }
            placed.insert(id, hb);
        }
        self.history.retain(|id, _| placed.contains_key(id));
        self.placed = placed;
        self.now = Some(tick);
    }

    /// Label of the newest record.
    #[must_use]
    pub fn now(&self) -> Option<Tick> {
        self.now
    }

    /// The history of one player, oldest first.
    pub fn history(&self, id: NetId) -> impl Iterator<Item = (Tick, Hitbox)> + '_ {
        self.history.get(&id).into_iter().flatten().copied()
    }

    /// Where the hitbox of `id` is placed now (authoritative outside a cast).
    #[must_use]
    pub fn hitbox(&self, id: NetId) -> Option<Hitbox> {
        self.placed.get(&id).copied()
    }

    /// The newest recorded hitbox of `id`.
    #[must_use]
    pub fn authoritative(&self, id: NetId) -> Option<Hitbox> {
        self.history.get(&id)?.back().map(|&(_, hb)| hb)
    }

    /// The hitbox of `id` at `time` (ticks), interpolated between history
    /// entries and clamped to the history's ends.
    #[must_use]
    pub fn hitbox_at(&self, id: NetId, time: f64) -> Option<Hitbox> {
        let ring = self.history.get(&id)?;
        let &(first_t, first) = ring.front()?;
        let &(last_t, last) = ring.back()?;
        if time <= f64::from(first_t.0) {
            return Some(first);
        }
        if time >= f64::from(last_t.0) {
            return Some(last);
        }
        let i = ring.iter().position(|&(t, _)| f64::from(t.0) > time)?;
        let (t0, a) = ring[i - 1];
        let (t1, b) = ring[i];
        let s = (time - f64::from(t0.0)) / f64::from(t1.0 - t0.0);
        Some(a.lerp(b, s as f32))
    }

    /// The instant a shot is rewound to, in ticks, and whether the claim
    /// was moved: consistency with `shooter_rtt` (seconds), then the
    /// [`MAX_REWIND`] cap. `None` before the first record.
    #[must_use]
    pub fn resolve_view_time(&self, fire: &FireCommand, shooter_rtt: f32) -> Option<(f64, bool)> {
        let now = f64::from(self.now?.0);
        let dt = f64::from(DT);
        let claimed = fire.view_time();
        let mut age = (now - claimed) * dt;
        let max_consistent = f64::from(shooter_rtt.max(0.0) + CONSISTENCY_SLACK);
        let limit = max_consistent.min(f64::from(MAX_REWIND));
        let clamped = !(0.0..=limit).contains(&age);
        if clamped {
            tracing::debug!(age, limit, "fire command instant clamped");
        }
        age = age.clamp(0.0, limit);
        Some((now - age / dt, clamped))
    }

    /// Rewinds every player but `shooter` to the instant of `fire`, casts
    /// the shot from the shooter's authoritative eye against them and
    /// `statics`, then restores every hitbox. `None` when the shooter has
    /// no history.
    pub fn rewind_and_cast(
        &mut self,
        statics: &StaticWorld,
        shooter: NetId,
        fire: &FireCommand,
        shooter_rtt: f32,
    ) -> Option<ShotOutcome> {
        let origin = self.authoritative(shooter)?.eye();
        let (time, clamped) = self.resolve_view_time(fire, shooter_rtt)?;
        let ids: Vec<NetId> = self.placed.keys().copied().collect();
        for &id in &ids {
            if id != shooter {
                if let Some(hb) = self.hitbox_at(id, time) {
                    self.placed.insert(id, hb);
                }
            }
        }
        let hit = self.cast(statics, shooter, origin, fire);
        for &id in &ids {
            if let Some(hb) = self.authoritative(id) {
                self.placed.insert(id, hb);
            }
        }
        Some(ShotOutcome {
            rewound_to: time,
            clamped,
            hit,
        })
    }

    /// Earliest hit against the placed hitboxes before the map blocks the
    /// ray. Each player's head is tested before its body; between players
    /// the nearest contact wins, ties going to the lowest id.
    fn cast(
        &self,
        statics: &StaticWorld,
        shooter: NetId,
        origin: Vec3,
        fire: &FireCommand,
    ) -> Option<Hit> {
        let dir = fire.direction.try_normalize()?;
        let range = fire.range.max(0.0);
        let max = statics
            .ray_cast(origin, dir, range)
            .map_or(range, |h| h.distance);
        let ray = Ray::new(v3(origin), v3(dir));
        let head = Ball::new(HEAD_RADIUS);
        let mut best: Option<Hit> = None;
        for (&id, hb) in &self.placed {
            if id == shooter {
                continue;
            }
            // The head sits inside the top of the body capsule: a ray through
            // it is a head hit even though it enters the body first.
            let body = player_capsule(hb.body_height());
            let contact = head
                .cast_ray(&at(hb.head_center()), &ray, max, true)
                .map(|d| (BodyPart::Head, d))
                .or_else(|| {
                    body.cast_ray(&at(hb.body_center()), &ray, max, true)
                        .map(|d| (BodyPart::Body, d))
                });
            let Some((part, d)) = contact else { continue };
            if d <= max && best.is_none_or(|b| d < b.distance) {
                best = Some(Hit {
                    target: id,
                    part,
                    distance: d,
                    point: origin + dir * d,
                });
            }
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHOOTER: NetId = NetId(1);
    const TARGET: NetId = NetId(2);

    /// Target walking +X at `speed` from x = -5, shooter still 10 m away
    /// along +Z, `ticks` records.
    fn walking(speed: f32, ticks: u32) -> LagCompensation {
        let mut lag = LagCompensation::new();
        for t in 1..=ticks {
            let x = -5.0 + speed * t as f32 * DT;
            lag.record(
                Tick(t),
                [
                    (SHOOTER, Hitbox::standing(Vec3::new(0.0, 0.91, 10.0))),
                    (TARGET, Hitbox::standing(Vec3::new(x, 0.91, 0.0))),
                ],
            );
        }
        lag
    }

    fn aim_at(lag: &LagCompensation, point: Vec3, view: f64) -> FireCommand {
        let eye = lag.authoritative(SHOOTER).unwrap().eye();
        FireCommand {
            view_tick: Tick(view.floor() as u32),
            view_fraction: view.fract() as f32,
            direction: point - eye,
            range: 100.0,
        }
    }

    #[test]
    fn history_holds_one_second_per_player() {
        let lag = walking(5.0, 200);
        assert_eq!(lag.history(TARGET).count(), HISTORY_TICKS);
        assert_eq!(lag.history(TARGET).next().unwrap().0, Tick(200 - 60));
        // Departed players are dropped.
        let mut lag = lag;
        lag.record(Tick(201), [(SHOOTER, Hitbox::standing(Vec3::ZERO))]);
        assert_eq!(lag.history(TARGET).count(), 0);
        assert!(lag.hitbox(TARGET).is_none());
    }

    #[test]
    fn history_interpolates_between_entries() {
        let lag = walking(6.0, 100);
        let hb = lag.hitbox_at(TARGET, 50.5).unwrap();
        assert!((hb.position.x - (-5.0 + 6.0 * 50.5 * DT)).abs() < 1e-4);
    }

    #[test]
    fn a_shot_at_the_seen_position_hits_and_restores() {
        let mut lag = walking(5.0, 120);
        let statics = StaticWorld::test_arena();
        // The shooter saw the target 10 ticks (167 ms) ago, rtt 100 ms.
        let view = 110.25;
        let seen = lag.hitbox_at(TARGET, view).unwrap();
        let fire = aim_at(&lag, seen.body_center(), view);
        let before: Vec<_> = [SHOOTER, TARGET].map(|id| lag.hitbox(id).unwrap()).into();
        let out = lag.rewind_and_cast(&statics, SHOOTER, &fire, 0.1).unwrap();
        assert!(!out.clamped);
        assert!((out.rewound_to - view).abs() < 1e-4);
        let hit = out.hit.expect("rewound shot must hit");
        assert_eq!((hit.target, hit.part), (TARGET, BodyPart::Body));
        // Restore: everything is back at the authoritative positions.
        for (id, b) in [SHOOTER, TARGET].into_iter().zip(before) {
            assert_eq!(lag.hitbox(id), Some(b));
            assert_eq!(lag.hitbox(id), lag.authoritative(id));
        }
        // Without compensation the same aim misses (the target is 0.8 m on).
        let unrewound = FireCommand {
            view_tick: Tick(120),
            view_fraction: 0.0,
            ..fire
        };
        let out = lag
            .rewind_and_cast(&statics, SHOOTER, &unrewound, 0.1)
            .unwrap();
        assert!(out.hit.is_none());
    }

    #[test]
    fn a_300_ms_claim_is_clamped_to_250_ms() {
        let lag = walking(5.0, 120);
        // 300 ms = 18 ticks; consistent with a 200 ms round trip.
        let fire = aim_at(&lag, Vec3::ZERO, 120.0 - 18.0);
        let (t, clamped) = lag.resolve_view_time(&fire, 0.2).unwrap();
        assert!(clamped);
        assert!((t - (120.0 - 15.0)).abs() < 1e-6, "{t}");
        // A low-latency shooter cannot claim more than rtt + slack.
        let (t, clamped) = lag.resolve_view_time(&fire, 0.02).unwrap();
        assert!(clamped);
        let age = (120.0 - t) * f64::from(DT);
        assert!(
            (age - f64::from(0.02 + CONSISTENCY_SLACK)).abs() < 1e-6,
            "{age}"
        );
        // The future is now.
        let ahead = aim_at(&lag, Vec3::ZERO, 125.0);
        assert_eq!(lag.resolve_view_time(&ahead, 0.1), Some((120.0, true)));
    }

    #[test]
    fn heads_are_tested_first_and_walls_block() {
        let mut lag = LagCompensation::new();
        let target = Hitbox::standing(Vec3::new(5.0, 0.91, 0.0));
        lag.record(
            Tick(1),
            [
                (SHOOTER, Hitbox::standing(Vec3::new(0.0, 0.91, 0.0))),
                (TARGET, target),
            ],
        );
        let statics = StaticWorld::test_arena();
        let eye = lag.authoritative(SHOOTER).unwrap().eye();
        let fire = FireCommand {
            view_tick: Tick(1),
            view_fraction: 0.0,
            direction: target.head_center() - eye,
            range: 100.0,
        };
        let hit = lag
            .rewind_and_cast(&statics, SHOOTER, &fire, 0.0)
            .unwrap()
            .hit
            .unwrap();
        assert_eq!(hit.part, BodyPart::Head);
        // Behind the wall at x = 10 the target is safe.
        lag.record(
            Tick(2),
            [
                (SHOOTER, Hitbox::standing(Vec3::new(0.0, 0.91, 0.0))),
                (TARGET, Hitbox::standing(Vec3::new(12.0, 0.91, 0.0))),
            ],
        );
        let fire = FireCommand {
            view_tick: Tick(2),
            direction: Vec3::X,
            ..fire
        };
        assert!(lag
            .rewind_and_cast(&statics, SHOOTER, &fire, 0.0)
            .unwrap()
            .hit
            .is_none());
    }

    #[test]
    fn sliding_lowers_the_hitboxes() {
        let p = Vec3::new(0.0, 0.91, 0.0);
        let stand = Hitbox::standing(p);
        let slide = Hitbox {
            position: p,
            sliding: true,
        };
        assert_eq!(stand.feet(), slide.feet());
        assert!((stand.head_center().y - (p.y + 0.7)).abs() < 1e-6);
        assert!((slide.head_center().y - (p.y - 0.1)).abs() < 1e-6);
        assert!((slide.eye().y - stand.eye().y + 0.8).abs() < 1e-6);
    }
}

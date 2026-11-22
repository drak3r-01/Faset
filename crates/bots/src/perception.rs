//! Perception: field of view, range, line of sight by raycast, hearing, and a
//! queue that delays every observation by the level's reaction time.

use std::collections::VecDeque;

use faset_core::glam::Vec3;
use faset_core::NetId;
use faset_sim::StaticWorld;

use crate::aim::dir_of;
use crate::view::{RawEntity, RawState, Sighting};

/// Field of view of a bot, full width, degrees. The limit applies to the angle
/// between the view axis and the target: 55 degrees on each side (decision:
/// the plan says "110 degrees"; read as the full width of the cone).
pub const FOV_DEGREES: f32 = 110.0;

/// Line-of-sight refresh period per bot, ticks.
pub const LOS_PERIOD: u32 = 4;

/// Height of the aimed-at point above the target's feet, metres.
pub const TARGET_HEIGHT: f32 = 1.0;

/// How long a sighting is kept, seconds.
pub const MEMORY: f64 = 10.0;

/// Vision and hearing limits (decision: values not given by the plan).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PerceptionConfig {
    /// Full field of view, radians.
    pub fov: f32,
    /// Sight range, metres.
    pub sight_range: f32,
    /// Shots closer than this are heard, metres.
    pub hearing_range: f32,
}

impl Default for PerceptionConfig {
    fn default() -> Self {
        Self {
            fov: FOV_DEGREES.to_radians(),
            sight_range: 80.0,
            hearing_range: 30.0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Entry {
    id: NetId,
    pos: Vec3,
    vel: Vec3,
    seen: bool,
}

#[derive(Debug, Clone)]
struct Observation {
    time: f64,
    release: f64,
    entries: Vec<Entry>,
}

/// The delayed memory of one bot.
#[derive(Debug, Clone)]
pub struct Perception {
    cfg: PerceptionConfig,
    reaction: f64,
    phase: u32,
    queue: VecDeque<Observation>,
    known: Vec<Sighting>,
    los: Vec<(NetId, bool)>,
}

impl Perception {
    /// A perception delayed by `reaction_s`; `phase` spreads the line-of-sight
    /// refresh of several bots over the 4 ticks of the period.
    #[must_use]
    pub fn new(cfg: PerceptionConfig, reaction_s: f32, phase: u32) -> Self {
        Self {
            cfg,
            reaction: f64::from(reaction_s),
            phase,
            queue: VecDeque::new(),
            known: Vec::new(),
            los: Vec::new(),
        }
    }

    /// The limits in use.
    #[must_use]
    pub fn config(&self) -> &PerceptionConfig {
        &self.cfg
    }

    /// Forgets everything (after a respawn).
    pub fn clear(&mut self) {
        self.queue.clear();
        self.known.clear();
        self.los.clear();
    }

    /// Observes the world now, delivers the observations whose delay elapsed,
    /// and returns what the bot knows.
    pub fn update(&mut self, raw: &RawState, world: &StaticWorld) -> &[Sighting] {
        let obs = self.observe(raw, world);
        self.queue.push_back(obs);
        // A death is public: forget the dead.
        self.known
            .retain(|k| raw.enemies.iter().any(|e| e.id == k.id));
        while self
            .queue
            .front()
            .is_some_and(|o| o.release <= raw.time + 1e-9)
        {
            let Some(o) = self.queue.pop_front() else {
                break;
            };
            self.deliver(&o);
        }
        let now = raw.time;
        self.known.retain(|k| now - k.seen_at <= MEMORY);
        &self.known
    }

    fn deliver(&mut self, o: &Observation) {
        for k in &mut self.known {
            k.visible = false;
        }
        for e in &o.entries {
            if let Some(k) = self.known.iter_mut().find(|k| k.id == e.id) {
                // A heard shot never overwrites a fresher sighting.
                if e.seen || !k.seen || o.time >= k.seen_at {
                    k.pos = e.pos;
                    k.vel = e.vel;
                    k.seen_at = o.time;
                    k.seen = e.seen;
                    k.visible = e.seen;
                }
            } else {
                self.known.push(Sighting {
                    id: e.id,
                    pos: e.pos,
                    vel: e.vel,
                    seen_at: o.time,
                    seen: e.seen,
                    visible: e.seen,
                });
            }
        }
    }

    fn observe(&mut self, raw: &RawState, world: &StaticWorld) -> Observation {
        let me = &raw.me;
        let forward = dir_of(me.yaw, me.pitch);
        let cos_half = (self.cfg.fov * 0.5).cos();
        let in_view = |e: &RawEntity| -> Option<(Vec3, f32)> {
            let to = e.pos + Vec3::Y * TARGET_HEIGHT - me.eye;
            let dist = to.length();
            if dist > self.cfg.sight_range || dist < 1e-3 {
                return None;
            }
            (forward.dot(to / dist) >= cos_half).then_some((to / dist, dist))
        };
        if raw
            .tick
            .0
            .wrapping_add(self.phase)
            .is_multiple_of(LOS_PERIOD)
        {
            self.los.clear();
            for e in raw.enemies {
                if let Some((dir, dist)) = in_view(e) {
                    let clear = world.ray_cast(me.eye, dir, dist - 0.05).is_none();
                    self.los.push((e.id, clear));
                }
            }
        }
        let mut entries = Vec::new();
        for e in raw.enemies {
            let seen =
                in_view(e).is_some() && self.los.iter().any(|&(id, clear)| id == e.id && clear);
            if seen {
                entries.push(Entry {
                    id: e.id,
                    pos: e.pos,
                    vel: e.vel,
                    seen: true,
                });
            }
        }
        for s in raw.shots {
            if s.pos.distance(me.pos) <= self.cfg.hearing_range
                && !entries.iter().any(|e| e.id == s.shooter)
                && raw.enemies.iter().any(|e| e.id == s.shooter)
            {
                entries.push(Entry {
                    id: s.shooter,
                    pos: s.pos,
                    vel: Vec3::ZERO,
                    seen: false,
                });
            }
        }
        Observation {
            time: raw.time,
            release: raw.time + self.reaction,
            entries,
        }
    }
}

#[cfg(test)]
mod tests {
    use faset_core::{Tick, DT};
    use faset_sim::Volume;

    use super::*;
    use crate::view::{SelfState, WeaponParams};

    fn me(yaw: f32) -> SelfState {
        SelfState {
            pos: Vec3::ZERO,
            vel: Vec3::ZERO,
            eye: Vec3::new(0.0, 1.6, 0.0),
            yaw,
            pitch: 0.0,
            on_ground: true,
            health: 100.0,
            max_health: 100.0,
        }
    }

    struct Scene {
        enemies: Vec<RawEntity>,
        shots: Vec<crate::view::HeardShot>,
        yaw: f32,
    }

    impl Scene {
        fn at_angle_deg(deg: f32, dist: f32) -> Self {
            // Angle measured from the view axis (-Z), towards -X.
            let a = deg.to_radians();
            let pos = Vec3::new(-a.sin() * dist, 0.0, -a.cos() * dist);
            Scene {
                enemies: vec![RawEntity {
                    id: NetId(2),
                    pos,
                    vel: Vec3::ZERO,
                }],
                shots: Vec::new(),
                yaw: 0.0,
            }
        }

        fn raw(&self, tick: u32) -> RawState<'_> {
            RawState {
                tick: Tick(tick),
                time: f64::from(tick) * f64::from(DT),
                me: me(self.yaw),
                enemies: &self.enemies,
                shots: &self.shots,
                pickups: &[],
                objective: None,
                weapon: WeaponParams::hitscan(50.0),
            }
        }
    }

    fn open_floor() -> StaticWorld {
        StaticWorld::new(vec![Volume::cuboid(
            Vec3::new(0.0, -0.5, 0.0),
            Vec3::new(100.0, 0.5, 100.0),
        )])
    }

    /// Runs `ticks` ticks and returns the first tick at which the enemy is known.
    fn first_known(p: &mut Perception, s: &Scene, w: &StaticWorld, ticks: u32) -> Option<u32> {
        (0..ticks).find(|&t| !p.update(&s.raw(t), w).is_empty())
    }

    #[test]
    fn fov_is_110_degrees_wide() {
        let w = open_floor();
        for (deg, expect) in [(0.0, true), (50.0, true), (56.0, false), (111.0, false)] {
            let s = Scene::at_angle_deg(deg, 12.0);
            let mut p = Perception::new(PerceptionConfig::default(), 0.0, 0);
            let seen = first_known(&mut p, &s, &w, 10).is_some();
            assert_eq!(seen, expect, "target at {deg} degrees");
        }
    }

    #[test]
    fn a_wall_hides_the_target() {
        let mut w = open_floor();
        let mut vols = w.volumes().to_vec();
        vols.push(Volume::cuboid(
            Vec3::new(0.0, 2.0, -6.0),
            Vec3::new(4.0, 2.0, 0.25),
        ));
        w = StaticWorld::new(vols);
        let s = Scene::at_angle_deg(0.0, 12.0);
        let mut p = Perception::new(PerceptionConfig::default(), 0.0, 0);
        assert_eq!(first_known(&mut p, &s, &w, 30), None, "behind the wall");
        // Same target without the wall is seen.
        let mut p = Perception::new(PerceptionConfig::default(), 0.0, 0);
        assert!(first_known(&mut p, &s, &open_floor(), 30).is_some());
    }

    #[test]
    fn observation_appears_only_after_the_reaction_delay() {
        let w = open_floor();
        let s = Scene::at_angle_deg(10.0, 15.0);
        for reaction in [0.15_f32, 0.4, 0.6] {
            let mut p = Perception::new(PerceptionConfig::default(), reaction, 0);
            let tick = first_known(&mut p, &s, &w, 120).expect("seen eventually");
            let at = f64::from(tick) * f64::from(DT);
            assert!(
                at >= f64::from(reaction) - 1e-6,
                "reaction {reaction}: known at {at}"
            );
            assert!(
                at <= f64::from(reaction) + 2.0 * f64::from(DT),
                "reaction {reaction}: known at {at}, too late"
            );
        }
    }

    #[test]
    fn line_of_sight_is_refreshed_every_four_ticks() {
        let w = open_floor();
        let s = Scene::at_angle_deg(0.0, 12.0);
        // Phase 3: the first refresh is at tick 1.
        let mut p = Perception::new(PerceptionConfig::default(), 0.0, 3);
        assert_eq!(first_known(&mut p, &s, &w, 10), Some(1));
        let mut p = Perception::new(PerceptionConfig::default(), 0.0, 0);
        assert_eq!(first_known(&mut p, &s, &w, 10), Some(0));
    }

    #[test]
    fn close_shots_are_heard_without_sight() {
        let w = open_floor();
        let mut s = Scene::at_angle_deg(150.0, 20.0); // behind the bot
        s.shots.push(crate::view::HeardShot {
            shooter: NetId(2),
            pos: s.enemies[0].pos,
        });
        let mut p = Perception::new(PerceptionConfig::default(), 0.0, 0);
        let known = p.update(&s.raw(0), &w).to_vec();
        assert_eq!(known.len(), 1);
        assert!(!known[0].seen && !known[0].visible);
        // Too far: not heard.
        let mut far = Scene::at_angle_deg(150.0, 45.0);
        far.shots.push(crate::view::HeardShot {
            shooter: NetId(2),
            pos: far.enemies[0].pos,
        });
        let mut p = Perception::new(PerceptionConfig::default(), 0.0, 0);
        assert!(p.update(&far.raw(0), &w).is_empty());
    }

    #[test]
    fn the_dead_are_forgotten() {
        let w = open_floor();
        let mut s = Scene::at_angle_deg(0.0, 12.0);
        let mut p = Perception::new(PerceptionConfig::default(), 0.0, 0);
        assert!(!p.update(&s.raw(0), &w).is_empty());
        s.enemies.clear();
        assert!(p.update(&s.raw(1), &w).is_empty());
    }
}

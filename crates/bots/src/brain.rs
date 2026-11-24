//! The bot brain: utility decision, aim, path following, and the final
//! `InputCommand`.

use faset_core::glam::Vec3;
use faset_protocol::quant::{dequantize_yaw, quantize_pitch_command, quantize_yaw};
use faset_protocol::InputCommand;
use faset_sim::controller::{BUTTON_CROUCH, BUTTON_JUMP};
use faset_sim::rng::Pcg32;
use faset_sim::StaticWorld;

use crate::aim::{
    angle_diff, dir_of, pitch_of, solve_direct, solve_lob, yaw_of, AimSolution, AimTracker,
};
use crate::follow::{Drive, Follower};
use crate::level::{Level, LevelParams};
use crate::perception::{Perception, PerceptionConfig, TARGET_HEIGHT};
use crate::utility::{choose, score, Action, UtilityConfig};
use crate::view::{BotView, RawState, Sighting, BUTTON_FIRE};
use crate::NavGraph;

/// Radius of the body the bot tries to hit, metres.
const HIT_RADIUS: f32 = 0.45;
/// Newton steps of the gravity compensation and the lob prediction.
const AIM_ITERATIONS: usize = 2;
/// A sighting is extrapolated by at most this age, seconds.
const MAX_EXTRAPOLATION: f32 = 0.8;
/// Preferred fight distance as a fraction of the weapon range, bounded.
const ENGAGE_FRACTION: f32 = 0.4;
const ENGAGE_MIN: f32 = 6.0;
const ENGAGE_MAX: f32 = 18.0;
/// A patrol goal is kept at most this long, seconds.
const PATROL_TIME: f64 = 15.0;

/// Thinks for one bot. Deterministic given its seed.
#[derive(Debug, Clone)]
pub struct BotBrain {
    level: Level,
    params: LevelParams,
    cfg: UtilityConfig,
    aim: Option<AimTracker>,
    follower: Follower,
    rng: Pcg32,
    seed: u64,
    current: Option<Action>,
    patrol_goal: Option<Vec3>,
    patrol_until: f64,
    dodge_sign: f32,
    dodge_until: f64,
}

impl BotBrain {
    /// A brain of the given level; `seed` fixes every random choice (aim
    /// error, patrol goals, dodge timing, plan phase).
    #[must_use]
    pub fn new(level: Level, seed: u64) -> Self {
        Self::with_params(level, level.params(), UtilityConfig::default(), seed)
    }

    /// A brain with explicit level parameters and utility constants.
    #[must_use]
    pub fn with_params(level: Level, params: LevelParams, cfg: UtilityConfig, seed: u64) -> Self {
        let mut rng = Pcg32::new(seed, 0xB07);
        let stagger = rng.next_u32() % 30;
        Self {
            level,
            params,
            cfg,
            aim: None,
            follower: Follower::new(stagger),
            rng,
            seed,
            current: None,
            patrol_goal: None,
            patrol_until: 0.0,
            dodge_sign: 1.0,
            dodge_until: 0.0,
        }
    }

    #[must_use]
    pub fn level(&self) -> Level {
        self.level
    }

    #[must_use]
    pub fn params(&self) -> &LevelParams {
        &self.params
    }

    /// The seed given at construction.
    #[must_use]
    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// The action chosen on the last tick.
    #[must_use]
    pub fn action(&self) -> Option<Action> {
        self.current
    }

    /// Forgets plans and the view direction (after a respawn).
    pub fn reset(&mut self) {
        self.aim = None;
        self.current = None;
        self.patrol_goal = None;
        self.follower.reset();
    }

    /// One tick of thinking.
    pub fn think(&mut self, view: &BotView, dt: f32) -> InputCommand {
        let mut cmd = InputCommand {
            tick: view.tick,
            buttons: 0,
            move_x: 0,
            move_y: 0,
            yaw: quantize_yaw(view.me.yaw),
            pitch: quantize_pitch_command(view.me.pitch),
            weapon_slot: view.weapon.slot,
        };
        if view.me.health <= 0.0 {
            self.aim = None;
            self.current = None;
            return cmd;
        }
        if self.aim.is_none() {
            self.aim = Some(AimTracker::new(
                &self.params,
                self.seed,
                view.me.yaw,
                view.me.pitch,
            ));
        }

        // 1. Decide.
        let (scores, facts) = score(view, &self.cfg);
        let action = choose(&scores, self.current, &self.cfg);
        if self.current != Some(action) {
            self.current = Some(action);
        }

        // 2. Where to go, and how to look.
        let flags = self.params.moves;
        let mut goal: Option<Vec3> = None;
        let mut engage = false;
        let mut solution: Option<(AimSolution, f32)> = None;
        match action {
            Action::Attack => {
                if let Some(t) = facts.target {
                    solution = self.solve(view, &t);
                    let dist = t.pos.distance(view.me.pos);
                    let engage_dist =
                        (view.weapon.range * ENGAGE_FRACTION).clamp(ENGAGE_MIN, ENGAGE_MAX);
                    engage = true;
                    if dist > engage_dist {
                        goal = Some(t.pos);
                    }
                }
            }
            Action::Pursue => goal = facts.recent.map(|k| k.pos),
            Action::Heal => goal = facts.heal_at,
            Action::Pickup => goal = facts.pickup_at,
            Action::Objective => goal = view.objective,
            Action::Patrol => {}
        }
        if goal.is_none() && !engage {
            goal = Some(self.patrol(view));
        }

        // 3. Follow the path.
        let calm = !engage;
        let mut drive = Drive::default();
        if let Some(g) = goal {
            drive = self.follower.drive(view, g, flags, calm, dt);
            if drive.stuck || drive.unreachable {
                self.patrol_goal = None;
            }
            if drive.arrived && matches!(action, Action::Patrol | Action::Pursue) {
                self.patrol_goal = None;
            }
        }

        // 4. Wish direction (dodging while fighting).
        let mut wish = drive.dir;
        if let (true, Some(t)) = (engage, facts.target) {
            let to = Vec3::new(t.pos.x - view.me.pos.x, 0.0, t.pos.z - view.me.pos.z);
            let side = Vec3::new(-to.z, 0.0, to.x).normalize_or_zero();
            if flags.dodge {
                if view.time >= self.dodge_until {
                    self.dodge_sign = if self.rng.next_u32() & 1 == 0 {
                        1.0
                    } else {
                        -1.0
                    };
                    self.dodge_until = view.time + 0.5 + f64::from(self.rng.next_f32()) * 0.8;
                }
                let advance = if goal.is_some() { 0.6 } else { 0.0 };
                wish = (drive.dir * advance + side * self.dodge_sign).normalize_or_zero();
            }
        }

        // 5. Aim.
        let aim = self.aim.as_mut().expect("tracker created above");
        let (target_yaw, target_pitch, aiming) = match &solution {
            Some((s, _)) => (yaw_of(s.dir), pitch_of(s.dir), true),
            None if drive.dir != Vec3::ZERO => (yaw_of(drive.dir), 0.0, false),
            None => (aim.yaw(), 0.0, false),
        };
        aim.step(target_yaw, target_pitch, aiming, dt);

        // 6. Fire when the view is on target.
        let mut fire = false;
        if let Some((s, dist)) = &solution {
            let look = dir_of(aim.yaw(), aim.pitch());
            let off = look.dot(s.dir).clamp(-1.0, 1.0).acos();
            let tol = (HIT_RADIUS * self.params.fire_tolerance / dist.max(1.0)).atan();
            fire = off <= tol && angle_diff(aim.yaw(), yaw_of(s.dir)).abs() < 1.0;
        }

        // 7. Compose the command.
        let yaw_q = quantize_yaw(aim.yaw());
        let used_yaw = dequantize_yaw(yaw_q);
        let fwd = Vec3::new(-used_yaw.sin(), 0.0, -used_yaw.cos());
        let right = Vec3::new(used_yaw.cos(), 0.0, -used_yaw.sin());
        cmd.yaw = yaw_q;
        cmd.pitch = quantize_pitch_command(aim.pitch());
        cmd.move_y = (wish.dot(fwd) * 127.0).round().clamp(-127.0, 127.0) as i8;
        cmd.move_x = (wish.dot(right) * 127.0).round().clamp(-127.0, 127.0) as i8;
        if drive.jump {
            cmd.buttons |= BUTTON_JUMP;
        }
        if drive.crouch {
            cmd.buttons |= BUTTON_CROUCH;
        }
        if fire {
            cmd.buttons |= BUTTON_FIRE;
        }
        cmd
    }

    /// Aim solution and distance for a target, from its delayed sighting.
    fn solve(&self, view: &BotView, t: &Sighting) -> Option<(AimSolution, f32)> {
        let age = ((view.time - t.seen_at) as f32).clamp(0.0, MAX_EXTRAPOLATION);
        let centre = t.pos + t.vel * age + Vec3::Y * TARGET_HEIGHT;
        let d = centre - view.me.eye;
        let dist = d.length();
        let w = &view.weapon;
        let v = t.vel * self.params.lead;
        let sol = if w.projectile_speed <= 0.0 {
            AimSolution {
                dir: d.normalize_or_zero(),
                time: 0.0,
                point: d,
            }
        } else if w.lob {
            solve_lob(d, v, w.projectile_speed, w.gravity, AIM_ITERATIONS)?
        } else {
            solve_direct(d, v, w.projectile_speed, w.gravity, AIM_ITERATIONS).or_else(|| {
                // Target faster than the projectile: aim at it as it is.
                solve_direct(d, Vec3::ZERO, w.projectile_speed, w.gravity, AIM_ITERATIONS)
            })?
        };
        (sol.dir != Vec3::ZERO).then_some((sol, dist))
    }

    /// The patrol goal, drawn when none is held: half the time a spawn (where
    /// enemies reappear), otherwise any node of the graph.
    fn patrol(&mut self, view: &BotView) -> Vec3 {
        if let Some(g) = self.patrol_goal {
            if view.time < self.patrol_until {
                return g;
            }
        }
        let nav: &NavGraph = view.nav;
        let spawns = nav.spawns();
        let g = if !spawns.is_empty() && self.rng.next_u32() & 1 == 0 {
            spawns[self.rng.next_u32() as usize % spawns.len()]
        } else if nav.node_count() > 0 {
            nav.node(self.rng.next_u32() % nav.node_count() as u32).pos
        } else {
            view.me.pos
        };
        self.patrol_goal = Some(g);
        self.patrol_until = view.time + PATROL_TIME;
        g
    }
}

/// A brain with its own perception: what a server loop holds per bot.
#[derive(Debug, Clone)]
pub struct Bot {
    brain: BotBrain,
    perception: Perception,
}

impl Bot {
    /// A bot of the given level.
    #[must_use]
    pub fn new(level: Level, seed: u64) -> Self {
        Self::with_config(
            BotBrain::new(level, seed),
            PerceptionConfig::default(),
            (seed % 4) as u32,
        )
    }

    /// A bot from a brain and explicit perception limits; `los_phase` spreads
    /// the line-of-sight refresh of several bots.
    #[must_use]
    pub fn with_config(brain: BotBrain, cfg: PerceptionConfig, los_phase: u32) -> Self {
        let perception = Perception::new(cfg, brain.params().reaction_s, los_phase);
        Self { brain, perception }
    }

    #[must_use]
    pub fn brain(&self) -> &BotBrain {
        &self.brain
    }

    /// Forgets memory and plans (after a respawn).
    pub fn reset(&mut self) {
        self.brain.reset();
        self.perception.clear();
    }

    /// The map's vegetation that blocks this bot's sight (lot 6.11).
    pub fn set_sight_blockers(&mut self, sight: Option<std::sync::Arc<crate::SightBlockers>>) {
        self.perception.set_sight_blockers(sight);
    }

    /// Perceives, thinks, and returns the input of this tick.
    pub fn tick(
        &mut self,
        raw: &RawState,
        world: &StaticWorld,
        nav: &NavGraph,
        dt: f32,
    ) -> InputCommand {
        let sightings = self.perception.update(raw, world);
        let view = BotView {
            tick: raw.tick,
            time: raw.time,
            me: raw.me,
            sightings,
            pickups: raw.pickups,
            objective: raw.objective,
            weapon: raw.weapon,
            nav,
        };
        self.brain.think(&view, dt)
    }
}

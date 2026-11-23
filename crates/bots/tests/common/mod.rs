//! A small headless arena for the bot tests: the example map, the real
//! character controller of `faset-sim`, projectile shots and health, without
//! the server or the game mode (the `mode-fps` harness is private to that
//! crate, and `bots` may not depend on it).
//!
//! Gap recorded in the lot report: the `server` crate has no bot hook, so
//! bots are driven here exactly as the server loop would drive virtual
//! clients: one `InputCommand` per tick, applied with `step_player`.

#![allow(dead_code)]

use std::sync::OnceLock;

use faset_bots::{
    aim::dir_of, Bot, HeardShot, Level, NavGraph, PickupInfo, PickupKind, RawEntity, RawState,
    SelfState, WeaponParams, BUTTON_FIRE,
};
use faset_core::glam::Vec3;
use faset_core::{NetId, Tick, DT};
use faset_protocol::quant::{dequantize_pitch_command, dequantize_yaw};
use faset_sim::controller::{step_player, MoveCmd, MoveParams, PlayerState};
use faset_sim::MapCollision;

pub const MAX_HEALTH: f32 = 100.0;
pub const DAMAGE: f32 = 34.0;
pub const FIRE_INTERVAL: f32 = 0.5;
pub const PROJECTILE_SPEED: f32 = 40.0;
pub const GRAVITY: f32 = 6.0;
pub const RESPAWN_DELAY: f32 = 1.5;
/// Hit radius around the player's axis: capsule 0.35 plus the projectile.
const HIT_RADIUS: f32 = 0.5;
const PICKUP_RESPAWN: f32 = 30.0;
const PICKUP_HEAL: f32 = 50.0;

pub fn weapon() -> WeaponParams {
    WeaponParams {
        slot: 1,
        projectile_speed: PROJECTILE_SPEED,
        gravity: GRAVITY,
        range: 60.0,
        lob: false,
    }
}

pub struct Fixture {
    pub map: MapCollision,
    pub nav: NavGraph,
    pub spawns: Vec<Vec3>,
    pub health_pickup: Vec3,
    pub params: MoveParams,
}

pub fn fixture() -> &'static Fixture {
    static F: OnceLock<Fixture> = OnceLock::new();
    F.get_or_init(|| {
        let text = include_str!("../../../../assets/testdata/example.ron");
        let parsed = faset_mapfmt::parse(text.as_bytes()).unwrap();
        let map = faset_mapfmt::expand(&parsed);
        let params = MoveParams::default();
        let nav = NavGraph::build(&parsed, &params);
        let spawns = map.spawns.iter().map(|s| Vec3::from_array(s.at)).collect();
        let health_pickup = map
            .pickups
            .iter()
            .find(|p| p.kind == faset_mapfmt::PickupKind::Health)
            .map(|p| Vec3::from_array(p.at))
            .unwrap();
        Fixture {
            map: MapCollision::build(&map),
            nav,
            spawns,
            health_pickup,
            params,
        }
    })
}

pub struct Fighter {
    pub id: NetId,
    pub ps: PlayerState,
    pub bot: Bot,
    pub yaw: f32,
    pub pitch: f32,
    pub health: f32,
    pub alive: bool,
    pub respawn_at: f64,
    pub cooldown: f32,
    pub kills: u32,
    pub deaths: u32,
    pub shots: u32,
    pub hits: u32,
    pub jump_ticks: u32,
    pub crouch_ticks: u32,
}

impl Fighter {
    pub fn new(id: u16, level: Level, seed: u64, spawn: Vec3) -> Self {
        Self {
            id: NetId(id),
            ps: PlayerState::spawn(spawn + Vec3::Y * 1.0),
            bot: Bot::new(level, seed),
            yaw: 0.0,
            pitch: 0.0,
            health: MAX_HEALTH,
            alive: true,
            respawn_at: 0.0,
            cooldown: 0.0,
            kills: 0,
            deaths: 0,
            shots: 0,
            hits: 0,
            jump_ticks: 0,
            crouch_ticks: 0,
        }
    }

    pub fn me(&self) -> SelfState {
        SelfState {
            pos: self.ps.feet(),
            vel: self.ps.vel,
            eye: self.ps.eye(),
            yaw: self.yaw,
            pitch: self.pitch,
            on_ground: self.ps.on_ground,
            health: self.health,
            max_health: MAX_HEALTH,
        }
    }

    pub fn entity(&self) -> RawEntity {
        RawEntity {
            id: self.id,
            pos: self.ps.feet(),
            vel: self.ps.vel,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Projectile {
    pub owner: usize,
    pub pos: Vec3,
    pub vel: Vec3,
    pub age: f32,
}

/// Distance from `p` to the segment `a`-`b`.
fn dist_to_segment(p: Vec3, a: Vec3, b: Vec3) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-9)).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// Free-for-all arena: every fighter is hostile to all the others.
pub struct Arena {
    pub fighters: Vec<Fighter>,
    pub projectiles: Vec<Projectile>,
    pub shots: Vec<HeardShot>,
    pub tick: u32,
    pub pickup_ready_at: f64,
    pub bot_time: std::time::Duration,
    pub bot_calls: u64,
    pub time_bots: bool,
}

impl Arena {
    pub fn new(fighters: Vec<Fighter>) -> Self {
        Self {
            fighters,
            projectiles: Vec::new(),
            shots: Vec::new(),
            tick: 0,
            pickup_ready_at: 0.0,
            bot_time: std::time::Duration::ZERO,
            bot_calls: 0,
            time_bots: false,
        }
    }

    pub fn time(&self) -> f64 {
        f64::from(self.tick) * f64::from(DT)
    }

    fn respawn_point(&self, who: usize) -> Vec3 {
        let f = fixture();
        let others: Vec<Vec3> = self
            .fighters
            .iter()
            .enumerate()
            .filter(|(i, o)| *i != who && o.alive)
            .map(|(_, o)| o.ps.feet())
            .collect();
        // The spawn farthest from the nearest living opponent.
        f.spawns
            .iter()
            .copied()
            .max_by(|a, b| {
                let da = others
                    .iter()
                    .map(|o| o.distance(*a))
                    .fold(f32::MAX, f32::min);
                let db = others
                    .iter()
                    .map(|o| o.distance(*b))
                    .fold(f32::MAX, f32::min);
                da.total_cmp(&db)
            })
            .unwrap()
    }

    pub fn step(&mut self) {
        let f = fixture();
        let now = self.time();
        let heard = std::mem::take(&mut self.shots);
        let pickups = [PickupInfo {
            kind: PickupKind::Health,
            pos: f.health_pickup,
            available: now >= self.pickup_ready_at,
        }];
        let living: Vec<RawEntity> = self.fighters.iter().map(|x| x.entity()).collect();
        let mut fired: Vec<(usize, Vec3, Vec3)> = Vec::new();

        for i in 0..self.fighters.len() {
            if !self.fighters[i].alive {
                if now >= self.fighters[i].respawn_at {
                    let at = self.respawn_point(i);
                    let fi = &mut self.fighters[i];
                    fi.ps = PlayerState::spawn(at + Vec3::Y * 1.0);
                    fi.health = MAX_HEALTH;
                    fi.alive = true;
                    fi.bot.reset();
                }
                continue;
            }
            let enemies: Vec<RawEntity> = living
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != i && self.fighters[*j].alive)
                .map(|(_, e)| *e)
                .collect();
            let raw = RawState {
                tick: Tick(self.tick),
                time: now,
                me: self.fighters[i].me(),
                enemies: &enemies,
                shots: &heard,
                pickups: &pickups,
                objective: None,
                weapon: weapon(),
            };
            let t0 = self.time_bots.then(std::time::Instant::now);
            let cmd = self.fighters[i].bot.tick(&raw, f.map.world(), &f.nav, DT);
            if let Some(t0) = t0 {
                self.bot_time += t0.elapsed();
                self.bot_calls += 1;
            }
            let fi = &mut self.fighters[i];
            fi.yaw = dequantize_yaw(cmd.yaw);
            fi.pitch = dequantize_pitch_command(cmd.pitch);
            fi.jump_ticks += u32::from(cmd.buttons & faset_sim::controller::BUTTON_JUMP != 0);
            fi.crouch_ticks += u32::from(cmd.buttons & faset_sim::controller::BUTTON_CROUCH != 0);
            step_player(&mut fi.ps, MoveCmd::from(cmd), &f.map, &f.params, DT);
            fi.cooldown -= DT;
            if cmd.buttons & BUTTON_FIRE != 0 && fi.cooldown <= 0.0 {
                fi.cooldown = FIRE_INTERVAL;
                fi.shots += 1;
                let dir = dir_of(fi.yaw, fi.pitch);
                fired.push((i, fi.ps.eye() + dir * 0.5, dir * PROJECTILE_SPEED));
            }
            // Health pickup.
            let feet = fi.ps.feet();
            if now >= self.pickup_ready_at
                && (feet - f.health_pickup).length() < 1.5
                && fi.health < MAX_HEALTH
            {
                fi.health = (fi.health + PICKUP_HEAL).min(MAX_HEALTH);
                self.pickup_ready_at = now + f64::from(PICKUP_RESPAWN);
            }
        }
        for (owner, pos, vel) in fired {
            self.shots.push(HeardShot {
                shooter: self.fighters[owner].id,
                pos,
            });
            self.projectiles.push(Projectile {
                owner,
                pos,
                vel,
                age: 0.0,
            });
        }
        self.step_projectiles(now);
        self.tick += 1;
    }

    fn step_projectiles(&mut self, now: f64) {
        let f = fixture();
        let mut kept = Vec::with_capacity(self.projectiles.len());
        let projectiles = std::mem::take(&mut self.projectiles);
        'outer: for mut p in projectiles {
            p.age += DT;
            if p.age > 3.0 {
                continue;
            }
            const SUB: usize = 4;
            let dt = DT / SUB as f32;
            for _ in 0..SUB {
                p.vel.y -= GRAVITY * dt;
                let step = p.vel * dt;
                let len = step.length();
                if f.map.ray_cast(p.pos, step / len, len).is_some() {
                    continue 'outer;
                }
                let next = p.pos + step;
                for v in 0..self.fighters.len() {
                    if v == p.owner || !self.fighters[v].alive {
                        continue;
                    }
                    let feet = self.fighters[v].ps.feet();
                    let h = self.fighters[v].ps.height();
                    let (a, b) = (feet + Vec3::Y * 0.35, feet + Vec3::Y * (h - 0.35));
                    // Sample the step: it is 0.2 m long at most.
                    let hit = (0..=2).any(|k| {
                        let q = p.pos.lerp(next, k as f32 / 2.0);
                        dist_to_segment(q, a, b) < HIT_RADIUS
                    });
                    if hit {
                        self.fighters[p.owner].hits += 1;
                        let victim = &mut self.fighters[v];
                        victim.health -= DAMAGE;
                        if victim.health <= 0.0 {
                            victim.alive = false;
                            victim.deaths += 1;
                            victim.respawn_at = now + f64::from(RESPAWN_DELAY);
                            self.fighters[p.owner].kills += 1;
                        }
                        continue 'outer;
                    }
                }
                p.pos = next;
            }
            kept.push(p);
        }
        self.projectiles = kept;
    }
}

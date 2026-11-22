//! Bot trials: 16 bots play a match on a map without rendering and the run
//! is summarised in a JSON report (lot 5.5).
//!
//! The arena is the headless one of the `bots` acceptance tests (real
//! character controller of `faset-sim`, projectile shots, health, respawn),
//! made team-aware. The bots are driven exactly as a server loop would drive
//! virtual clients: one input command per tick. The run is a pure function of
//! the map, the mode and the seed: no clock, no hash iteration order, so two
//! runs give byte-identical reports.
//!
//! Decisions (lot 5.5):
//! - every bot plays at level Normal, with seed `seed * 1000 + index`;
//! - team mode splits the bots by index parity (A, B), uses the spawns of the
//!   bot's team (and `Any`), and has no friendly fire; free-for-all makes all
//!   bots hostile to each other and uses every spawn;
//! - weapon: one projectile weapon (34 damage, 0.5 s, 40 m/s), 100 health,
//!   respawn after 1.5 s at the spawn farthest from the nearest living enemy;
//!   only Health pickups are modelled (+50, delay from the map);
//! - "engagement distance" is the killer to victim distance at the killing hit;
//! - "death rate at spawn" is the fraction of deaths within 6 m of a spawn;
//! - grids: deaths in 8 m cells; "visited" is judged on 4 m cells holding at
//!   least one navigation node; a cell is visited when a living bot stood in
//!   it; "time per team" is the summed living time of the team's bots;
//! - the trial lasts 300 s of match time (18000 ticks), simulated as fast as
//!   the machine allows.

use std::collections::{BTreeMap, BTreeSet};

use faset_bots::{
    aim::dir_of, Bot, HeardShot, Level, NavGraph, PickupInfo, PickupKind, RawEntity, RawState,
    SelfState, WeaponParams, BUTTON_FIRE,
};
use faset_core::glam::Vec3;
use faset_core::{NetId, Tick, DT};
use faset_mapfmt::{MapFile, Team};
use faset_protocol::quant::{dequantize_pitch_command, dequantize_yaw};
use faset_sim::controller::{step_player, MoveCmd, MoveParams, PlayerState};
use faset_sim::MapCollision;
use serde::Serialize;

/// Version of the trial report schema.
pub const TRIAL_SCHEMA_VERSION: u32 = 1;
/// Bots in a trial.
pub const BOTS: usize = 16;
/// Match time of a trial, seconds.
pub const TRIAL_SECONDS: f64 = 300.0;

const MAX_HEALTH: f32 = 100.0;
const DAMAGE: f32 = 34.0;
const FIRE_INTERVAL: f32 = 0.5;
const PROJECTILE_SPEED: f32 = 40.0;
const GRAVITY: f32 = 6.0;
const RESPAWN_DELAY: f64 = 1.5;
const HIT_RADIUS: f32 = 0.5;
const PICKUP_HEAL: f32 = 50.0;
const DEATH_CELL_M: f32 = 8.0;
const VISIT_CELL_M: f32 = 4.0;
const SPAWN_RADIUS_M: f32 = 6.0;

/// Game mode of a trial.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TrialMode {
    /// Two teams, no friendly fire.
    Team,
    /// Everybody against everybody.
    Ffa,
}

/// Inputs of a trial.
#[derive(Debug, Clone, Copy)]
pub struct TrialConfig {
    pub seed: u64,
    pub mode: TrialMode,
    /// Match time to simulate, seconds.
    pub seconds: f64,
}

impl TrialConfig {
    /// A full trial: 5 minutes, team mode.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            mode: TrialMode::Team,
            seconds: TRIAL_SECONDS,
        }
    }
}

/// A box on the ground plane with a death count.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ZoneCount {
    /// `[x, z]` of the minimum corner, metres.
    pub min: [f32; 2],
    /// `[x, z]` of the maximum corner, metres.
    pub max: [f32; 2],
    pub deaths: u32,
}

/// An area no bot walked into.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UnvisitedZone {
    pub min: [f32; 2],
    pub max: [f32; 2],
}

/// Deaths close to the spawn points.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SpawnDeaths {
    pub radius_m: f32,
    pub deaths_near_spawn: u32,
    pub total_deaths: u32,
    /// `deaths_near_spawn / total_deaths`, 0 without deaths.
    pub rate: f32,
}

/// Kills and time of one team (in free-for-all, teams A and B are the bots
/// of even and odd index).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TeamStats {
    pub team: &'static str,
    pub kills: u32,
    pub deaths: u32,
    /// Summed living time of the team's bots, seconds.
    pub alive_seconds: f32,
}

/// The JSON report of a trial. Field names are fixed (schema 1).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TrialReport {
    pub schema_version: u32,
    pub map_name: String,
    pub mode: TrialMode,
    pub seed: u64,
    pub bots: u32,
    pub match_seconds: f64,
    pub total_kills: u32,
    /// Width of the cells of `elimination_zones`, metres.
    pub death_cell_m: f32,
    /// Where deaths happen, most deadly first.
    pub elimination_zones: Vec<ZoneCount>,
    pub spawn_deaths: SpawnDeaths,
    /// Mean killer to victim distance at the killing hit, metres (0 without kills).
    pub mean_engagement_distance_m: f32,
    /// Width of the cells of `unvisited_zones`, metres.
    pub visit_cell_m: f32,
    /// Cells with a walkable floor that no bot entered.
    pub unvisited_zones: Vec<UnvisitedZone>,
    pub team_balance: [TeamStats; 2],
}

impl TrialReport {
    /// Pretty JSON with a trailing newline.
    #[must_use]
    pub fn to_json(&self) -> String {
        let mut s = serde_json::to_string_pretty(self).unwrap_or_default();
        s.push('\n');
        s
    }
}

struct Fighter {
    id: NetId,
    team: usize,
    ps: PlayerState,
    bot: Bot,
    yaw: f32,
    pitch: f32,
    health: f32,
    alive: bool,
    respawn_at: f64,
    cooldown: f32,
}

struct Projectile {
    owner: usize,
    pos: Vec3,
    vel: Vec3,
    age: f32,
}

fn dist_to_segment(p: Vec3, a: Vec3, b: Vec3) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-9)).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

fn cell_of(x: f32, z: f32, size: f32) -> [i32; 2] {
    [(x / size).floor() as i32, (z / size).floor() as i32]
}

fn cell_box(c: [i32; 2], size: f32) -> ([f32; 2], [f32; 2]) {
    let min = [c[0] as f32 * size, c[1] as f32 * size];
    (min, [min[0] + size, min[1] + size])
}

fn round3(v: f32) -> f32 {
    (v * 1000.0).round() / 1000.0
}

fn hostile(mode: TrialMode, a: usize, b: usize) -> bool {
    mode == TrialMode::Ffa || a != b
}

/// Plays a trial of `map` and returns its report.
#[must_use]
pub fn run_trial(map: &MapFile, cfg: &TrialConfig) -> TrialReport {
    let map = faset_mapfmt::expand(map);
    let params = MoveParams::default();
    let nav = NavGraph::build(&map, &params);
    let collision = MapCollision::build(&map);

    let all_spawns: Vec<(Vec3, Team)> = map
        .spawns
        .iter()
        .map(|s| (Vec3::from_array(s.at), s.team))
        .collect();
    let spawn_spots: Vec<Vec3> = all_spawns.iter().map(|(p, _)| *p).collect();
    let spawns_of = |team: usize| -> Vec<Vec3> {
        if cfg.mode == TrialMode::Ffa {
            return spawn_spots.clone();
        }
        let wanted = if team == 0 { Team::A } else { Team::B };
        let own: Vec<Vec3> = all_spawns
            .iter()
            .filter(|(_, t)| *t == wanted || *t == Team::Any)
            .map(|(p, _)| *p)
            .collect();
        if own.is_empty() {
            spawn_spots.clone()
        } else {
            own
        }
    };
    let team_spawns = [spawns_of(0), spawns_of(1)];

    let health_pickups: Vec<(Vec3, f64)> = map
        .pickups
        .iter()
        .filter(|p| p.kind == faset_mapfmt::PickupKind::Health)
        .map(|p| (Vec3::from_array(p.at), f64::from(p.respawn_s)))
        .collect();
    let mut pickup_ready = vec![0.0f64; health_pickups.len()];

    let mut fighters: Vec<Fighter> = (0..BOTS)
        .map(|i| {
            let team = i % 2;
            let pool = &team_spawns[team];
            let at = if pool.is_empty() {
                Vec3::ZERO
            } else {
                pool[i / 2 % pool.len()]
            };
            Fighter {
                id: NetId(i as u16 + 1),
                team,
                ps: PlayerState::spawn(at + Vec3::Y),
                bot: Bot::new(
                    Level::Normal,
                    cfg.seed.wrapping_mul(1000).wrapping_add(i as u64),
                ),
                yaw: 0.0,
                pitch: 0.0,
                health: MAX_HEALTH,
                alive: true,
                respawn_at: 0.0,
                cooldown: 0.0,
            }
        })
        .collect();

    let walkable: BTreeSet<[i32; 2]> = nav
        .nodes()
        .iter()
        .map(|n| cell_of(n.pos.x, n.pos.z, VISIT_CELL_M))
        .collect();
    let mut visited: BTreeSet<[i32; 2]> = BTreeSet::new();
    let mut death_cells: BTreeMap<[i32; 2], u32> = BTreeMap::new();
    let mut deaths_near_spawn = 0u32;
    let mut total_deaths = 0u32;
    let mut engagement_sum = 0.0f64;
    let mut team_kills = [0u32; 2];
    let mut team_deaths = [0u32; 2];
    let mut team_alive_ticks = [0u64; 2];
    let weapon = WeaponParams {
        slot: 1,
        projectile_speed: PROJECTILE_SPEED,
        gravity: GRAVITY,
        range: 60.0,
        lob: false,
    };

    let ticks = (cfg.seconds / f64::from(DT)).round() as u32;
    let mut shots: Vec<HeardShot> = Vec::new();
    let mut projectiles: Vec<Projectile> = Vec::new();
    for tick in 0..ticks {
        let now = f64::from(tick) * f64::from(DT);
        let heard = std::mem::take(&mut shots);
        let pickups: Vec<PickupInfo> = health_pickups
            .iter()
            .zip(&pickup_ready)
            .map(|((pos, _), ready)| PickupInfo {
                kind: PickupKind::Health,
                pos: *pos,
                available: now >= *ready,
            })
            .collect();
        let snapshot: Vec<(RawEntity, usize, bool)> = fighters
            .iter()
            .map(|f| {
                (
                    RawEntity {
                        id: f.id,
                        pos: f.ps.feet(),
                        vel: f.ps.vel,
                    },
                    f.team,
                    f.alive,
                )
            })
            .collect();
        let mut fired: Vec<(usize, Vec3, Vec3)> = Vec::new();

        #[allow(clippy::needless_range_loop)] // the body also reads other fighters
        for i in 0..fighters.len() {
            let team = fighters[i].team;
            let hostiles = || {
                snapshot
                    .iter()
                    .enumerate()
                    .filter(move |(j, (_, t, a))| *j != i && *a && hostile(cfg.mode, team, *t))
                    .map(|(_, (e, _, _))| *e)
            };
            if !fighters[i].alive {
                if now >= fighters[i].respawn_at {
                    let enemies: Vec<Vec3> = hostiles().map(|e| e.pos).collect();
                    let at = team_spawns[team]
                        .iter()
                        .copied()
                        .max_by(|a, b| {
                            let d = |p: &Vec3| {
                                enemies
                                    .iter()
                                    .map(|e| e.distance(*p))
                                    .fold(f32::MAX, f32::min)
                            };
                            d(a).total_cmp(&d(b))
                        })
                        .unwrap_or(Vec3::ZERO);
                    let f = &mut fighters[i];
                    f.ps = PlayerState::spawn(at + Vec3::Y);
                    f.health = MAX_HEALTH;
                    f.alive = true;
                    f.bot.reset();
                }
                continue;
            }
            team_alive_ticks[team] += 1;
            let enemies: Vec<RawEntity> = hostiles().collect();
            let f = &mut fighters[i];
            let raw = RawState {
                tick: Tick(tick),
                time: now,
                me: SelfState {
                    pos: f.ps.feet(),
                    vel: f.ps.vel,
                    eye: f.ps.eye(),
                    yaw: f.yaw,
                    pitch: f.pitch,
                    on_ground: f.ps.on_ground,
                    health: f.health,
                    max_health: MAX_HEALTH,
                },
                enemies: &enemies,
                shots: &heard,
                pickups: &pickups,
                objective: None,
                weapon,
            };
            let cmd = f.bot.tick(&raw, collision.world(), &nav, DT);
            f.yaw = dequantize_yaw(cmd.yaw);
            f.pitch = dequantize_pitch_command(cmd.pitch);
            step_player(&mut f.ps, MoveCmd::from(cmd), &collision, &params, DT);
            let feet = f.ps.feet();
            visited.insert(cell_of(feet.x, feet.z, VISIT_CELL_M));
            f.cooldown -= DT;
            if cmd.buttons & BUTTON_FIRE != 0 && f.cooldown <= 0.0 {
                f.cooldown = FIRE_INTERVAL;
                let dir = dir_of(f.yaw, f.pitch);
                fired.push((i, f.ps.eye() + dir * 0.5, dir * PROJECTILE_SPEED));
            }
            if f.health < MAX_HEALTH {
                for (k, (pos, delay)) in health_pickups.iter().enumerate() {
                    if now >= pickup_ready[k] && (feet - *pos).length() < 1.5 {
                        f.health = (f.health + PICKUP_HEAL).min(MAX_HEALTH);
                        pickup_ready[k] = now + delay;
                        break;
                    }
                }
            }
        }
        for (owner, pos, vel) in fired {
            shots.push(HeardShot {
                shooter: fighters[owner].id,
                pos,
            });
            projectiles.push(Projectile {
                owner,
                pos,
                vel,
                age: 0.0,
            });
        }

        let mut kept = Vec::with_capacity(projectiles.len());
        'outer: for mut p in std::mem::take(&mut projectiles) {
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
                if collision.ray_cast(p.pos, step / len, len).is_some() {
                    continue 'outer;
                }
                let next = p.pos + step;
                for v in 0..fighters.len() {
                    if v == p.owner
                        || !fighters[v].alive
                        || !hostile(cfg.mode, fighters[p.owner].team, fighters[v].team)
                    {
                        continue;
                    }
                    let feet = fighters[v].ps.feet();
                    let h = fighters[v].ps.height();
                    let (a, b) = (feet + Vec3::Y * 0.35, feet + Vec3::Y * (h - 0.35));
                    let hit = (0..=2).any(|k| {
                        let q = p.pos.lerp(next, k as f32 / 2.0);
                        dist_to_segment(q, a, b) < HIT_RADIUS
                    });
                    if hit {
                        let killer_pos = fighters[p.owner].ps.feet();
                        let killer_team = fighters[p.owner].team;
                        let victim = &mut fighters[v];
                        victim.health -= DAMAGE;
                        if victim.health <= 0.0 {
                            victim.alive = false;
                            victim.respawn_at = now + RESPAWN_DELAY;
                            let vpos = victim.ps.feet();
                            total_deaths += 1;
                            team_deaths[victim.team] += 1;
                            team_kills[killer_team] += 1;
                            engagement_sum += f64::from(killer_pos.distance(vpos));
                            *death_cells
                                .entry(cell_of(vpos.x, vpos.z, DEATH_CELL_M))
                                .or_insert(0) += 1;
                            if spawn_spots
                                .iter()
                                .any(|s| s.distance(vpos) <= SPAWN_RADIUS_M)
                            {
                                deaths_near_spawn += 1;
                            }
                        }
                        continue 'outer;
                    }
                }
                p.pos = next;
            }
            kept.push(p);
        }
        projectiles = kept;
    }

    let mut elimination_zones: Vec<ZoneCount> = death_cells
        .iter()
        .map(|(c, n)| {
            let (min, max) = cell_box(*c, DEATH_CELL_M);
            ZoneCount {
                min,
                max,
                deaths: *n,
            }
        })
        .collect();
    // Stable sort: ties keep the cell order of the map.
    elimination_zones.sort_by_key(|z| std::cmp::Reverse(z.deaths));
    let unvisited_zones = walkable
        .difference(&visited)
        .map(|c| {
            let (min, max) = cell_box(*c, VISIT_CELL_M);
            UnvisitedZone { min, max }
        })
        .collect();
    let rate = if total_deaths == 0 {
        0.0
    } else {
        deaths_near_spawn as f32 / total_deaths as f32
    };
    let team_stats = |t: usize, name: &'static str| TeamStats {
        team: name,
        kills: team_kills[t],
        deaths: team_deaths[t],
        alive_seconds: round3(team_alive_ticks[t] as f32 * DT),
    };
    TrialReport {
        schema_version: TRIAL_SCHEMA_VERSION,
        map_name: map.meta.name.clone(),
        mode: cfg.mode,
        seed: cfg.seed,
        bots: BOTS as u32,
        match_seconds: cfg.seconds,
        total_kills: total_deaths,
        death_cell_m: DEATH_CELL_M,
        elimination_zones,
        spawn_deaths: SpawnDeaths {
            radius_m: SPAWN_RADIUS_M,
            deaths_near_spawn,
            total_deaths,
            rate: round3(rate),
        },
        mean_engagement_distance_m: if total_deaths == 0 {
            0.0
        } else {
            round3((engagement_sum / f64::from(total_deaths)) as f32)
        },
        visit_cell_m: VISIT_CELL_M,
        unvisited_zones,
        team_balance: [team_stats(0, "A"), team_stats(1, "B")],
    }
}

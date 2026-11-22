//! The shot pipeline: weapon state, cadence, spread, recoil and the cast.
//!
//! [`fire`] is a pure function of its arguments: time, seed, shot counter
//! (inside [`WeaponState`]), view basis and a [`RayCaster`]. The client calls
//! it for prediction with a [`SceneCaster`] over the entities it displays;
//! the server calls it for validation with a [`RewindCaster`] or
//! [`ServerCaster`], which place the other players where the shooter saw
//! them (lot 1.7). Both advance the weapon with [`WeaponState::tick`] once
//! per simulation tick.
//!
//! Decisions:
//! * Cadence: `t_next <- t_next + 60 / rpm`, except that a shot more than one
//!   tick later than `t_next` restarts from `now` (`t_next <- now + 60 / rpm`).
//!   A plain `t_next + interval` would let a player who paused fire several
//!   shots at once; within one tick of lateness the schedule is kept so the
//!   average rate stays exact at any rpm.
//! * A shot is accepted up to [`CADENCE_TOLERANCE`] early (float rounding).
//! * Shotgun damage values are per pellet.
//! * The bloom `beta` is capped at `sigma_max` so it recovers in bounded
//!   time.
//! * The random draws of one shot come from one PCG stream in this order:
//!   recoil yaw, recoil pitch, then `(u1, u2)` of pellet 0, 1, ...
//! * The recoil pattern index restarts [`SPRAY_RESET`] seconds after the
//!   last shot.

use std::collections::BTreeMap;

use faset_core::glam::Vec3;
use faset_core::{NetId, Tick, DT};
use faset_server::{BodyPart, FireCommand, Hitbox, LagCompensation, Server, HEAD_RADIUS};
use faset_sim::collision::player_capsule;
use faset_sim::rng::Pcg32;
use faset_sim::StaticWorld;
use parry3d::math::{Pose, Vector};
use parry3d::query::{Ray, RayCast};

use crate::damage::{pellet_damage, DamageEvent, DamageType, HitZone};
use crate::recoil::RecoilState;
use crate::spread::{cone_sigma, decay_beta, draw_pair, spread_direction, state_multiplier};
use crate::spread::{AimBasis, Stance};
use crate::weapon::{FireMode, WeaponDef, WeaponKind};

/// How early a shot may arrive and still meet the cadence, seconds.
pub const CADENCE_TOLERANCE: f64 = 1e-4;
/// Pause after which the recoil pattern starts again, seconds.
pub const SPRAY_RESET: f64 = 0.4;

/// Ray-sphere intersection: smallest `t >= 0` along the unit direction `d`
/// from `o`. With `m = o - c`, `b = m.d`, `q = |m|^2 - r^2`: a hit needs
/// `b^2 >= q` and then `t = -b - sqrt(b^2 - q)`. An origin inside the sphere
/// hits at 0.
#[must_use]
pub fn ray_sphere(o: Vec3, d: Vec3, c: Vec3, r: f32) -> Option<f32> {
    let m = o - c;
    let b = m.dot(d);
    let q = m.length_squared() - r * r;
    if b * b < q {
        return None;
    }
    let t = -b - (b * b - q).sqrt();
    if t >= 0.0 {
        Some(t)
    } else if q < 0.0 {
        Some(0.0)
    } else {
        None
    }
}

/// The first player a ray touched.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayerContact {
    pub target: NetId,
    pub zone: HitZone,
    /// From the ray origin, metres.
    pub distance: f32,
    pub point: Vec3,
}

/// Something that finds what a ray hits among the players: the part of the
/// world that differs between client prediction and server validation.
pub trait RayCaster {
    /// First player contact of the ray, if the map does not stop it first.
    /// Server casters ignore `origin` and use the shooter's authoritative
    /// eye.
    fn cast(&mut self, origin: Vec3, dir: Vec3, range: f32) -> Option<PlayerContact>;
}

fn v3(v: Vec3) -> Vector {
    Vector::new(v.x, v.y, v.z)
}

fn at(p: Vec3) -> Pose {
    Pose::translation(p.x, p.y, p.z)
}

/// Casts against hitboxes given as they are (no rewind): the client's view
/// of the scene, and the reference for the tests. Each player's head is
/// tested before its body; the nearest contact before the map wins, ties
/// going to the lowest id.
#[derive(Clone, Copy)]
pub struct SceneCaster<'a> {
    pub statics: &'a StaticWorld,
    pub shooter: NetId,
    pub targets: &'a [(NetId, Hitbox)],
}

impl RayCaster for SceneCaster<'_> {
    fn cast(&mut self, origin: Vec3, dir: Vec3, range: f32) -> Option<PlayerContact> {
        let stop = self
            .statics
            .ray_cast(origin, dir, range)
            .map_or(range, |h| h.distance);
        let ray = Ray::new(v3(origin), v3(dir));
        let mut sorted: Vec<&(NetId, Hitbox)> = self.targets.iter().collect();
        sorted.sort_by_key(|(id, _)| *id);
        let mut best: Option<PlayerContact> = None;
        for (id, hb) in sorted {
            if *id == self.shooter {
                continue;
            }
            // The head sits inside the top of the body capsule: a ray
            // through it is a head hit even though it enters the body first.
            let contact = ray_sphere(origin, dir, hb.head_center(), HEAD_RADIUS)
                .map(|d| (HitZone::Head, d))
                .or_else(|| {
                    player_capsule(hb.body_height())
                        .cast_ray(&at(hb.body_center()), &ray, stop, true)
                        .map(|d| (HitZone::Body, d))
                });
            let Some((zone, distance)) = contact else {
                continue;
            };
            if distance < stop && best.is_none_or(|b| distance < b.distance) {
                best = Some(PlayerContact {
                    target: *id,
                    zone,
                    distance,
                    point: origin + dir * distance,
                });
            }
        }
        best
    }
}

fn zone_of(part: BodyPart) -> HitZone {
    match part {
        BodyPart::Head => HitZone::Head,
        BodyPart::Body => HitZone::Body,
    }
}

fn to_contact(hit: faset_server::Hit) -> PlayerContact {
    PlayerContact {
        target: hit.target,
        zone: zone_of(hit.part),
        distance: hit.distance,
        point: hit.point,
    }
}

/// Server validation over a [`LagCompensation`] history: the other players
/// are placed at the instant the shooter saw (`view_tick` plus
/// `view_fraction`), within the rewind limits of lot 1.7.
pub struct RewindCaster<'a> {
    pub lag: &'a mut LagCompensation,
    pub statics: &'a StaticWorld,
    pub shooter: NetId,
    /// Measured round trip of the shooter, seconds.
    pub rtt: f32,
    pub view_tick: Tick,
    pub view_fraction: f32,
}

impl RayCaster for RewindCaster<'_> {
    fn cast(&mut self, _origin: Vec3, dir: Vec3, range: f32) -> Option<PlayerContact> {
        let fire = FireCommand {
            view_tick: self.view_tick,
            view_fraction: self.view_fraction,
            direction: dir,
            range,
        };
        self.lag
            .rewind_and_cast(self.statics, self.shooter, &fire, self.rtt)?
            .hit
            .map(to_contact)
    }
}

/// Server validation through [`Server::rewind_and_cast`], which uses the
/// shooter's measured round trip.
pub struct ServerCaster<'a> {
    pub server: &'a mut Server,
    pub shooter: NetId,
    pub view_tick: Tick,
    pub view_fraction: f32,
}

impl RayCaster for ServerCaster<'_> {
    fn cast(&mut self, _origin: Vec3, dir: Vec3, range: f32) -> Option<PlayerContact> {
        let fire = FireCommand {
            view_tick: self.view_tick,
            view_fraction: self.view_fraction,
            direction: dir,
            range,
        };
        self.server
            .rewind_and_cast(self.shooter, &fire)?
            .hit
            .map(to_contact)
    }
}

/// Everything about a weapon that changes while it is carried.
#[derive(Debug, Clone, PartialEq)]
pub struct WeaponState {
    /// Rounds in the magazine.
    pub ammo: u32,
    /// Rounds outside the magazine.
    pub reserve: u32,
    /// t_next: earliest time of the next shot.
    pub next_fire: f64,
    /// Bloom added to the cone.
    pub beta: f32,
    /// Shots fired; seeds the next shot's generator.
    pub shot_counter: u32,
    /// End of the running reload.
    pub reload_end: Option<f64>,
    /// Shots left in the current burst.
    pub burst_left: u32,
    pub recoil: RecoilState,
    last_shot: f64,
    spray_index: usize,
}

impl WeaponState {
    /// A full weapon, drawn at `now` (it can fire once the draw time is
    /// over).
    #[must_use]
    pub fn new(def: &WeaponDef, now: f64) -> Self {
        Self {
            ammo: def.magazine,
            reserve: def.reserve,
            next_fire: now + f64::from(def.draw_time),
            beta: 0.0,
            shot_counter: 0,
            reload_end: None,
            burst_left: 0,
            recoil: RecoilState::default(),
            last_shot: f64::NEG_INFINITY,
            spray_index: 0,
        }
    }

    /// Whether a reload is running.
    #[must_use]
    pub fn reloading(&self) -> bool {
        self.reload_end.is_some()
    }

    /// Starts a reload at `now`. `false` when the magazine is full, there is
    /// no reserve, or one already runs.
    pub fn start_reload(&mut self, def: &WeaponDef, now: f64) -> bool {
        if self.reloading() || self.ammo >= def.magazine || self.reserve == 0 {
            return false;
        }
        self.reload_end = Some(now + f64::from(def.reload_time));
        self.burst_left = 0;
        true
    }

    /// One simulation tick of `dt` seconds ending at `now`: finishes a due
    /// reload, decays the bloom and runs the recoil spring.
    pub fn tick(&mut self, def: &WeaponDef, now: f64, dt: f32) {
        self.settle(def, now);
        self.beta = decay_beta(self.beta, dt, def.spread.tau);
        self.recoil.step(def.recoil.omega, dt);
    }

    fn settle(&mut self, def: &WeaponDef, now: f64) {
        if self.reload_end.is_some_and(|end| now >= end) {
            let take = (def.magazine - self.ammo).min(self.reserve);
            self.ammo += take;
            self.reserve -= take;
            self.reload_end = None;
        }
    }

    /// Cone half angle right now for `stance`.
    #[must_use]
    pub fn sigma(&self, def: &WeaponDef, stance: Stance) -> f32 {
        cone_sigma(
            &def.spread,
            state_multiplier(&def.spread, &def.sight, stance),
            self.beta,
        )
    }
}

/// What the shooter did and where it was.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShotInput {
    /// Time of the shot, seconds (tick time).
    pub now: f64,
    /// Server seed of the match.
    pub seed: u64,
    pub shooter: NetId,
    /// Eye position (used by client-side casters).
    pub origin: Vec3,
    /// Camera basis, recoil included.
    pub basis: AimBasis,
    pub stance: Stance,
    /// The trigger went down since the last call (needed by `Semi` and to
    /// start a `Burst`).
    pub fresh_press: bool,
}

/// Why a shot did not happen. The weapon state is untouched.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ShotReject {
    /// The weapon is not hitscan.
    NotHitscan,
    /// The weapon is not a projectile weapon.
    NotProjectile,
    /// The weapon is not a melee weapon.
    NotMelee,
    /// Still reloading.
    Reloading,
    /// The trigger must be pressed again.
    NeedsFreshPress,
    /// Magazine empty.
    Empty,
    /// Earlier than the cadence allows (or the weapon is still being drawn);
    /// seconds to wait.
    Cadence { wait: f64 },
}

/// One pellet of a shot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PelletReport {
    pub direction: Vec3,
    pub contact: Option<PlayerContact>,
}

/// What a shot did.
#[derive(Debug, Clone, PartialEq)]
pub struct ShotReport {
    /// Counter the generator was seeded with.
    pub shot_index: u32,
    /// Cone half angle used.
    pub sigma: f32,
    pub pellets: Vec<PelletReport>,
    /// Damage summed per victim, ascending victim id. The zone is `Head` if
    /// any pellet hit the head, the position is the first contact.
    pub damage: Vec<DamageEvent>,
    /// View kick added to the shooter's recoil, `(yaw, pitch)`.
    pub recoil_kick: (f32, f32),
}

/// Checks the trigger rules and commits cadence, ammo and burst for one
/// shot of any weapon kind; returns the shot counter and the generator
/// seeded with it. `ammo` is false for melee, which never runs dry.
pub(crate) fn begin_shot(
    def: &WeaponDef,
    weapon: &mut WeaponState,
    input: &ShotInput,
    ammo: bool,
) -> Result<(u32, Pcg32), ShotReject> {
    let now = input.now;
    weapon.settle(def, now);
    if weapon.reloading() {
        return Err(ShotReject::Reloading);
    }
    let needs_press = match def.fire_mode {
        FireMode::Auto => false,
        FireMode::Semi => true,
        FireMode::Burst => weapon.burst_left == 0,
    };
    if needs_press && !input.fresh_press {
        return Err(ShotReject::NeedsFreshPress);
    }
    if ammo && weapon.ammo == 0 {
        return Err(ShotReject::Empty);
    }
    if now + CADENCE_TOLERANCE < weapon.next_fire {
        return Err(ShotReject::Cadence {
            wait: weapon.next_fire - now,
        });
    }

    // Commit: cadence, ammo, burst.
    let base = if now - weapon.next_fire > f64::from(DT) {
        now
    } else {
        weapon.next_fire
    };
    weapon.next_fire = base + def.interval();
    if ammo {
        weapon.ammo -= 1;
    }
    weapon.burst_left = match def.fire_mode {
        FireMode::Burst if weapon.burst_left == 0 => def.burst_count - 1,
        FireMode::Burst => weapon.burst_left - 1,
        _ => 0,
    };
    if now - weapon.last_shot > SPRAY_RESET {
        weapon.spray_index = 0;
    }
    let shot_index = weapon.shot_counter;
    weapon.shot_counter = weapon.shot_counter.wrapping_add(1);
    Ok((
        shot_index,
        Pcg32::for_shot(input.seed, input.shooter, shot_index),
    ))
}

/// Bloom, recoil kick and spray bookkeeping after a committed shot.
pub(crate) fn end_shot(
    def: &WeaponDef,
    weapon: &mut WeaponState,
    now: f64,
    u_yaw: f32,
    u_pitch: f32,
) -> (f32, f32) {
    weapon.beta = (weapon.beta + def.spread.delta_beta).min(def.spread.sigma_max);
    let kick = weapon
        .recoil
        .kick(&def.recoil, weapon.spray_index, u_yaw, u_pitch);
    weapon.spray_index += 1;
    weapon.last_shot = now;
    kick
}

/// Fires `weapon` once if the rules allow it.
///
/// # Errors
/// [`ShotReject`] when the weapon cannot fire at `input.now`.
pub fn fire(
    def: &WeaponDef,
    weapon: &mut WeaponState,
    input: &ShotInput,
    caster: &mut dyn RayCaster,
) -> Result<ShotReport, ShotReject> {
    if def.kind != WeaponKind::Hitscan {
        return Err(ShotReject::NotHitscan);
    }
    let now = input.now;
    let (shot_index, mut rng) = begin_shot(def, weapon, input, true)?;
    let (u_yaw, u_pitch) = draw_pair(&mut rng);
    let sigma = weapon.sigma(def, input.stance);
    let mut pellets = Vec::with_capacity(def.pellets as usize);
    let mut by_victim: BTreeMap<NetId, DamageEvent> = BTreeMap::new();
    for _ in 0..def.pellets {
        let (u1, u2) = draw_pair(&mut rng);
        let direction = spread_direction(&input.basis, sigma, u1, u2);
        let contact = caster.cast(input.origin, direction, def.range);
        if let Some(c) = contact {
            let amount = pellet_damage(&def.damage, c.distance, c.zone);
            by_victim
                .entry(c.target)
                .and_modify(|e| {
                    e.amount += amount;
                    if c.zone == HitZone::Head {
                        e.zone = HitZone::Head;
                    }
                })
                .or_insert(DamageEvent {
                    attacker: input.shooter,
                    victim: c.target,
                    amount,
                    kind: DamageType::Bullet,
                    zone: c.zone,
                    position: c.point,
                });
        }
        pellets.push(PelletReport { direction, contact });
    }

    let recoil_kick = end_shot(def, weapon, now, u_yaw, u_pitch);

    Ok(ShotReport {
        shot_index,
        sigma,
        pellets,
        damage: by_victim.into_values().collect(),
        recoil_kick,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use faset_server::HISTORY_TICKS;
    use faset_sim::Volume;

    const SHOOTER: NetId = NetId(1);
    const TARGET: NetId = NetId(2);

    fn rifle() -> WeaponDef {
        WeaponDef::builtin().remove(0)
    }

    fn precise(mut def: WeaponDef) -> WeaponDef {
        def.spread.sigma_base = 0.0;
        def.spread.delta_beta = 0.0;
        def.spread.sigma_max = 0.0;
        def
    }

    fn input(now: f64, basis: AimBasis) -> ShotInput {
        ShotInput {
            now,
            seed: 7,
            shooter: SHOOTER,
            origin: Vec3::new(0.0, 1.6, 0.0),
            basis,
            stance: Stance::default(),
            fresh_press: true,
        }
    }

    fn forward() -> AimBasis {
        AimBasis::from_quantized(0, 0)
    }

    /// A player standing 10 m ahead (-Z) of the origin, feet on y = 0.
    fn ahead(id: u16, z: f32) -> (NetId, Hitbox) {
        (NetId(id), Hitbox::standing(Vec3::new(0.0, 0.9, z)))
    }

    #[test]
    fn ray_sphere_hit_miss_and_tangent() {
        let o = Vec3::ZERO;
        let d = Vec3::X;
        // Hit: front of a unit sphere centred 5 m away.
        let t = ray_sphere(o, d, Vec3::new(5.0, 0.0, 0.0), 1.0).unwrap();
        assert!((t - 4.0).abs() < 1e-6);
        // Miss: passes 2 m beside it.
        assert!(ray_sphere(o, d, Vec3::new(5.0, 2.0, 0.0), 1.0).is_none());
        // Tangent: b^2 == q exactly, touches at the middle.
        let t = ray_sphere(o, d, Vec3::new(5.0, 1.0, 0.0), 1.0).unwrap();
        assert_eq!(t, 5.0);
        // Behind the origin.
        assert!(ray_sphere(o, d, Vec3::new(-5.0, 0.0, 0.0), 1.0).is_none());
        // Inside the sphere.
        assert_eq!(ray_sphere(o, d, Vec3::new(0.5, 0.0, 0.0), 1.0), Some(0.0));
    }

    #[test]
    fn head_is_tested_before_body() {
        let statics = StaticWorld::new(vec![]);
        let targets = [ahead(2, -10.0)];
        let mut c = SceneCaster {
            statics: &statics,
            shooter: SHOOTER,
            targets: &targets,
        };
        // Head centre is 1.6 m above the feet: a level ray at eye height.
        let head = c
            .cast(Vec3::new(0.0, 1.6, 0.0), Vec3::NEG_Z, 100.0)
            .unwrap();
        assert_eq!(head.zone, HitZone::Head);
        // A ray 0.5 m lower misses the head and hits the body.
        let body = c
            .cast(Vec3::new(0.0, 1.0, 0.0), Vec3::NEG_Z, 100.0)
            .unwrap();
        assert_eq!(body.zone, HitZone::Body);
        assert!((body.distance - 9.65).abs() < 0.01);
        // Off-centre by 0.1 m the ray enters the body capsule (0.30 m before
        // the centre line) before the head sphere (0.17 m): still a head hit.
        let edge = c
            .cast(Vec3::new(0.1, 1.6, 0.0), Vec3::NEG_Z, 100.0)
            .unwrap();
        assert_eq!(edge.zone, HitZone::Head);
        assert!((edge.distance - 9.827).abs() < 0.01);
    }

    #[test]
    fn first_contact_before_stop_distance_wins() {
        // A wall at z = -5 stands between the shooter and the target.
        let statics = StaticWorld::new(vec![Volume::cuboid(
            Vec3::new(0.0, 2.0, -5.5),
            Vec3::new(5.0, 2.0, 0.5),
        )]);
        let targets = [ahead(2, -10.0)];
        let mut c = SceneCaster {
            statics: &statics,
            shooter: SHOOTER,
            targets: &targets,
        };
        assert!(c
            .cast(Vec3::new(0.0, 1.6, 0.0), Vec3::NEG_Z, 100.0)
            .is_none());
        // Nearest of two players wins, whatever the order.
        let open = StaticWorld::new(vec![]);
        let targets = [ahead(3, -12.0), ahead(2, -8.0)];
        let mut c = SceneCaster {
            statics: &open,
            shooter: SHOOTER,
            targets: &targets,
        };
        let hit = c
            .cast(Vec3::new(0.0, 1.6, 0.0), Vec3::NEG_Z, 100.0)
            .unwrap();
        assert_eq!(hit.target, NetId(2));
        // Out of range: nothing.
        assert!(c.cast(Vec3::new(0.0, 1.6, 0.0), Vec3::NEG_Z, 5.0).is_none());
    }

    #[test]
    fn cadence_600_rpm_spaces_shots_by_a_tenth_and_rejects_early_ones() {
        let def = precise(rifle());
        assert!((def.interval() - 0.1).abs() < 1e-12);
        let mut w = WeaponState::new(&def, 0.0);
        let statics = StaticWorld::new(vec![]);
        let mut scene = SceneCaster {
            statics: &statics,
            shooter: SHOOTER,
            targets: &[],
        };
        // Drawn at 0 with a 0.5 s draw time.
        let t0 = 1.0;
        assert!(matches!(
            fire(&def, &mut w, &input(0.2, forward()), &mut scene),
            Err(ShotReject::Cadence { .. })
        ));
        assert!(fire(&def, &mut w, &input(t0, forward()), &mut scene).is_ok());
        // 0.05 s later: rejected, nothing consumed.
        let ammo = w.ammo;
        let counter = w.shot_counter;
        let err = fire(&def, &mut w, &input(t0 + 0.05, forward()), &mut scene).unwrap_err();
        assert!(matches!(err, ShotReject::Cadence { wait } if (wait - 0.05).abs() < 1e-9));
        assert_eq!((w.ammo, w.shot_counter), (ammo, counter));
        // Exactly 0.1 s later: accepted, and again 0.1 s after that.
        assert!(fire(&def, &mut w, &input(t0 + 0.1, forward()), &mut scene).is_ok());
        assert!(fire(&def, &mut w, &input(t0 + 0.2, forward()), &mut scene).is_ok());
        assert!(fire(&def, &mut w, &input(t0 + 0.25, forward()), &mut scene).is_err());
    }

    #[test]
    fn a_pause_does_not_bank_shots() {
        let def = precise(rifle());
        let mut w = WeaponState::new(&def, 0.0);
        let statics = StaticWorld::new(vec![]);
        let mut scene = SceneCaster {
            statics: &statics,
            shooter: SHOOTER,
            targets: &[],
        };
        fire(&def, &mut w, &input(1.0, forward()), &mut scene).unwrap();
        // 10 s of idle, then two shots in the same instant.
        fire(&def, &mut w, &input(11.0, forward()), &mut scene).unwrap();
        assert!(fire(&def, &mut w, &input(11.0, forward()), &mut scene).is_err());
    }

    #[test]
    fn trigger_modes_ammo_and_reload() {
        let mut def = precise(WeaponDef::builtin().remove(1)); // pistol, semi
        def.magazine = 2;
        def.reserve = 5;
        let statics = StaticWorld::new(vec![]);
        let mut scene = SceneCaster {
            statics: &statics,
            shooter: SHOOTER,
            targets: &[],
        };
        let mut w = WeaponState::new(&def, 0.0);
        let mut held = input(1.0, forward());
        held.fresh_press = false;
        assert_eq!(
            fire(&def, &mut w, &held, &mut scene).unwrap_err(),
            ShotReject::NeedsFreshPress
        );
        fire(&def, &mut w, &input(1.0, forward()), &mut scene).unwrap();
        fire(&def, &mut w, &input(1.3, forward()), &mut scene).unwrap();
        assert_eq!(
            fire(&def, &mut w, &input(1.6, forward()), &mut scene).unwrap_err(),
            ShotReject::Empty
        );
        assert!(w.start_reload(&def, 1.6));
        assert!(!w.start_reload(&def, 1.6));
        assert_eq!(
            fire(&def, &mut w, &input(2.0, forward()), &mut scene).unwrap_err(),
            ShotReject::Reloading
        );
        // 1.5 s reload ends at 3.1.
        w.tick(&def, 3.2, DT);
        assert_eq!((w.ammo, w.reserve), (2, 3));
        fire(&def, &mut w, &input(3.2, forward()), &mut scene).unwrap();

        // Burst: three shots per press, then a new press is needed.
        let mut def = precise(rifle());
        def.fire_mode = FireMode::Burst;
        let mut w = WeaponState::new(&def, 0.0);
        let mut t = 1.0;
        fire(&def, &mut w, &input(t, forward()), &mut scene).unwrap();
        for _ in 0..2 {
            t += 0.1;
            let mut i = input(t, forward());
            i.fresh_press = false;
            fire(&def, &mut w, &i, &mut scene).unwrap();
        }
        t += 0.1;
        let mut i = input(t, forward());
        i.fresh_press = false;
        assert_eq!(
            fire(&def, &mut w, &i, &mut scene).unwrap_err(),
            ShotReject::NeedsFreshPress
        );
    }

    #[test]
    fn spread_blooms_per_shot_and_returns() {
        let def = rifle();
        let mut w = WeaponState::new(&def, 0.0);
        let statics = StaticWorld::new(vec![]);
        let mut scene = SceneCaster {
            statics: &statics,
            shooter: SHOOTER,
            targets: &[],
        };
        let r1 = fire(&def, &mut w, &input(1.0, forward()), &mut scene).unwrap();
        assert!((r1.sigma - def.spread.sigma_base).abs() < 1e-7);
        let r2 = fire(&def, &mut w, &input(1.1, forward()), &mut scene).unwrap();
        assert!((r2.sigma - (def.spread.sigma_base + def.spread.delta_beta)).abs() < 1e-6);
        for k in 0..60 {
            w.tick(&def, 1.1 + f64::from(k) * f64::from(DT), DT);
        }
        assert!(w.beta < def.spread.delta_beta * 0.1);
        // The shot function uses the cone computed from the state.
        let mut moving = input(2.5, forward());
        moving.stance.moving = true;
        let r3 = fire(&def, &mut w, &moving, &mut scene).unwrap();
        assert!(r3.sigma > 1.9 * def.spread.sigma_base);
    }

    #[test]
    fn client_and_server_paths_give_identical_shots() {
        let def = rifle();
        let basis = AimBasis::from_quantized(1000, 200);
        let statics = StaticWorld::new(vec![]);
        let mut scene = SceneCaster {
            statics: &statics,
            shooter: SHOOTER,
            targets: &[],
        };
        let mut client = WeaponState::new(&def, 0.0);
        let mut server = WeaponState::new(&def, 0.0);
        for k in 0..20 {
            let now = 1.0 + f64::from(k) * 0.1;
            client.tick(&def, now, DT);
            server.tick(&def, now, DT);
            let a = fire(&def, &mut client, &input(now, basis), &mut scene).unwrap();
            let b = fire(&def, &mut server, &input(now, basis), &mut scene).unwrap();
            for (p, q) in a.pellets.iter().zip(&b.pellets) {
                assert_eq!(
                    p.direction.to_array().map(f32::to_bits),
                    q.direction.to_array().map(f32::to_bits)
                );
            }
            assert_eq!(a.recoil_kick, b.recoil_kick);
        }
        assert_eq!(client, server);
    }

    #[test]
    fn shotgun_pellets_have_own_directions_and_sum_per_victim() {
        let def = WeaponDef::builtin().remove(2);
        let statics = StaticWorld::new(vec![]);
        let targets = [ahead(2, -3.0)];
        let mut scene = SceneCaster {
            statics: &statics,
            shooter: SHOOTER,
            targets: &targets,
        };
        let mut w = WeaponState::new(&def, 0.0);
        let basis = AimBasis::from_quantized(0, 0);
        let mut i = input(1.0, basis);
        i.origin = Vec3::new(0.0, 1.0, 0.0);
        let r = fire(&def, &mut w, &i, &mut scene).unwrap();
        assert_eq!(r.pellets.len(), 8);
        let mut dirs: Vec<[u32; 3]> = r
            .pellets
            .iter()
            .map(|p| p.direction.to_array().map(f32::to_bits))
            .collect();
        dirs.sort_unstable();
        dirs.dedup();
        assert_eq!(dirs.len(), 8);
        let hits = r.pellets.iter().filter(|p| p.contact.is_some()).count();
        assert!(hits >= 1, "at 3 m a body-height shotgun blast connects");
        // One event per victim, the sum of its pellets.
        assert_eq!(r.damage.len(), 1);
        let sum: f32 = r
            .pellets
            .iter()
            .filter_map(|p| p.contact)
            .map(|c| pellet_damage(&def.damage, c.distance, c.zone))
            .sum();
        assert!((r.damage[0].amount - sum).abs() < 1e-4);
        assert_eq!(r.damage[0].victim, TARGET);
    }

    #[test]
    fn recoil_kicks_follow_the_pattern_and_restart_after_a_pause() {
        let def = precise(rifle());
        let statics = StaticWorld::new(vec![]);
        let mut scene = SceneCaster {
            statics: &statics,
            shooter: SHOOTER,
            targets: &[],
        };
        let mut w = WeaponState::new(&def, 0.0);
        let a = fire(&def, &mut w, &input(1.0, forward()), &mut scene).unwrap();
        let b = fire(&def, &mut w, &input(1.1, forward()), &mut scene).unwrap();
        let f = def.recoil.random_fraction;
        let span = |p: f32| (p * (1.0 - f), p * (1.0 + f));
        let (lo, hi) = span(def.recoil.pattern[0].1);
        assert!((lo..=hi).contains(&a.recoil_kick.1));
        let (lo, hi) = span(def.recoil.pattern[1].1);
        assert!((lo..=hi).contains(&b.recoil_kick.1));
        let c = fire(&def, &mut w, &input(5.0, forward()), &mut scene).unwrap();
        let (lo, hi) = span(def.recoil.pattern[0].1);
        assert!((lo..=hi).contains(&c.recoil_kick.1));
    }

    /// Target walking +X at 5 m/s, shooter 10 m away; the shooter aims at
    /// where it saw the target 100 ms ago.
    #[test]
    fn a_shot_at_a_moving_target_with_100_ms_latency_hits() {
        let def = precise(rifle());
        let statics = StaticWorld::new(vec![Volume::cuboid(
            Vec3::new(0.0, -0.5, 0.0),
            Vec3::new(50.0, 0.5, 50.0),
        )]);
        let mut lag = LagCompensation::new();
        for t in 1..=120u32 {
            let x = -5.0 + 5.0 * t as f32 * DT;
            lag.record(
                Tick(t),
                [
                    (SHOOTER, Hitbox::standing(Vec3::new(0.0, 0.91, 10.0))),
                    (TARGET, Hitbox::standing(Vec3::new(x, 0.91, 0.0))),
                ],
            );
        }
        assert!(lag.history(TARGET).count() <= HISTORY_TICKS);
        let eye = lag.authoritative(SHOOTER).unwrap().eye();
        // 100 ms before now = 6 ticks: tick 114.
        let seen = lag.hitbox_at(TARGET, 114.0).unwrap();
        let basis = AimBasis::looking_along(seen.body_center() - eye).unwrap();
        let mut w = WeaponState::new(&def, 0.0);
        let mut scene = RewindCaster {
            lag: &mut lag,
            statics: &statics,
            shooter: SHOOTER,
            rtt: 0.1,
            view_tick: Tick(114),
            view_fraction: 0.0,
        };
        let mut i = input(2.0, basis);
        i.origin = eye;
        let report = fire(&def, &mut w, &i, &mut scene).unwrap();
        let hit = report.pellets[0].contact.expect("lag compensated hit");
        assert_eq!(hit.target, TARGET);
        assert_eq!(report.damage.len(), 1);

        // Without compensation the same aim misses: the target has moved
        // 0.5 m on.
        let now_boxes = [(TARGET, lag.authoritative(TARGET).unwrap())];
        let mut plain = SceneCaster {
            statics: &statics,
            shooter: SHOOTER,
            targets: &now_boxes,
        };
        let mut w = WeaponState::new(&def, 0.0);
        let r = fire(&def, &mut w, &i, &mut plain).unwrap();
        assert!(r.pellets[0].contact.is_none());
    }
}

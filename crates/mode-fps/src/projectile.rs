//! Projectile weapons: launch, per-tick integration with swept collision,
//! and the server-side set of live projectiles.
//!
//! Integration, per tick of `dt` seconds with gravity vector `g`, gravity
//! factor `k_g` and quadratic drag `k_d`:
//!
//! ```text
//! a = k_g * g - k_d * |v| * v;   p <- p + v*dt + a*dt^2/2;   v <- v + a*dt
//! ```
//!
//! Decision: the position uses the second-order term so that without drag
//! the trajectory is exactly `p0 + v0 t + k_g g t^2 / 2` (a plain
//! semi-implicit Euler step drifts by `k_g g dt t / 2`, 17 cm after a
//! second). With drag the step is second order in `dt`.
//!
//! The move of each tick is swept with a sphere of the weapon's radius
//! against the static volumes (and, on the server, the players' capsules),
//! so a fast projectile never crosses a wall. A bounce reflects the
//! velocity about the surface normal and scales it by the restitution; the
//! rest of the tick is dropped (decision: the projectile stops at the
//! contact point for that tick). The end of the last bounce, a player
//! contact or the lifetime ends the projectile.
//!
//! Network model: the server creates the real projectile at the command
//! tick and advances it by half the shooter's round trip, capped at 100 ms
//! ([`catch_up_time`]); clients get only the [`ProjectileSpawn`] event and
//! simulate it themselves (statics only). Contacts with players are
//! resolved by the server, in the present.

use faset_core::glam::Vec3;
use faset_core::{NetId, DT};
use faset_server::Hitbox;
use faset_sim::collision::player_capsule;
use faset_sim::StaticWorld;
use parry3d::bounding_volume::{Aabb, BoundingVolume};
use parry3d::math::{Pose, Vector};
use parry3d::query::{self, ShapeCastOptions};
use parry3d::shape::{Ball, Shape};

use crate::damage::{DamageEvent, DamageType, HitZone};
use crate::explosion::{blast, explosion_centre};
use crate::message::ProjectileSpawn;
use crate::shot::{begin_shot, end_shot, ShotInput, ShotReject, WeaponState};
use crate::spread::{draw_pair, spread_direction};
use crate::weapon::{ProjectileDef, WeaponDef, WeaponKind};

/// Longest head start the server gives a projectile, seconds.
pub const MAX_CATCH_UP: f32 = 0.1;
/// A bounce leaves the projectile this far off the surface, metres, so the
/// next sweep does not start in contact.
const BOUNCE_OFFSET: f32 = 1e-4;
/// Gravity of the world, m/s^2 (the controller's).
pub const GRAVITY: Vec3 = Vec3::new(0.0, -20.0, 0.0);

/// Head start of a projectile fired by a player with round trip `rtt`:
/// half of it, at most [`MAX_CATCH_UP`].
#[must_use]
pub fn catch_up_time(rtt: f32) -> f32 {
    (rtt * 0.5).clamp(0.0, MAX_CATCH_UP)
}

/// Kinematic state of one projectile.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProjectileState {
    pub pos: Vec3,
    pub vel: Vec3,
    /// Seconds since creation.
    pub age: f64,
    pub bounces_left: u32,
}

impl ProjectileState {
    /// A fresh projectile of `def` leaving `pos` with velocity `vel`.
    #[must_use]
    pub fn new(def: &ProjectileDef, pos: Vec3, vel: Vec3) -> Self {
        Self {
            pos,
            vel,
            age: 0.0,
            bounces_left: def.bounces,
        }
    }
}

/// What a step needs to know about the world.
#[derive(Clone, Copy)]
pub struct ProjectileEnv<'a> {
    pub def: &'a ProjectileDef,
    pub statics: &'a StaticWorld,
    /// Gravity vector `g` (see [`GRAVITY`]).
    pub gravity: Vec3,
    /// The shooter, whom the projectile passes through.
    pub owner: NetId,
    /// Players the projectile can hit (empty for a client's visual copy).
    pub targets: &'a [(NetId, Hitbox)],
}

/// How a step ended.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ProjectileEvent {
    /// Still in flight.
    Flying,
    /// Bounced off a wall; the state already holds the new velocity.
    Bounced { point: Vec3, normal: Vec3 },
    /// Stopped against a wall (`target` None) or a player.
    Impact {
        point: Vec3,
        normal: Vec3,
        target: Option<NetId>,
    },
    /// The lifetime ran out.
    Expired,
}

/// Result of [`step_projectile`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProjectileStep {
    pub state: ProjectileState,
    pub event: ProjectileEvent,
}

fn v3(v: Vec3) -> Vector {
    Vector::new(v.x, v.y, v.z)
}

fn from_v(v: Vector) -> Vec3 {
    Vec3::new(v.x, v.y, v.z)
}

fn pose(p: Vec3) -> Pose {
    Pose::translation(p.x, p.y, p.z)
}

/// Earliest contact of a sphere moving by `delta` with `shape` at `at`:
/// `(fraction, outward normal)`; surfaces the sphere moves away from are
/// ignored.
fn sweep_shape(
    shape: &dyn Shape,
    at: &Pose,
    ball: &Ball,
    from: Vec3,
    delta: Vec3,
) -> Option<(f32, Vec3)> {
    let opts = ShapeCastOptions {
        max_time_of_impact: 1.0,
        target_distance: 0.0,
        stop_at_penetration: true,
        compute_impact_geometry_on_penetration: true,
    };
    let hit =
        query::cast_shapes(at, Vector::ZERO, shape, &pose(from), v3(delta), ball, opts).ok()??;
    let t = hit.time_of_impact.clamp(0.0, 1.0);
    let mut normal = from_v(at.rotation * hit.normal1);
    // The normal of a shape cast is imprecise for a sphere that grazes a
    // face at low speed (a few percent off): at the contact point, the
    // direction from the closest point of the shape to the centre is exact.
    let centre = from + delta * t;
    let proj = shape.project_point(at, v3(centre), false);
    if !proj.is_inside {
        let radial = centre - from_v(proj.point);
        if radial.length() > 1e-6 {
            normal = radial.normalize();
        }
    }
    (normal.dot(delta) < 0.0).then_some((t, normal))
}

/// Advances `state` by one tick of `dt` seconds. Pure: the same input
/// always gives the same output.
#[must_use]
pub fn step_projectile(state: &ProjectileState, dt: f32, env: &ProjectileEnv) -> ProjectileStep {
    let def = env.def;
    let speed = state.vel.length();
    let accel = env.gravity * def.gravity - state.vel * (def.drag * speed);
    let delta = state.vel * dt + accel * (0.5 * dt * dt);
    let vel = state.vel + accel * dt;
    let mut next = ProjectileState {
        age: state.age + f64::from(dt),
        ..*state
    };

    let ball = Ball::new(def.radius);
    let swept = Aabb::new(
        v3(state.pos.min(state.pos + delta)),
        v3(state.pos.max(state.pos + delta)),
    )
    .loosened(def.radius + 1e-3);
    // Earliest contact: (fraction, normal, player).
    let mut best: Option<(f32, Vec3, Option<NetId>)> = None;
    for vol in env.statics.volumes() {
        if !vol.shape.compute_aabb(&vol.pose).intersects(&swept) {
            continue;
        }
        if let Some((t, n)) = sweep_shape(vol.shape.as_ref(), &vol.pose, &ball, state.pos, delta) {
            if best.is_none_or(|b| t < b.0) {
                best = Some((t, n, None));
            }
        }
    }
    let mut sorted: Vec<&(NetId, Hitbox)> = env.targets.iter().collect();
    sorted.sort_by_key(|(id, _)| *id);
    for (id, hb) in sorted {
        if *id == env.owner {
            continue;
        }
        let capsule = player_capsule(hb.body_height());
        if let Some((t, n)) =
            sweep_shape(&capsule, &pose(hb.body_center()), &ball, state.pos, delta)
        {
            if best.is_none_or(|b| t < b.0) {
                best = Some((t, n, Some(*id)));
            }
        }
    }

    if let Some((t, normal, target)) = best {
        let point = state.pos + delta * t;
        if target.is_none() && state.bounces_left > 0 {
            next.bounces_left -= 1;
            next.vel = (vel - normal * (2.0 * vel.dot(normal))) * def.restitution;
            next.pos = point + normal * BOUNCE_OFFSET;
            return ProjectileStep {
                state: next,
                event: ProjectileEvent::Bounced { point, normal },
            };
        }
        next.pos = point;
        next.vel = vel;
        return ProjectileStep {
            state: next,
            event: ProjectileEvent::Impact {
                point,
                normal,
                target,
            },
        };
    }

    next.pos = state.pos + delta;
    next.vel = vel;
    // Tolerance: the age is a sum of dt, which is not exact in binary.
    let event = if next.age >= f64::from(def.lifetime) - 1e-6 {
        ProjectileEvent::Expired
    } else {
        ProjectileEvent::Flying
    };
    ProjectileStep { state: next, event }
}

/// Runs [`step_projectile`] for `seconds` (rounded to whole ticks), stopping
/// at the first step that does not end in [`ProjectileEvent::Flying`] or
/// [`ProjectileEvent::Bounced`]. Used for the server's latency head start.
#[must_use]
pub fn advance_projectile(
    state: &ProjectileState,
    seconds: f32,
    env: &ProjectileEnv,
) -> ProjectileStep {
    let ticks = (seconds / DT).round().max(0.0) as u32;
    let mut step = ProjectileStep {
        state: *state,
        event: ProjectileEvent::Flying,
    };
    for _ in 0..ticks {
        step = step_projectile(&step.state, DT, env);
        if matches!(
            step.event,
            ProjectileEvent::Impact { .. } | ProjectileEvent::Expired
        ) {
            break;
        }
    }
    step
}

/// What [`launch`] produced.
#[derive(Debug, Clone, PartialEq)]
pub struct Launch {
    /// Counter the generator was seeded with.
    pub shot_index: u32,
    /// Cone half angle used.
    pub sigma: f32,
    /// One creation event per pellet.
    pub spawns: Vec<ProjectileSpawn>,
    /// View kick added to the shooter's recoil, `(yaw, pitch)`.
    pub recoil_kick: (f32, f32),
}

/// Fires a projectile weapon once if the rules allow it: same cadence,
/// ammo, spread and recoil rules as [`fire`](crate::shot::fire). Decision:
/// the projectile leaves the eye and does not inherit the shooter's
/// velocity. `weapon_index` and `tick` go into the creation events.
///
/// # Errors
/// [`ShotReject`] when the weapon cannot fire at `input.now`.
pub fn launch(
    def: &WeaponDef,
    weapon_index: u8,
    weapon: &mut WeaponState,
    input: &ShotInput,
    tick: u32,
) -> Result<Launch, ShotReject> {
    let (Some(proj), WeaponKind::Projectile) = (&def.projectile, def.kind) else {
        return Err(ShotReject::NotProjectile);
    };
    let (shot_index, mut rng) = begin_shot(def, weapon, input, true)?;
    let (u_yaw, u_pitch) = draw_pair(&mut rng);
    let sigma = weapon.sigma(def, input.stance);
    let mut spawns = Vec::with_capacity(def.pellets as usize);
    for _ in 0..def.pellets {
        let (u1, u2) = draw_pair(&mut rng);
        let dir = spread_direction(&input.basis, sigma, u1, u2);
        spawns.push(ProjectileSpawn {
            shooter: input.shooter,
            weapon: weapon_index,
            tick,
            seed: rng.next_u32(),
            origin: input.origin,
            velocity: dir * proj.speed,
        });
    }
    let recoil_kick = end_shot(def, weapon, input.now, u_yaw, u_pitch);
    Ok(Launch {
        shot_index,
        sigma,
        spawns,
        recoil_kick,
    })
}

/// A projectile that ended, with what it did to the players.
#[derive(Debug, Clone, PartialEq)]
pub struct Detonation {
    pub owner: NetId,
    /// Index of the weapon in the weapon list.
    pub weapon: usize,
    /// Where the blast (or the impact) happened.
    pub centre: Vec3,
    /// Whether an explosion went off (for the visual cue).
    pub exploded: bool,
    /// Damage to queue for the chain, ascending victim id.
    pub damage: Vec<DamageEvent>,
    /// Velocity changes to add to the players, ascending id.
    pub knockback: Vec<(NetId, Vec3)>,
}

#[derive(Debug, Clone, PartialEq)]
struct Live {
    owner: NetId,
    weapon: usize,
    state: ProjectileState,
}

/// The server's live projectiles.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProjectileSystem {
    live: Vec<Live>,
}

impl ProjectileSystem {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Projectiles in flight.
    #[must_use]
    pub fn len(&self) -> usize {
        self.live.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.live.is_empty()
    }

    /// Creates the real projectile of `spawn`, advanced by `catch_up`
    /// seconds (see [`catch_up_time`]). Returns its detonation if it already
    /// ended during the head start.
    pub fn spawn(
        &mut self,
        weapons: &[WeaponDef],
        spawn: &ProjectileSpawn,
        catch_up: f32,
        statics: &StaticWorld,
        targets: &[(NetId, Hitbox)],
    ) -> Option<Detonation> {
        let index = usize::from(spawn.weapon);
        let def = weapons.get(index)?.projectile.as_ref()?;
        let env = ProjectileEnv {
            def,
            statics,
            gravity: GRAVITY,
            owner: spawn.shooter,
            targets,
        };
        let state = ProjectileState::new(def, spawn.origin, spawn.velocity);
        let step = advance_projectile(&state, catch_up.clamp(0.0, MAX_CATCH_UP), &env);
        if let Some(d) = resolve(weapons, index, spawn.shooter, &step, statics, targets) {
            return Some(d);
        }
        self.live.push(Live {
            owner: spawn.shooter,
            weapon: index,
            state: step.state,
        });
        None
    }

    /// One tick of every live projectile against the players as they are
    /// now. Returns the projectiles that ended, in creation order.
    pub fn step(
        &mut self,
        weapons: &[WeaponDef],
        statics: &StaticWorld,
        targets: &[(NetId, Hitbox)],
    ) -> Vec<Detonation> {
        let mut out = Vec::new();
        let mut kept = Vec::with_capacity(self.live.len());
        for mut p in std::mem::take(&mut self.live) {
            let Some(def) = weapons[p.weapon].projectile.as_ref() else {
                continue;
            };
            let env = ProjectileEnv {
                def,
                statics,
                gravity: GRAVITY,
                owner: p.owner,
                targets,
            };
            let step = step_projectile(&p.state, DT, &env);
            if let Some(d) = resolve(weapons, p.weapon, p.owner, &step, statics, targets) {
                out.push(d);
            } else {
                p.state = step.state;
                kept.push(p);
            }
        }
        self.live = kept;
        out
    }
}

/// Turns the end of a projectile into damage and knockback; `None` while it
/// still flies.
fn resolve(
    weapons: &[WeaponDef],
    index: usize,
    owner: NetId,
    step: &ProjectileStep,
    statics: &StaticWorld,
    targets: &[(NetId, Hitbox)],
) -> Option<Detonation> {
    let def = &weapons[index];
    let proj = def.projectile.as_ref()?;
    let (centre, target) = match step.event {
        ProjectileEvent::Flying | ProjectileEvent::Bounced { .. } => return None,
        ProjectileEvent::Impact {
            point,
            normal,
            target,
        } => (explosion_centre(point, normal), target),
        ProjectileEvent::Expired => (step.state.pos, None),
    };
    let mut det = Detonation {
        owner,
        weapon: index,
        centre,
        exploded: false,
        damage: Vec::new(),
        knockback: Vec::new(),
    };
    if let Some(explosion) = &proj.explosion {
        det.exploded = true;
        for e in blast(centre, owner, explosion, targets, statics) {
            det.damage.extend(e.damage);
            if e.delta_v != Vec3::ZERO {
                det.knockback.push((e.victim, e.delta_v));
            }
        }
    } else if let Some(victim) = target {
        det.damage.push(DamageEvent {
            attacker: owner,
            victim,
            amount: def.damage.base,
            kind: DamageType::Bullet,
            zone: HitZone::Body,
            position: step.state.pos,
        });
    }
    Some(det)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spread::{AimBasis, Stance};
    use faset_sim::Volume;

    const OWNER: NetId = NetId(1);

    fn def(speed: f32, gravity: f32, drag: f32) -> ProjectileDef {
        ProjectileDef {
            speed,
            gravity,
            lifetime: 5.0,
            drag,
            radius: 0.1,
            bounces: 0,
            restitution: 0.5,
            explosion: None,
        }
    }

    fn env<'a>(d: &'a ProjectileDef, w: &'a StaticWorld) -> ProjectileEnv<'a> {
        ProjectileEnv {
            def: d,
            statics: w,
            gravity: GRAVITY,
            owner: OWNER,
            targets: &[],
        }
    }

    #[test]
    fn drag_free_trajectory_matches_closed_form() {
        let d = def(30.0, 0.7, 0.0);
        let w = StaticWorld::new(vec![]);
        let e = env(&d, &w);
        let p0 = Vec3::new(1.0, 2.0, 3.0);
        let v0 = Vec3::new(10.0, 12.0, -20.0);
        let mut s = ProjectileState::new(&d, p0, v0);
        for n in 1..=180 {
            s = step_projectile(&s, DT, &e).state;
            let t = n as f32 * DT;
            let exact = p0 + v0 * t + GRAVITY * (0.5 * 0.7 * t * t);
            assert!(
                (s.pos - exact).length() < 1e-3,
                "tick {n}: {:?} vs {exact:?}",
                s.pos
            );
        }
    }

    #[test]
    fn drag_slows_the_projectile() {
        let free = def(30.0, 0.0, 0.0);
        let drag = def(30.0, 0.0, 0.02);
        let w = StaticWorld::new(vec![]);
        let v0 = Vec3::new(0.0, 0.0, -30.0);
        let mut a = ProjectileState::new(&free, Vec3::ZERO, v0);
        let mut b = ProjectileState::new(&drag, Vec3::ZERO, v0);
        for _ in 0..60 {
            a = step_projectile(&a, DT, &env(&free, &w)).state;
            b = step_projectile(&b, DT, &env(&drag, &w)).state;
        }
        assert!(b.vel.length() < a.vel.length() - 5.0);
        assert!(b.pos.z > a.pos.z);
    }

    #[test]
    fn fast_projectile_never_tunnels_a_thin_wall() {
        // A 0.2 m wall at z = -10, 60 m/s: one tick moves 1 m.
        let w = StaticWorld::new(vec![Volume::cuboid(
            Vec3::new(0.0, 0.0, -10.0),
            Vec3::new(5.0, 5.0, 0.1),
        )]);
        let d = def(60.0, 0.0, 0.0);
        // Many start offsets so the sample points land on every phase.
        for i in 0..40 {
            let z0 = -i as f32 * 0.025;
            let mut s =
                ProjectileState::new(&d, Vec3::new(0.0, 0.0, z0), Vec3::new(0.0, 0.0, -60.0));
            let mut ended = false;
            for _ in 0..60 {
                let step = step_projectile(&s, DT, &env(&d, &w));
                assert!(step.state.pos.z > -10.0 - 1e-3, "tunnelled from {z0}");
                s = step.state;
                if matches!(step.event, ProjectileEvent::Impact { .. }) {
                    ended = true;
                    break;
                }
            }
            assert!(ended);
            // Stopped one radius in front of the wall face (z = -9.9).
            assert!((s.pos.z - (-9.9 + 0.1)).abs() < 1e-2, "{}", s.pos.z);
        }
    }

    #[test]
    fn bounces_are_counted_then_impact() {
        // Floor at y = 0; a ball dropped from 2 m with 2 bounces.
        let w = StaticWorld::new(vec![Volume::cuboid(
            Vec3::new(0.0, -0.5, 0.0),
            Vec3::new(50.0, 0.5, 50.0),
        )]);
        let mut d = def(1.0, 1.0, 0.0);
        d.bounces = 2;
        d.restitution = 0.5;
        d.lifetime = 30.0;
        let mut s = ProjectileState::new(&d, Vec3::new(0.0, 2.0, 0.0), Vec3::ZERO);
        let (mut bounced, mut impacts) = (0, 0);
        let mut last_up = 0.0_f32;
        for _ in 0..600 {
            let step = step_projectile(&s, DT, &env(&d, &w));
            s = step.state;
            match step.event {
                ProjectileEvent::Bounced { normal, .. } => {
                    bounced += 1;
                    assert!((normal - Vec3::Y).length() < 1e-2, "{normal:?} {s:?}");
                    assert!(s.vel.y > 0.0);
                    // Each bounce keeps half the speed.
                    if bounced == 2 {
                        assert!(
                            (s.vel.y - last_up * 0.5).abs() < 0.3,
                            "{} {last_up}",
                            s.vel.y
                        );
                    }
                    last_up = s.vel.y;
                }
                ProjectileEvent::Impact { target, .. } => {
                    impacts += 1;
                    assert_eq!(target, None);
                    break;
                }
                _ => {}
            }
        }
        assert_eq!((bounced, impacts), (2, 1));
        assert_eq!(s.bounces_left, 0);
    }

    #[test]
    fn lifetime_expires_on_time() {
        let mut d = def(10.0, 0.0, 0.0);
        d.lifetime = 0.5;
        let w = StaticWorld::new(vec![]);
        let mut s = ProjectileState::new(&d, Vec3::ZERO, Vec3::new(0.0, 0.0, -10.0));
        let mut ticks = 0;
        loop {
            let step = step_projectile(&s, DT, &env(&d, &w));
            s = step.state;
            ticks += 1;
            if step.event == ProjectileEvent::Expired {
                break;
            }
            assert!(ticks < 100);
        }
        assert_eq!(ticks, 30);
    }

    #[test]
    fn players_are_hit_but_not_the_owner() {
        let d = def(30.0, 0.0, 0.0);
        let w = StaticWorld::new(vec![]);
        let targets = [
            (OWNER, Hitbox::standing(Vec3::new(0.0, 0.9, 0.0))),
            (NetId(2), Hitbox::standing(Vec3::new(0.0, 0.9, -6.0))),
        ];
        let e = ProjectileEnv {
            targets: &targets,
            ..env(&d, &w)
        };
        let s = ProjectileState::new(&d, Vec3::new(0.0, 0.9, 0.0), Vec3::new(0.0, 0.0, -30.0));
        let step = advance_projectile(&s, 0.5, &e);
        assert!(matches!(
            step.event,
            ProjectileEvent::Impact {
                target: Some(NetId(2)),
                ..
            }
        ));
    }

    #[test]
    fn catch_up_is_half_the_rtt_capped_at_100_ms() {
        assert!((catch_up_time(0.06) - 0.03).abs() < 1e-6);
        assert!((catch_up_time(0.5) - 0.1).abs() < 1e-6);
        assert_eq!(catch_up_time(-1.0), 0.0);
    }

    fn rocket() -> WeaponDef {
        WeaponDef::builtin().remove(3)
    }

    #[test]
    fn launch_is_deterministic_and_follows_the_cadence() {
        let def = rocket();
        let input = ShotInput {
            now: 1.0,
            seed: 5,
            shooter: OWNER,
            origin: Vec3::new(0.0, 1.6, 0.0),
            basis: AimBasis::from_quantized(0, 0),
            stance: Stance::default(),
            fresh_press: true,
        };
        let mut a = WeaponState::new(&def, 0.0);
        let mut b = WeaponState::new(&def, 0.0);
        let la = launch(&def, 3, &mut a, &input, 60).unwrap();
        let lb = launch(&def, 3, &mut b, &input, 60).unwrap();
        assert_eq!(la, lb);
        assert_eq!(la.spawns.len(), 1);
        let v = la.spawns[0].velocity;
        assert!((v.length() - 35.0).abs() < 1e-3);
        assert!(v.z < 0.0);
        assert_eq!(a.ammo, def.magazine - 1);
        // Too soon: 70 rpm is 0.857 s between rockets.
        let again = ShotInput { now: 1.2, ..input };
        assert!(matches!(
            launch(&def, 3, &mut a, &again, 72),
            Err(ShotReject::Cadence { .. })
        ));
        // A hitscan weapon is refused.
        let rifle = WeaponDef::builtin().remove(0);
        let mut w = WeaponState::new(&rifle, 0.0);
        assert_eq!(
            launch(&rifle, 0, &mut w, &input, 60),
            Err(ShotReject::NotProjectile)
        );
    }

    #[test]
    fn system_detonates_a_rocket_and_hurts_the_target() {
        let weapons = WeaponDef::builtin();
        let w = StaticWorld::new(vec![Volume::cuboid(
            Vec3::new(0.0, 0.0, -20.0),
            Vec3::new(10.0, 10.0, 0.5),
        )]);
        let targets = [
            (OWNER, Hitbox::standing(Vec3::new(0.0, 0.9, 0.0))),
            (NetId(2), Hitbox::standing(Vec3::new(1.5, 0.9, -18.0))),
        ];
        let spawn = ProjectileSpawn {
            shooter: OWNER,
            weapon: 3,
            tick: 0,
            seed: 0,
            origin: Vec3::new(0.0, 1.0, 0.0),
            velocity: Vec3::new(0.0, 0.0, -35.0),
        };
        let mut sys = ProjectileSystem::new();
        assert!(sys.spawn(&weapons, &spawn, 0.03, &w, &targets).is_none());
        assert_eq!(sys.len(), 1);
        let mut dets = Vec::new();
        for _ in 0..120 {
            dets.extend(sys.step(&weapons, &w, &targets));
            if !dets.is_empty() {
                break;
            }
        }
        assert!(sys.is_empty());
        let d = &dets[0];
        assert!(d.exploded);
        assert!(d.centre.z > -19.5 && d.centre.z < -19.0, "{:?}", d.centre);
        assert_eq!(d.damage.len(), 1);
        assert_eq!(d.damage[0].victim, NetId(2));
        assert!(d
            .knockback
            .iter()
            .any(|(id, v)| *id == NetId(2) && v.z > 0.0));
    }
}

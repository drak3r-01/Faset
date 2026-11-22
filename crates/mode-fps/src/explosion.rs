//! Area explosions: damage and knockback falling with the distance from the
//! blast centre.
//!
//! For `d < R`: `D(d) = D0 * (1 - d/R)^p` and `delta_v = u_hat * K * (1 - d/R)`
//! with `u_hat` pointing from the centre to the target. A ray from the
//! centre to the target must be free of walls, otherwise there is neither
//! damage nor knockback. The shooter's own damage is halved
//! ([`SELF_DAMAGE_FACTOR`]) but the knockback is kept: that is the explosive
//! jump.
//!
//! Decisions:
//! * The target of a blast is the centre of the player's body capsule.
//! * [`explosion_centre`] backs a blast off its surface by
//!   [`CENTRE_OFFSET`] so the line-of-sight ray does not start inside the
//!   wall it hit.
//! * Knockback is a velocity change of the target; applying it (and
//!   clearing `on_ground`) belongs to whoever owns the controller state.

use faset_core::glam::Vec3;
use faset_core::NetId;
use faset_server::Hitbox;
use faset_sim::StaticWorld;

use crate::damage::{DamageEvent, DamageType, HitZone};
use crate::weapon::ExplosionDef;

/// Share of the damage a player takes from their own blast.
pub const SELF_DAMAGE_FACTOR: f32 = 0.5;
/// How far a blast centre is moved off the surface it hit, metres.
pub const CENTRE_OFFSET: f32 = 0.05;

/// Where a blast goes off for a projectile that stopped at `point` against a
/// surface of normal `normal`.
#[must_use]
pub fn explosion_centre(point: Vec3, normal: Vec3) -> Vec3 {
    point + normal * CENTRE_OFFSET
}

/// Damage and velocity change a blast at `centre` gives a target at
/// `target`. `(0, 0)` outside the radius or behind a wall.
#[must_use]
pub fn explosion_effect(
    centre: Vec3,
    target: Vec3,
    def: &ExplosionDef,
    world: &StaticWorld,
) -> (f32, Vec3) {
    let offset = target - centre;
    let d = offset.length();
    if d >= def.radius {
        return (0.0, Vec3::ZERO);
    }
    let u_hat = if d > 1e-6 { offset / d } else { Vec3::Y };
    if d > 1e-6 && world.ray_cast(centre, u_hat, d).is_some() {
        return (0.0, Vec3::ZERO);
    }
    let k = 1.0 - d / def.radius;
    (def.damage * k.powf(def.exponent), u_hat * def.knockback * k)
}

/// What a blast does to one player.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlastEffect {
    pub victim: NetId,
    /// Damage after the self factor; `None` when it is zero.
    pub damage: Option<DamageEvent>,
    /// Velocity change to add to the victim.
    pub delta_v: Vec3,
}

/// Runs a blast of `source` at `centre` over `targets` (every player,
/// the source included), in ascending id order. Players the blast does not
/// reach are left out.
#[must_use]
pub fn blast(
    centre: Vec3,
    source: NetId,
    def: &ExplosionDef,
    targets: &[(NetId, Hitbox)],
    world: &StaticWorld,
) -> Vec<BlastEffect> {
    let mut sorted: Vec<&(NetId, Hitbox)> = targets.iter().collect();
    sorted.sort_by_key(|(id, _)| *id);
    let mut out = Vec::new();
    for (id, hb) in sorted {
        let at = hb.body_center();
        let (mut amount, delta_v) = explosion_effect(centre, at, def, world);
        if amount <= 0.0 && delta_v == Vec3::ZERO {
            continue;
        }
        if *id == source {
            amount *= SELF_DAMAGE_FACTOR;
        }
        out.push(BlastEffect {
            victim: *id,
            damage: (amount > 0.0).then_some(DamageEvent {
                attacker: source,
                victim: *id,
                amount,
                kind: DamageType::Explosion,
                zone: HitZone::Body,
                position: at,
            }),
            delta_v,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use faset_sim::controller::{step_player_in, MoveCmd, MoveParams, PlayerState};
    use faset_sim::Volume;

    fn def() -> ExplosionDef {
        ExplosionDef {
            radius: 4.0,
            damage: 100.0,
            exponent: 1.5,
            knockback: 14.0,
        }
    }

    fn floor() -> StaticWorld {
        StaticWorld::new(vec![Volume::cuboid(
            Vec3::new(0.0, -0.5, 0.0),
            Vec3::new(50.0, 0.5, 50.0),
        )])
    }

    #[test]
    fn damage_and_knockback_follow_the_formula() {
        let w = StaticWorld::new(vec![]);
        let c = Vec3::ZERO;
        let (dmg, dv) = explosion_effect(c, Vec3::new(1.0, 0.0, 0.0), &def(), &w);
        let k = 1.0 - 1.0 / 4.0_f32;
        assert!((dmg - 100.0 * k.powf(1.5)).abs() < 1e-4);
        assert!((dv - Vec3::X * 14.0 * k).length() < 1e-5);
        // Outside the radius: nothing. At the centre: full damage, up.
        assert_eq!(
            explosion_effect(c, Vec3::new(4.0, 0.0, 0.0), &def(), &w),
            (0.0, Vec3::ZERO)
        );
        let (dmg, dv) = explosion_effect(c, c, &def(), &w);
        assert!((dmg - 100.0).abs() < 1e-4 && (dv - Vec3::Y * 14.0).length() < 1e-5);
    }

    #[test]
    fn wall_blocks_damage_and_knockback() {
        let w = StaticWorld::new(vec![Volume::cuboid(
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.1, 3.0, 3.0),
        )]);
        let c = Vec3::ZERO;
        let behind = Vec3::new(2.0, 0.0, 0.0);
        assert_eq!(explosion_effect(c, behind, &def(), &w), (0.0, Vec3::ZERO));
        let clear = Vec3::new(-2.0, 0.0, 0.0);
        assert!(explosion_effect(c, clear, &def(), &w).0 > 0.0);
    }

    #[test]
    fn self_damage_is_half_and_knockback_is_kept() {
        let w = StaticWorld::new(vec![]);
        let hb = Hitbox::standing(Vec3::new(2.0, 0.9, 0.0));
        let targets = [(NetId(1), hb), (NetId(2), hb)];
        let centre = Vec3::new(0.0, 0.9, 0.0);
        let effects = blast(centre, NetId(1), &def(), &targets, &w);
        assert_eq!(effects.len(), 2);
        let me = effects[0].damage.unwrap().amount;
        let other = effects[1].damage.unwrap().amount;
        assert!((me - other * 0.5).abs() < 1e-5, "{me} vs {other}");
        assert_eq!(effects[0].delta_v, effects[1].delta_v);
        assert_eq!(effects[0].damage.unwrap().kind, DamageType::Explosion);
    }

    #[test]
    fn explosive_jump_matches_formula() {
        let world = floor();
        let params = MoveParams::default();
        let g = params.gravity;
        // A player standing on the floor, blast right under the feet.
        let mut state = PlayerState::spawn(Vec3::new(0.0, 0.9 + 0.01, 0.0));
        for _ in 0..30 {
            step_player_in(&mut state, MoveCmd::default(), &world, &params, 1.0 / 60.0);
        }
        assert!(state.on_ground);
        let feet = state.feet();
        let centre = explosion_centre(feet, Vec3::Y);
        let hb = Hitbox::standing(state.pos);
        let effects = blast(centre, NetId(1), &def(), &[(NetId(1), hb)], &world);
        let e = effects[0];
        // The formula: straight up, K * (1 - d/R).
        let d = (state.pos - centre).length();
        let expected = 14.0 * (1.0 - d / 4.0);
        assert!(
            (e.delta_v - Vec3::Y * expected).length() < 1e-5,
            "{:?}",
            e.delta_v
        );

        state.vel += e.delta_v;
        state.on_ground = false;
        let start = state.pos.y;
        let mut apex = start;
        for _ in 0..240 {
            step_player_in(&mut state, MoveCmd::default(), &world, &params, 1.0 / 60.0);
            apex = apex.max(state.pos.y);
        }
        let analytic = expected * expected / (2.0 * g);
        assert!(
            (apex - start - analytic).abs() < 0.01,
            "apex {} vs analytic {analytic}",
            apex - start
        );
    }
}

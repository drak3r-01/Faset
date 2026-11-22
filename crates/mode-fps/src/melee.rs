//! Melee: a short-range arc hit test against hitboxes rewound to the instant
//! the shooter saw (lot 1.7 history), feeding the damage chain of lot 4.4
//! with [`DamageType::Melee`](crate::damage::DamageType::Melee).
//!
//! The reach (`range`), the opening of the arc (`arc`) and the cooldown
//! (`60 / rpm`) are data ([`MeleeDef`](crate::weapon::MeleeDef),
//! [`WeaponDef`]). Decisions:
//! * The reach is measured from the eye to the surface of the target's body
//!   capsule; the arc is the full opening angle around the view direction
//!   and is widened by the angular size of the capsule, so grazing a
//!   shoulder counts.
//! * One swing hits at most one player: the nearest one that is within
//!   reach, inside the arc and not hidden by a wall (ties go to the lowest
//!   id). The zone is always `Body`.
//! * Damage is `damage.base` with the weapon's falloff at the hit distance
//!   (the knife ships without falloff).
//! * Melee does not use ammo, spread, bloom or recoil.
//! * Shipped values (RON): knife, reach 1.8 m, arc 100 degrees, 120 rpm.

use faset_core::glam::Vec3;
use faset_core::{NetId, Tick};
use faset_server::{FireCommand, Hitbox, LagCompensation};
use faset_sim::collision::player_capsule;
use faset_sim::StaticWorld;

use crate::damage::{pellet_damage, DamageEvent, DamageType, HitZone};
use crate::shot::{begin_shot, PlayerContact, ShotInput, ShotReject, WeaponState};
use crate::weapon::{WeaponDef, WeaponKind};

/// Radius of the body capsule, metres.
const BODY_RADIUS: f32 = 0.35;

/// What a swing did.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeleeReport {
    /// Counter of the swing.
    pub shot_index: u32,
    pub contact: Option<PlayerContact>,
    pub damage: Option<DamageEvent>,
}

/// The players as the shooter saw them: every id in `players` but
/// `shooter`, placed where `lag` says they were at `view_tick` plus
/// `view_fraction`, within the rewind limits of lot 1.7 and the shooter's
/// round trip `rtt` (seconds).
#[must_use]
pub fn rewound_hitboxes(
    lag: &LagCompensation,
    shooter: NetId,
    players: impl IntoIterator<Item = NetId>,
    view_tick: Tick,
    view_fraction: f32,
    rtt: f32,
) -> Vec<(NetId, Hitbox)> {
    let probe = FireCommand {
        view_tick,
        view_fraction,
        direction: Vec3::NEG_Z,
        range: 0.0,
    };
    let Some((time, _)) = lag.resolve_view_time(&probe, rtt) else {
        return Vec::new();
    };
    players
        .into_iter()
        .filter(|&id| id != shooter)
        .filter_map(|id| lag.hitbox_at(id, time).map(|hb| (id, hb)))
        .collect()
}

/// Nearest target of a swing from `eye` along `forward`, without damage.
fn pick(
    eye: Vec3,
    forward: Vec3,
    range: f32,
    arc: f32,
    shooter: NetId,
    targets: &[(NetId, Hitbox)],
    statics: &StaticWorld,
) -> Option<PlayerContact> {
    let mut sorted: Vec<&(NetId, Hitbox)> = targets.iter().collect();
    sorted.sort_by_key(|(id, _)| *id);
    let mut best: Option<PlayerContact> = None;
    for (id, hb) in sorted {
        if *id == shooter {
            continue;
        }
        let c = hb.body_center();
        let half = player_capsule(hb.body_height()).segment.b.y;
        let p = Vec3::new(c.x, eye.y.clamp(c.y - half, c.y + half), c.z);
        let to = p - eye;
        let dist = to.length();
        let surface = (dist - BODY_RADIUS).max(0.0);
        if surface > range {
            continue;
        }
        if surface > 0.0 {
            let dir = to / dist;
            let tolerance = arc * 0.5 + (BODY_RADIUS / dist).min(1.0).asin();
            if forward.dot(dir).clamp(-1.0, 1.0).acos() > tolerance {
                continue;
            }
            if statics.ray_cast(eye, dir, surface).is_some() {
                continue;
            }
        }
        if best.is_none_or(|b| surface < b.distance) {
            let dir = if dist > 1e-6 { to / dist } else { forward };
            best = Some(PlayerContact {
                target: *id,
                zone: HitZone::Body,
                distance: surface,
                point: eye + dir * surface,
            });
        }
    }
    best
}

/// Swings a melee weapon once if the rules allow it. `targets` are the
/// other players as the shooter saw them (see [`rewound_hitboxes`]; a
/// client passes the entities it displays); `input.origin` is the
/// shooter's eye.
///
/// # Errors
/// [`ShotReject`] when the weapon cannot swing at `input.now`.
pub fn melee_attack(
    def: &WeaponDef,
    weapon: &mut WeaponState,
    input: &ShotInput,
    targets: &[(NetId, Hitbox)],
    statics: &StaticWorld,
) -> Result<MeleeReport, ShotReject> {
    let (Some(melee), WeaponKind::Melee) = (&def.melee, def.kind) else {
        return Err(ShotReject::NotMelee);
    };
    let (shot_index, _) = begin_shot(def, weapon, input, false)?;
    let contact = pick(
        input.origin,
        input.basis.forward,
        melee.range,
        melee.arc,
        input.shooter,
        targets,
        statics,
    );
    let damage = contact.map(|c| DamageEvent {
        attacker: input.shooter,
        victim: c.target,
        amount: pellet_damage(&def.damage, c.distance, c.zone),
        kind: DamageType::Melee,
        zone: c.zone,
        position: c.point,
    });
    Ok(MeleeReport {
        shot_index,
        contact,
        damage,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spread::{AimBasis, Stance};
    use faset_sim::Volume;

    const ME: NetId = NetId(1);

    fn knife() -> WeaponDef {
        WeaponDef::builtin().remove(5)
    }

    fn input(now: f64, basis: AimBasis) -> ShotInput {
        ShotInput {
            now,
            seed: 1,
            shooter: ME,
            origin: Vec3::new(0.0, 1.6, 0.0),
            basis,
            stance: Stance::default(),
            fresh_press: true,
        }
    }

    fn ahead(id: u16, x: f32, z: f32) -> (NetId, Hitbox) {
        (NetId(id), Hitbox::standing(Vec3::new(x, 0.9, z)))
    }

    fn forward() -> AimBasis {
        AimBasis::from_quantized(0, 0)
    }

    fn none() -> StaticWorld {
        StaticWorld::new(vec![])
    }

    #[test]
    fn hits_in_range_and_arc_with_melee_damage() {
        let def = knife();
        let mut w = WeaponState::new(&def, 0.0);
        let r = melee_attack(
            &def,
            &mut w,
            &input(1.0, forward()),
            &[ahead(2, 0.0, -1.5)],
            &none(),
        )
        .unwrap();
        let hit = r.contact.unwrap();
        assert_eq!(hit.target, NetId(2));
        assert!((hit.distance - (1.5 - 0.35)).abs() < 0.05);
        let d = r.damage.unwrap();
        assert_eq!(d.kind, DamageType::Melee);
        assert_eq!(d.amount, 45.0);
        // Melee spends no ammo.
        assert_eq!(w.ammo, def.magazine);
    }

    #[test]
    fn misses_out_of_range_out_of_arc_and_behind_walls() {
        let def = knife();
        let far = [ahead(2, 0.0, -3.0)];
        let side = [ahead(2, 2.0, 0.0)];
        let near = [ahead(2, 0.0, -1.5)];
        let wall = StaticWorld::new(vec![Volume::cuboid(
            Vec3::new(0.0, 1.0, -0.8),
            Vec3::new(3.0, 3.0, 0.05),
        )]);
        for (targets, statics) in [(&far[..], none()), (&side[..], none()), (&near[..], wall)] {
            let mut w = WeaponState::new(&def, 0.0);
            let r = melee_attack(&def, &mut w, &input(1.0, forward()), targets, &statics).unwrap();
            assert_eq!(r.contact, None);
            assert_eq!(r.damage, None);
        }
    }

    #[test]
    fn nearest_wins_and_the_shooter_is_ignored() {
        let def = knife();
        let mut w = WeaponState::new(&def, 0.0);
        let targets = [
            ahead(3, 0.0, -1.8),
            ahead(2, 0.0, -1.2),
            (ME, Hitbox::standing(Vec3::new(0.0, 0.9, 0.0))),
        ];
        let r = melee_attack(&def, &mut w, &input(1.0, forward()), &targets, &none()).unwrap();
        assert_eq!(r.contact.unwrap().target, NetId(2));
    }

    #[test]
    fn cooldown_follows_rpm_and_wrong_kind_is_refused() {
        let def = knife();
        let mut w = WeaponState::new(&def, 0.0);
        let t = [ahead(2, 0.0, -1.0)];
        melee_attack(&def, &mut w, &input(1.0, forward()), &t, &none()).unwrap();
        // 120 rpm: 0.5 s between swings.
        assert!(matches!(
            melee_attack(&def, &mut w, &input(1.3, forward()), &t, &none()),
            Err(ShotReject::Cadence { .. })
        ));
        assert!(melee_attack(&def, &mut w, &input(1.5, forward()), &t, &none()).is_ok());
        let rifle = WeaponDef::builtin().remove(0);
        let mut rw = WeaponState::new(&rifle, 0.0);
        assert_eq!(
            melee_attack(&rifle, &mut rw, &input(1.0, forward()), &t, &none()),
            Err(ShotReject::NotMelee)
        );
    }

    #[test]
    fn rewound_hitboxes_place_targets_where_the_shooter_saw_them() {
        let mut lag = LagCompensation::new();
        for t in 0..30u32 {
            // Player 2 walks along +X at 0.1 m per tick.
            lag.record(
                Tick(t),
                [
                    (ME, Hitbox::standing(Vec3::new(0.0, 0.9, 0.0))),
                    (
                        NetId(2),
                        Hitbox::standing(Vec3::new(t as f32 * 0.1, 0.9, -1.5)),
                    ),
                ],
            );
        }
        // The shooter saw tick 20 (9 ticks, 0.15 s ago), rtt 0.1 s.
        let seen = rewound_hitboxes(&lag, ME, [ME, NetId(2)], Tick(20), 0.0, 0.1);
        assert_eq!(seen.len(), 1);
        assert!((seen[0].1.position.x - 2.0).abs() < 1e-4);
    }
}

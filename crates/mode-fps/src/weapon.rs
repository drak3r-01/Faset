//! Weapon data: [`WeaponDef`], one RON file per weapon.
//!
//! Angles are radians, distances metres, times seconds. Loading validates
//! the numbers so the shot pipeline never meets a division by zero or a
//! cone wider than a hemisphere.

use serde::{Deserialize, Serialize};

/// What a weapon fires. [`Hitscan`](Self::Hitscan) goes through
/// [`fire`](crate::shot::fire), [`Projectile`](Self::Projectile) through
/// [`launch`](crate::projectile::launch) and [`Melee`](Self::Melee) through
/// [`melee_attack`](crate::melee::melee_attack); `Beam` is only named.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WeaponKind {
    Hitscan,
    Projectile,
    Melee,
    Beam,
}

/// How the trigger works.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FireMode {
    /// Fires at the cadence while the trigger is held.
    Auto,
    /// One shot per press.
    Semi,
    /// [`WeaponDef::burst_count`] shots per press, at the cadence.
    Burst,
}

/// Damage of one pellet: base value, head multiplier and distance falloff.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DamageDef {
    /// D0, at or before the falloff start.
    pub base: f32,
    /// Multiplier of a head hit.
    pub head_multiplier: f32,
    /// r0: distance where the falloff starts, metres.
    pub falloff_start: f32,
    /// r1: distance where the falloff ends, metres.
    pub falloff_end: f32,
    /// m_min: damage multiplier from r1 on, in `0..=1`.
    pub min_multiplier: f32,
}

/// Spread cone, in radians.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpreadDef {
    /// sigma_base: cone half angle at rest.
    pub sigma_base: f32,
    /// sigma_max: widest cone.
    pub sigma_max: f32,
    /// delta_beta: bloom added per shot.
    pub delta_beta: f32,
    /// tau: time constant of the bloom return, seconds.
    pub tau: f32,
    /// Multiplier of `sigma_base` while moving.
    pub move_multiplier: f32,
    /// Multiplier of `sigma_base` while airborne.
    pub air_multiplier: f32,
}

/// Recoil: a per-shot view kick and the spring that returns the view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecoilDef {
    /// View kick of consecutive shots as `(yaw, pitch)` radians (positive
    /// pitch looks up). The last entry repeats.
    pub pattern: Vec<(f32, f32)>,
    /// Fraction of each kick that is random, in `0..=1`.
    pub random_fraction: f32,
    /// omega: stiffness of the critically damped return spring, rad/s.
    pub omega: f32,
}

/// Aiming down the sights.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SightDef {
    /// Vertical field of view while sighted, radians.
    pub fov: f32,
    /// Multiplier of the cone while sighted.
    pub spread_multiplier: f32,
    /// Multiplier of the movement speed while sighted.
    pub speed_multiplier: f32,
}

fn default_radius() -> f32 {
    0.1
}

fn default_restitution() -> f32 {
    0.5
}

fn default_exponent() -> f32 {
    1.5
}

/// Projectile block (lot 4.5). Per tick the projectile accelerates by
/// `a = gravity * g - drag * |v| * v` (g being the world's gravity vector).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectileDef {
    /// Muzzle speed, m/s.
    pub speed: f32,
    /// k_g: gravity multiplier.
    pub gravity: f32,
    /// Lifetime, seconds.
    pub lifetime: f32,
    /// k_d: quadratic drag coefficient, 1/m (0 = none).
    #[serde(default)]
    pub drag: f32,
    /// Radius of the swept sphere, metres.
    #[serde(default = "default_radius")]
    pub radius: f32,
    /// Number of bounces on walls before the projectile detonates.
    #[serde(default)]
    pub bounces: u32,
    /// Fraction of the speed kept by a bounce, in `0..=1` (decision: the
    /// plan gives none, 0.5 by default).
    #[serde(default = "default_restitution")]
    pub restitution: f32,
    /// Blast released when the projectile ends (impact, last bounce or
    /// timeout). Without it a player contact deals `damage.base` directly.
    #[serde(default)]
    pub explosion: Option<ExplosionDef>,
}

/// Area blast: damage `D0 * (1 - d/R)^p` and knockback `K * (1 - d/R)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExplosionDef {
    /// R: blast radius, metres.
    pub radius: f32,
    /// D0: damage at the centre.
    pub damage: f32,
    /// p: falloff exponent of the damage (decision: 1.5 by default).
    #[serde(default = "default_exponent")]
    pub exponent: f32,
    /// K: knockback speed at the centre, m/s.
    pub knockback: f32,
}

/// Melee block (lot 4.5): a short-range arc in front of the eye.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeleeDef {
    /// Reach from the eye to the surface of the target, metres.
    pub range: f32,
    /// Full opening angle of the arc, radians.
    pub arc: f32,
}

/// Status effect a quirky gadget inflicts on a hit player (lot 4.6). Applying
/// it is up to the mode; the data only names it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum EffectDef {
    /// Forces the victim into a slide.
    Slide { duration: f32 },
    /// Blinds the victim.
    Blind { duration: f32 },
    /// Multiplies the victim's speed by `factor` in `0..=1`.
    Slow { duration: f32, factor: f32 },
}

fn default_burst_count() -> u32 {
    3
}

fn default_range() -> f32 {
    300.0
}

/// A weapon, as loaded from its RON file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WeaponDef {
    /// Unique key.
    pub id: String,
    pub kind: WeaponKind,
    pub fire_mode: FireMode,
    /// Shots per burst, for [`FireMode::Burst`] (decision: 3 by default).
    #[serde(default = "default_burst_count")]
    pub burst_count: u32,
    /// Rounds per minute.
    pub rpm: f32,
    /// Pellets per shot, each with its own direction.
    pub pellets: u32,
    pub damage: DamageDef,
    /// Rounds in a full magazine.
    pub magazine: u32,
    /// Rounds carried outside the magazine.
    pub reserve: u32,
    /// Seconds to reload.
    pub reload_time: f32,
    /// Seconds before the weapon can fire once drawn.
    pub draw_time: f32,
    /// Longest ray, metres (decision: not in the plan, 300 m default).
    #[serde(default = "default_range")]
    pub range: f32,
    pub spread: SpreadDef,
    pub recoil: RecoilDef,
    pub sight: SightDef,
    /// Required for [`WeaponKind::Projectile`].
    #[serde(default)]
    pub projectile: Option<ProjectileDef>,
    /// Required for [`WeaponKind::Melee`]; the cooldown is `60 / rpm`.
    #[serde(default)]
    pub melee: Option<MeleeDef>,
    /// Status effect inflicted on a hit player (decision: new optional block).
    #[serde(default)]
    pub effect: Option<EffectDef>,
}

/// A weapon file that cannot be used.
#[derive(Debug, thiserror::Error)]
pub enum WeaponError {
    /// Not valid RON for [`WeaponDef`].
    #[error("weapon file: {0}")]
    Parse(#[from] ron::error::SpannedError),
    /// A value out of its range.
    #[error("weapon `{id}`: {reason}")]
    Invalid { id: String, reason: &'static str },
}

impl WeaponDef {
    /// Parses and validates a RON weapon file.
    ///
    /// # Errors
    /// Malformed RON or an out-of-range value.
    pub fn from_ron(text: &str) -> Result<Self, WeaponError> {
        let def: Self = ron::from_str(text)?;
        def.validate()?;
        Ok(def)
    }

    /// Seconds between two shots: `60 / rpm`.
    #[must_use]
    pub fn interval(&self) -> f64 {
        60.0 / f64::from(self.rpm)
    }

    /// Checks every number the pipeline divides by or clamps against.
    ///
    /// # Errors
    /// The first value out of range.
    pub fn validate(&self) -> Result<(), WeaponError> {
        let bad = |reason| {
            Err(WeaponError::Invalid {
                id: self.id.clone(),
                reason,
            })
        };
        let finite_pos = |v: f32| v.is_finite() && v > 0.0;
        if !finite_pos(self.rpm) {
            return bad("rpm must be positive");
        }
        if self.pellets == 0 || self.magazine == 0 || self.burst_count == 0 {
            return bad("pellets, magazine and burst_count must be at least 1");
        }
        if !finite_pos(self.range) {
            return bad("range must be positive");
        }
        let d = &self.damage;
        if !(d.base.is_finite() && d.base >= 0.0 && d.head_multiplier >= 0.0) {
            return bad("damage base and head multiplier must be non-negative");
        }
        if !(d.falloff_start >= 0.0 && d.falloff_end >= d.falloff_start) {
            return bad("falloff needs 0 <= start <= end");
        }
        if !(0.0..=1.0).contains(&d.min_multiplier) {
            return bad("min_multiplier must be in 0..=1");
        }
        let s = &self.spread;
        if !(s.sigma_base >= 0.0 && s.sigma_max >= s.sigma_base && s.sigma_max < 1.5) {
            return bad("spread needs 0 <= sigma_base <= sigma_max < 1.5 rad");
        }
        if !(s.delta_beta >= 0.0 && finite_pos(s.tau)) {
            return bad("spread delta_beta must be >= 0 and tau > 0");
        }
        if !(s.move_multiplier >= 0.0 && s.air_multiplier >= 0.0) {
            return bad("spread multipliers must be non-negative");
        }
        if !finite_pos(self.recoil.omega) || !(0.0..=1.0).contains(&self.recoil.random_fraction) {
            return bad("recoil omega must be positive and random_fraction in 0..=1");
        }
        if self.recoil.pattern.is_empty() {
            return bad("recoil pattern needs at least one entry");
        }
        if !(self.sight.spread_multiplier >= 0.0 && finite_pos(self.sight.fov)) {
            return bad("sight fov must be positive, spread multiplier non-negative");
        }
        if !(self.reload_time >= 0.0 && self.draw_time >= 0.0) {
            return bad("reload and draw times must be non-negative");
        }
        match (self.kind, &self.projectile, &self.melee) {
            (WeaponKind::Projectile, None, _) => {
                return bad("projectile weapon needs a projectile block")
            }
            (WeaponKind::Melee, _, None) => return bad("melee weapon needs a melee block"),
            _ => {}
        }
        if let Some(p) = &self.projectile {
            if !(finite_pos(p.speed) && finite_pos(p.lifetime) && finite_pos(p.radius)) {
                return bad("projectile speed, lifetime and radius must be positive");
            }
            if !(p.gravity.is_finite() && p.drag.is_finite() && p.drag >= 0.0) {
                return bad("projectile gravity must be finite and drag non-negative");
            }
            if !(0.0..=1.0).contains(&p.restitution) {
                return bad("projectile restitution must be in 0..=1");
            }
            if let Some(e) = &p.explosion {
                if !(finite_pos(e.radius) && finite_pos(e.exponent)) {
                    return bad("explosion radius and exponent must be positive");
                }
                let ok = |v: f32| v.is_finite() && v >= 0.0;
                if !(ok(e.damage) && ok(e.knockback)) {
                    return bad("explosion damage and knockback must be non-negative");
                }
            }
        }
        if let Some(e) = &self.effect {
            let (duration, factor) = match *e {
                EffectDef::Slide { duration } | EffectDef::Blind { duration } => (duration, 1.0),
                EffectDef::Slow { duration, factor } => (duration, factor),
            };
            if !(finite_pos(duration) && (0.0..=1.0).contains(&factor)) {
                return bad("effect duration must be positive and slow factor in 0..=1");
            }
        }
        if let Some(m) = &self.melee {
            if !(finite_pos(m.range) && finite_pos(m.arc) && m.arc <= std::f32::consts::TAU) {
                return bad("melee range must be positive and arc in (0, 2 pi]");
            }
        }
        Ok(())
    }

    /// The weapons shipped with the crate: rifle, pistol, shotgun, rocket
    /// launcher, grenade launcher and knife (decision: example values, the class and loadout numbers belong to
    /// lot 4.6).
    ///
    /// # Panics
    /// Never: the embedded files are covered by tests.
    #[must_use]
    pub fn builtin() -> Vec<Self> {
        [
            include_str!("../weapons/rifle.ron"),
            include_str!("../weapons/pistol.ron"),
            include_str!("../weapons/shotgun.ron"),
            include_str!("../weapons/rocket_launcher.ron"),
            include_str!("../weapons/grenade_launcher.ron"),
            include_str!("../weapons/knife.ron"),
        ]
        .into_iter()
        .map(|t| Self::from_ron(t).expect("embedded weapon file is valid"))
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_weapons_load_and_are_valid() {
        let all = WeaponDef::builtin();
        assert_eq!(all.len(), 6);
        assert!((all[0].interval() - 0.1).abs() < 1e-9);
        assert_eq!(all[2].pellets, 8);
        assert!(all[..3].iter().all(|w| w.kind == WeaponKind::Hitscan));
        assert_eq!(all[3].kind, WeaponKind::Projectile);
        assert_eq!(all[5].kind, WeaponKind::Melee);
    }

    #[test]
    fn invalid_values_are_rejected() {
        let mut w = WeaponDef::builtin().remove(0);
        w.rpm = 0.0;
        assert!(matches!(w.validate(), Err(WeaponError::Invalid { .. })));
        let mut w = WeaponDef::builtin().remove(0);
        w.damage.falloff_end = 1.0;
        assert!(w.validate().is_err());
        assert!(matches!(
            WeaponDef::from_ron("(nonsense"),
            Err(WeaponError::Parse(_))
        ));
    }

    #[test]
    fn round_trips_through_ron() {
        let w = WeaponDef::builtin().remove(1);
        let text = ron::to_string(&w).unwrap();
        assert_eq!(WeaponDef::from_ron(&text).unwrap(), w);
    }
}

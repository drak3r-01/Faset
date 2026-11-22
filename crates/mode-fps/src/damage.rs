//! Damage events, falloff and the damage chain.
//!
//! Chain, in order: armour absorbs first, then the class multiplier, then
//! the active-bonus multiplier; friendly fire (off in team modes) and spawn
//! protection (1.5 s) cancel the whole event before anything is consumed.
//! Health decreasing to 0 kills; the respawn follows 3 s later.

use faset_core::glam::Vec3;
use faset_core::NetId;

use crate::weapon::DamageDef;

/// Seconds of invulnerability after a respawn.
pub const SPAWN_PROTECTION: f64 = 1.5;
/// Seconds between a death and the respawn.
pub const RESPAWN_DELAY: f64 = 3.0;
/// Health of a fresh player (decision: lot 4.6's classes will override).
pub const DEFAULT_HEALTH: f32 = 100.0;

/// Where a shot landed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitZone {
    Head,
    Body,
}

/// What caused the damage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DamageType {
    Bullet,
    Explosion,
    Melee,
    Beam,
}

/// One packet of damage, before the chain.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DamageEvent {
    pub attacker: NetId,
    pub victim: NetId,
    pub amount: f32,
    pub kind: DamageType,
    pub zone: HitZone,
    /// Where the shot landed.
    pub position: Vec3,
}

/// Distance multiplier: `mix(1, m_min, clamp((r - r0) / (r1 - r0), 0, 1))`.
/// With `r1 == r0` the falloff is a step at that distance.
#[must_use]
pub fn falloff_multiplier(def: &DamageDef, distance: f32) -> f32 {
    let span = def.falloff_end - def.falloff_start;
    let t = if span > 0.0 {
        ((distance - def.falloff_start) / span).clamp(0.0, 1.0)
    } else if distance >= def.falloff_end {
        1.0
    } else {
        0.0
    };
    1.0 + (def.min_multiplier - 1.0) * t
}

/// Damage of one pellet landing at `distance` in `zone`.
#[must_use]
pub fn pellet_damage(def: &DamageDef, distance: f32, zone: HitZone) -> f32 {
    let head = if zone == HitZone::Head {
        def.head_multiplier
    } else {
        1.0
    };
    def.base * falloff_multiplier(def, distance) * head
}

/// What the chain needs to know about the two players and the rules.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DamageContext {
    /// Victim class multiplier (lot 4.6 supplies the values).
    pub class_multiplier: f32,
    /// Active bonuses multiplier (lot 4.7 supplies the values).
    pub bonus_multiplier: f32,
    /// Attacker and victim are different players of the same team.
    pub same_team: bool,
    /// Friendly fire is on.
    pub friendly_fire: bool,
}

impl Default for DamageContext {
    fn default() -> Self {
        Self {
            class_multiplier: 1.0,
            bonus_multiplier: 1.0,
            same_team: false,
            friendly_fire: false,
        }
    }
}

/// Why the chain applied nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ignored {
    Dead,
    FriendlyFire,
    SpawnProtected,
}

/// Result of [`Combatant::apply`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DamageOutcome {
    Ignored(Ignored),
    Applied {
        /// Taken from health.
        health_damage: f32,
        /// Absorbed by armour.
        armour_absorbed: f32,
        /// This event killed the victim.
        killed: bool,
    },
}

/// Health, armour and the timers of one player.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Combatant {
    pub health: f32,
    pub max_health: f32,
    pub armour: f32,
    /// No damage before this time.
    pub protected_until: f64,
    /// Respawn time of a dead player.
    pub respawn_at: Option<f64>,
}

impl Combatant {
    /// A player spawning at `now` with full health and spawn protection.
    #[must_use]
    pub fn spawned(now: f64, max_health: f32) -> Self {
        Self {
            health: max_health,
            max_health,
            armour: 0.0,
            protected_until: now + SPAWN_PROTECTION,
            respawn_at: None,
        }
    }

    #[must_use]
    pub fn alive(&self) -> bool {
        self.respawn_at.is_none()
    }

    /// Whether spawn protection still holds at `now`.
    #[must_use]
    pub fn protected(&self, now: f64) -> bool {
        now < self.protected_until
    }

    /// Runs the chain for `event` at `now`.
    pub fn apply(&mut self, event: &DamageEvent, ctx: &DamageContext, now: f64) -> DamageOutcome {
        if !self.alive() {
            return DamageOutcome::Ignored(Ignored::Dead);
        }
        if ctx.same_team && !ctx.friendly_fire {
            return DamageOutcome::Ignored(Ignored::FriendlyFire);
        }
        if self.protected(now) {
            return DamageOutcome::Ignored(Ignored::SpawnProtected);
        }
        let mut amount = event.amount.max(0.0);
        let armour_absorbed = amount.min(self.armour);
        self.armour -= armour_absorbed;
        amount -= armour_absorbed;
        amount *= ctx.class_multiplier * ctx.bonus_multiplier;
        let health_damage = amount.min(self.health);
        self.health -= health_damage;
        let killed = self.health <= 0.0;
        if killed {
            self.respawn_at = Some(now + RESPAWN_DELAY);
        }
        DamageOutcome::Applied {
            health_damage,
            armour_absorbed,
            killed,
        }
    }

    /// Whether a dead player is due to respawn at `now`.
    #[must_use]
    pub fn respawn_due(&self, now: f64) -> bool {
        self.respawn_at.is_some_and(|t| now >= t)
    }

    /// Respawns at `now`: full health, no armour, spawn protection.
    pub fn respawn(&mut self, now: f64) {
        *self = Self::spawned(now, self.max_health);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def() -> DamageDef {
        DamageDef {
            base: 20.0,
            head_multiplier: 2.0,
            falloff_start: 10.0,
            falloff_end: 30.0,
            min_multiplier: 0.5,
        }
    }

    fn event(amount: f32) -> DamageEvent {
        DamageEvent {
            attacker: NetId(1),
            victim: NetId(2),
            amount,
            kind: DamageType::Bullet,
            zone: HitZone::Body,
            position: Vec3::ZERO,
        }
    }

    #[test]
    fn falloff_at_r0_r1_and_beyond() {
        let d = def();
        assert_eq!(falloff_multiplier(&d, 0.0), 1.0);
        assert_eq!(falloff_multiplier(&d, 10.0), 1.0);
        assert!((falloff_multiplier(&d, 20.0) - 0.75).abs() < 1e-6);
        assert_eq!(falloff_multiplier(&d, 30.0), 0.5);
        assert_eq!(falloff_multiplier(&d, 500.0), 0.5);
        assert_eq!(pellet_damage(&d, 10.0, HitZone::Body), 20.0);
        assert_eq!(pellet_damage(&d, 10.0, HitZone::Head), 40.0);
        assert_eq!(pellet_damage(&d, 100.0, HitZone::Body), 10.0);
    }

    #[test]
    fn degenerate_falloff_is_a_step() {
        let mut d = def();
        d.falloff_end = d.falloff_start;
        assert_eq!(falloff_multiplier(&d, 9.9), 1.0);
        assert_eq!(falloff_multiplier(&d, 10.0), 0.5);
    }

    #[test]
    fn armour_absorbs_first_then_multipliers_hit_the_rest() {
        let mut c = Combatant::spawned(0.0, 100.0);
        c.armour = 15.0;
        let ctx = DamageContext {
            class_multiplier: 0.5,
            bonus_multiplier: 2.0,
            ..DamageContext::default()
        };
        // 40 damage: 15 absorbed, 25 * 0.5 * 2.0 = 25 to health.
        let out = c.apply(&event(40.0), &ctx, 2.0);
        assert_eq!(
            out,
            DamageOutcome::Applied {
                health_damage: 25.0,
                armour_absorbed: 15.0,
                killed: false
            }
        );
        assert_eq!((c.health, c.armour), (75.0, 0.0));
    }

    #[test]
    fn spawn_protection_blocks_damage_for_one_and_a_half_seconds() {
        let mut c = Combatant::spawned(10.0, 100.0);
        let ctx = DamageContext::default();
        for t in [10.0, 11.0, 11.49] {
            assert_eq!(
                c.apply(&event(50.0), &ctx, t),
                DamageOutcome::Ignored(Ignored::SpawnProtected)
            );
        }
        assert_eq!(c.health, 100.0);
        assert!(matches!(
            c.apply(&event(50.0), &ctx, 11.5),
            DamageOutcome::Applied { .. }
        ));
        assert_eq!(c.health, 50.0);
    }

    #[test]
    fn friendly_fire_is_off_unless_enabled_and_consumes_nothing() {
        let mut c = Combatant::spawned(0.0, 100.0);
        c.armour = 10.0;
        let mut ctx = DamageContext {
            same_team: true,
            ..DamageContext::default()
        };
        assert_eq!(
            c.apply(&event(30.0), &ctx, 5.0),
            DamageOutcome::Ignored(Ignored::FriendlyFire)
        );
        assert_eq!((c.health, c.armour), (100.0, 10.0));
        ctx.friendly_fire = true;
        assert!(matches!(
            c.apply(&event(30.0), &ctx, 5.0),
            DamageOutcome::Applied { .. }
        ));
    }

    #[test]
    fn death_then_respawn_after_three_seconds() {
        let mut c = Combatant::spawned(0.0, 100.0);
        let ctx = DamageContext::default();
        let out = c.apply(&event(150.0), &ctx, 20.0);
        assert!(matches!(
            out,
            DamageOutcome::Applied {
                health_damage,
                killed: true,
                ..
            } if health_damage == 100.0
        ));
        assert!(!c.alive());
        assert_eq!(
            c.apply(&event(1.0), &ctx, 21.0),
            DamageOutcome::Ignored(Ignored::Dead)
        );
        assert!(!c.respawn_due(22.99));
        assert!(c.respawn_due(23.0));
        c.respawn(23.0);
        assert!(c.alive() && c.protected(24.0) && !c.protected(24.5));
        assert_eq!(c.health, 100.0);
    }
}

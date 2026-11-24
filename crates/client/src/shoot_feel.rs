//! The feel of shooting (lot 6.9): the weapon's recoil curve, the camera
//! shake of shots and blasts, and the direction of incoming damage.
//!
//! Everything is derived from data that already exists: the recoil of the
//! view model comes from the weapon's `recoil` block, its rate of fire and its
//! kind. No gameplay value is added.

use faset_assets::viewmodel::RecoilPattern;
use faset_core::glam::{EulerRot, Quat, Vec3};
use faset_mode_fps::{WeaponDef, WeaponKind};

/// Weapon rise per radian of view kick: the view kick of a rifle is about
/// 0.7 degrees, too small to see on the weapon, so the weapon moves further.
const RISE_PER_RAD: f32 = 2.0;
/// Sideways travel of the weapon per radian of yaw kick.
const SIDE_PER_RAD: f32 = 2.0;
/// Base kick back towards the player, metres.
const BACK_BASE: f32 = 0.02;
/// Extra kick back per radian of view kick, metres.
const BACK_PER_RAD: f32 = 2.0;
/// Longest kick back, metres.
const BACK_MAX: f32 = 0.1;
/// Kick of a melee swing: `(side, rise, back)`.
const MELEE_KICK: [f32; 3] = [0.0, 0.04, 0.09];

/// The recoil curve of the view model for a weapon: the kick of each shot of
/// a burst (from the weapon's own pattern), the spring that returns it (the
/// weapon's `omega`) and the time without a shot after which the burst
/// restarts (one and a half shot intervals, between 0.2 and 0.6 s).
///
/// The kick is `(side, rise, back)` in the units of
/// [`RecoilPattern`]: metres for `side` and `back`, `rise` as muzzle climb.
#[must_use]
pub fn recoil_pattern(def: &WeaponDef) -> RecoilPattern {
    let kick = |yaw: f32, pitch: f32| -> [f32; 3] {
        if def.kind == WeaponKind::Melee {
            return MELEE_KICK;
        }
        [
            yaw * SIDE_PER_RAD,
            pitch * RISE_PER_RAD,
            (BACK_BASE + pitch.abs() * BACK_PER_RAD).min(BACK_MAX),
        ]
    };
    let kicks: Vec<[f32; 3]> = if def.recoil.pattern.is_empty() {
        vec![kick(0.0, 0.0)]
    } else {
        def.recoil
            .pattern
            .iter()
            .map(|&(y, p)| kick(y, p))
            .collect()
    };
    let f = def.recoil.random_fraction.clamp(0.0, 1.0);
    let widest = kicks.iter().fold([0.0f32; 3], |m, k| {
        [
            m[0].max(k[0].abs()),
            m[1].max(k[1].abs()),
            m[2].max(k[2].abs()),
        ]
    });
    RecoilPattern {
        kicks,
        random: [widest[0] * f, widest[1] * f, widest[2] * f * 0.5],
        return_omega: def.recoil.omega.max(1.0),
        reset_after: ((def.interval() * 1.5) as f32).clamp(0.2, 0.6),
    }
}

/// Camera shake added by one shot of `def`: grows with the view kick.
#[must_use]
pub fn shot_trauma(def: &WeaponDef) -> f32 {
    let pitch = def
        .recoil
        .pattern
        .first()
        .map_or(0.0, |&(_, pitch)| pitch.abs());
    if def.kind == WeaponKind::Melee {
        return 0.0;
    }
    (0.06 + pitch * 4.0).min(0.35)
}

/// Shake added by a blast `distance` metres from the camera: full at the
/// centre, nothing beyond [`BLAST_SHAKE_RANGE`].
#[must_use]
pub fn blast_trauma(distance: f32) -> f32 {
    (0.9 * (1.0 - distance / BLAST_SHAKE_RANGE)).clamp(0.0, 0.9)
}

/// Distance beyond which a blast does not shake the camera, metres.
pub const BLAST_SHAKE_RANGE: f32 = 30.0;
/// Trauma lost per second.
const TRAUMA_DECAY: f32 = 1.6;
/// Largest rotation of a full shake, radians (about 1.7 degrees).
const SHAKE_MAX_ANGLE: f32 = 0.03;

/// Camera shake with the trauma model: shots and blasts add trauma in `0..=1`,
/// it decays linearly, and the rotation grows with its square so small
/// shakes are subtle and large ones strong.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CameraShake {
    trauma: f32,
    time: f32,
}

impl CameraShake {
    /// Adds trauma (clamped to 1).
    pub fn add(&mut self, trauma: f32) {
        self.trauma = (self.trauma + trauma.max(0.0)).min(1.0);
    }

    /// Current trauma, 0 to 1.
    #[must_use]
    pub fn trauma(&self) -> f32 {
        self.trauma
    }

    /// Advances by `dt` seconds.
    pub fn update(&mut self, dt: f32) {
        self.time += dt;
        self.trauma = (self.trauma - TRAUMA_DECAY * dt).max(0.0);
    }

    /// The rotation to apply to the camera: a smooth pseudo-random yaw, pitch
    /// and roll (sums of sines at unrelated rates), zero without trauma.
    #[must_use]
    pub fn rotation(&self) -> Quat {
        let k = self.trauma * self.trauma * SHAKE_MAX_ANGLE;
        if k <= 0.0 {
            return Quat::IDENTITY;
        }
        let t = self.time;
        let wave = |a: f32, b: f32| 0.6 * (a * t).sin() + 0.4 * (b * t + 1.3).sin();
        Quat::from_euler(
            EulerRot::YXZ,
            k * wave(37.0, 61.0),
            k * wave(43.0, 71.0),
            k * 0.6 * wave(31.0, 53.0),
        )
    }
}

/// Seconds a damage indicator stays.
pub const DAMAGE_SECONDS: f32 = 1.6;
/// Indicators kept.
pub const MAX_DAMAGE_ARCS: usize = 8;

/// One direction of incoming damage, for the HUD.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DamageArc {
    /// Where the damage came from relative to where the player looks:
    /// 0 ahead, positive to the right, radians in `(-pi, pi]`.
    pub angle: f32,
    /// Opacity, 1 when fresh and 0 when over.
    pub alpha: f32,
}

/// The sources of the damage the local player took lately.
#[derive(Debug, Clone, Default)]
pub struct DamageIndicators {
    /// World position of the attacker and age.
    sources: Vec<(Vec3, f32)>,
}

impl DamageIndicators {
    /// Remembers an attacker at `from`.
    pub fn push(&mut self, from: Vec3) {
        if self.sources.len() == MAX_DAMAGE_ARCS {
            self.sources.remove(0);
        }
        self.sources.push((from, 0.0));
    }

    /// Ages the indicators by `dt` seconds.
    pub fn update(&mut self, dt: f32) {
        for s in &mut self.sources {
            s.1 += dt;
        }
        self.sources.retain(|s| s.1 < DAMAGE_SECONDS);
    }

    /// Indicators alive.
    #[must_use]
    pub fn len(&self) -> usize {
        self.sources.len()
    }

    /// Whether none is shown.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }

    /// The arcs to draw for a player at `eye` looking along `forward`.
    #[must_use]
    pub fn arcs(&self, eye: Vec3, forward: Vec3) -> Vec<DamageArc> {
        let f = Vec3::new(forward.x, 0.0, forward.z)
            .try_normalize()
            .unwrap_or(Vec3::NEG_Z);
        let right = Vec3::new(-f.z, 0.0, f.x);
        self.sources
            .iter()
            .filter_map(|&(from, age)| {
                let to = Vec3::new(from.x - eye.x, 0.0, from.z - eye.z);
                let angle = if to.length_squared() < 1e-4 {
                    0.0
                } else {
                    to.dot(right).atan2(to.dot(f))
                };
                let alpha = (1.0 - age / DAMAGE_SECONDS).clamp(0.0, 1.0);
                (alpha > 0.0).then_some(DamageArc { angle, alpha })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use faset_assets::viewmodel::{Viewmodel, ViewmodelInputs};

    fn weapon(id: &str) -> WeaponDef {
        let text = match id {
            "assault_rifle" => include_str!("../../mode-fps/data/weapons/assault_rifle.ron"),
            "pump_shotgun" => include_str!("../../mode-fps/data/weapons/pump_shotgun.ron"),
            "smg" => include_str!("../../mode-fps/data/weapons/smg.ron"),
            "baguette" => include_str!("../../mode-fps/data/weapons/baguette.ron"),
            _ => panic!("unknown test weapon"),
        };
        WeaponDef::from_ron(text).unwrap()
    }

    #[test]
    fn heavier_weapons_kick_harder_than_light_ones() {
        let first = |id: &str| recoil_pattern(&weapon(id)).kicks[0];
        let (rifle, smg, shotgun) = (first("assault_rifle"), first("smg"), first("pump_shotgun"));
        let (rifle, smg, shotgun) = (Vec3::from(rifle), Vec3::from(smg), Vec3::from(shotgun));
        assert!(
            smg.y < rifle.y && rifle.y < shotgun.y,
            "{smg} {rifle} {shotgun}"
        );
        assert!(smg.z < rifle.z && rifle.z < shotgun.z);
        assert!(shotgun.z <= BACK_MAX + 1e-6);
    }

    #[test]
    fn the_curve_follows_the_weapon_data() {
        let def = weapon("assault_rifle");
        let p = recoil_pattern(&def);
        assert_eq!(p.return_omega, def.recoil.omega);
        assert_eq!(p.kicks.len(), def.recoil.pattern.len());
        // 600 rpm: 0.1 s between shots, burst restarts after 0.2 s at least.
        assert!((0.2..=0.6).contains(&p.reset_after));
        let slow = recoil_pattern(&weapon("pump_shotgun"));
        assert!(
            slow.reset_after > p.reset_after,
            "slower weapons wait longer"
        );
        assert!(recoil_pattern(&weapon("smg")).reset_after <= p.reset_after);
    }

    #[test]
    fn a_melee_swing_kicks_even_with_no_view_kick() {
        let p = recoil_pattern(&weapon("baguette"));
        assert_eq!(p.kicks, vec![MELEE_KICK]);
        assert_eq!(shot_trauma(&weapon("baguette")), 0.0);
    }

    #[test]
    fn the_recoil_kicks_then_recovers_without_overshoot() {
        for id in ["assault_rifle", "pump_shotgun", "smg"] {
            let def = weapon(id);
            let pattern = recoil_pattern(&def);
            let idle = ViewmodelInputs {
                grounded: true,
                ..Default::default()
            };
            let mut vm = Viewmodel::new(3);
            vm.on_shot(&pattern);
            let kicked = vm.update(1.0 / 60.0, &idle);
            assert!(kicked.translation.z > 0.0, "{id} kicks back");
            let mut peak = kicked.translation.z;
            let mut last = peak;
            // Critically damped: after the peak the offset only shrinks.
            for _ in 0..120 {
                let t = vm.update(1.0 / 60.0, &idle);
                assert!(t.translation.z <= last + 1e-4, "{id} overshoots");
                last = t.translation.z;
                peak = peak.max(last);
            }
            assert!(last.abs() < 2e-3, "{id} recovers within 2 s: {last}");
            assert!(peak > 0.0);
        }
    }

    #[test]
    fn shot_trauma_grows_with_the_kick_and_is_capped() {
        let rifle = shot_trauma(&weapon("assault_rifle"));
        assert!(shot_trauma(&weapon("smg")) < rifle);
        assert!(rifle < shot_trauma(&weapon("pump_shotgun")));
        assert!(shot_trauma(&weapon("pump_shotgun")) <= 0.35);
    }

    #[test]
    fn a_blast_shakes_by_its_distance() {
        assert!((blast_trauma(0.0) - 0.9).abs() < 1e-6);
        assert!(blast_trauma(10.0) < blast_trauma(5.0));
        assert_eq!(blast_trauma(BLAST_SHAKE_RANGE), 0.0);
        assert_eq!(blast_trauma(500.0), 0.0);
    }

    #[test]
    fn the_shake_decays_to_nothing_and_stays_small() {
        let mut s = CameraShake::default();
        assert_eq!(s.rotation(), Quat::IDENTITY);
        s.add(0.8);
        s.add(0.8);
        assert_eq!(s.trauma(), 1.0, "capped");
        let mut biggest = 0.0f32;
        for _ in 0..200 {
            s.update(1.0 / 120.0);
            let (y, x, z) = s.rotation().to_euler(EulerRot::YXZ);
            biggest = biggest.max(y.abs()).max(x.abs()).max(z.abs());
        }
        assert!(
            biggest > 0.0 && biggest <= SHAKE_MAX_ANGLE * 1.01,
            "{biggest}"
        );
        for _ in 0..200 {
            s.update(1.0 / 120.0);
        }
        assert_eq!(s.trauma(), 0.0);
        assert_eq!(s.rotation(), Quat::IDENTITY);
    }

    #[test]
    fn damage_comes_from_the_side_it_hit() {
        let mut d = DamageIndicators::default();
        let eye = Vec3::new(1.0, 1.6, 1.0);
        let forward = Vec3::NEG_Z;
        for from in [
            Vec3::new(1.0, 1.0, -9.0), // ahead
            Vec3::new(11.0, 1.0, 1.0), // right
            Vec3::new(-9.0, 1.0, 1.0), // left
            Vec3::new(1.0, 1.0, 11.0), // behind
        ] {
            d.push(from);
        }
        let a = d.arcs(eye, forward);
        let pi = std::f32::consts::PI;
        assert!(a[0].angle.abs() < 1e-5);
        assert!((a[1].angle - pi / 2.0).abs() < 1e-5);
        assert!((a[2].angle + pi / 2.0).abs() < 1e-5);
        assert!((a[3].angle.abs() - pi).abs() < 1e-5);
        // Turning right by 90 degrees puts the right-hand attacker ahead.
        let a = d.arcs(eye, Vec3::X);
        assert!(a[1].angle.abs() < 1e-5);
    }

    #[test]
    fn damage_arcs_fade_and_are_bounded() {
        let mut d = DamageIndicators::default();
        for i in 0..20 {
            d.push(Vec3::new(i as f32, 0.0, 5.0));
        }
        assert_eq!(d.len(), MAX_DAMAGE_ARCS);
        d.update(DAMAGE_SECONDS / 2.0);
        let a = d.arcs(Vec3::ZERO, Vec3::NEG_Z);
        assert!(a.iter().all(|a| (a.alpha - 0.5).abs() < 1e-5));
        d.update(DAMAGE_SECONDS);
        assert!(d.is_empty());
    }
}

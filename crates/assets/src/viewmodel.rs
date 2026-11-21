//! First-person weapon animation: three critically damped springs (sway,
//! step, recoil) summed into one offset of the weapon from its rest place.
//!
//! The recoil kick is a pure function of the pattern, the shot index and a
//! seed ([`recoil_kick`]), so the client can predict it and the server
//! recompute it. [`Viewmodel::update`] integrates at the fixed step
//! [`FIXED_DT`] with an accumulator, so the result does not depend on the
//! frame rate. Units are metres and radians; the weapon points to -Z.
//!
//! Recoil kick vectors read `(side, rise, back)`: `side` and `back` are
//! translations in metres (`back` towards the player, +Z); `rise` is the
//! muzzle climb, applied as a pitch of `rise * RECOIL_PITCH_PER_UNIT` rad.
//! With a return stiffness `w >= 12` and a kick below 0.1 the offset is back
//! under `1e-3` within 1 s (`(1 + w t) e^(-w t)` decay).

use faset_core::glam::{EulerRot, Mat4, Quat, Vec3};
use serde::{Deserialize, Serialize};

use crate::anim::{Spring, FIXED_DT, RUN_SPEED};

/// Pitch (rad) produced by one unit of recoil `rise`.
pub const RECOIL_PITCH_PER_UNIT: f32 = 3.0;
/// Stiffness of the sway spring (rad/s).
pub const SWAY_OMEGA: f32 = 10.0;
/// Stiffness of the step spring (rad/s).
pub const STEP_OMEGA: f32 = 12.0;
/// Sway (rad) produced per rad/s of look rotation.
pub const SWAY_GAIN: f32 = 0.012;
/// Largest sway rotation (rad).
pub const SWAY_MAX: f32 = 0.08;
/// Step bob amplitude (m) at full speed.
pub const STEP_AMPLITUDE: f32 = 0.012;

/// Per-shot recoil of a weapon (a local test recipe until the weapon data of
/// phase 4 defines it).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecoilPattern {
    /// Kick of each shot of a burst; the last entry repeats.
    pub kicks: Vec<[f32; 3]>,
    /// Half-width of the uniform random part added to each axis.
    pub random: [f32; 3],
    /// Return stiffness `w` of the recoil spring (rad/s).
    pub return_omega: f32,
    /// Seconds without a shot after which the burst index restarts.
    pub reset_after: f32,
}

impl RecoilPattern {
    /// Pattern of the test rifle.
    pub fn test_rifle() -> Self {
        Self {
            kicks: vec![
                [0.0, 0.010, 0.030],
                [0.002, 0.012, 0.034],
                [-0.003, 0.014, 0.036],
            ],
            random: [0.002, 0.002, 0.003],
            return_omega: 14.0,
            reset_after: 0.35,
        }
    }
}

fn splitmix(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Uniform value in `[-1, 1)` from a seed and a key.
fn signed_unit(seed: u64, key: u64) -> f32 {
    let h = splitmix(splitmix(seed) ^ key);
    ((h >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0
}

/// Recoil kick `(side, rise, back)` of shot number `shot` of a burst: the
/// pattern entry plus its random part drawn from `seed` and `shot`. Pure:
/// the same inputs always give the same kick.
pub fn recoil_kick(pattern: &RecoilPattern, shot: u32, seed: u64) -> Vec3 {
    let base = pattern
        .kicks
        .get(shot as usize)
        .or(pattern.kicks.last())
        .copied()
        .unwrap_or([0.0; 3]);
    let key = u64::from(shot) << 2;
    Vec3::new(
        base[0] + pattern.random[0] * signed_unit(seed, key),
        base[1] + pattern.random[1] * signed_unit(seed, key | 1),
        base[2] + pattern.random[2] * signed_unit(seed, key | 2),
    )
}

/// Offset of the weapon from its rest place.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    pub translation: Vec3,
    pub rotation: Quat,
}

impl Transform {
    pub const IDENTITY: Self = Self {
        translation: Vec3::ZERO,
        rotation: Quat::IDENTITY,
    };

    pub fn to_mat4(&self) -> Mat4 {
        Mat4::from_rotation_translation(self.rotation, self.translation)
    }
}

/// What the viewmodel reads each update.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ViewmodelInputs {
    /// Look rotation since the previous update: yaw (left positive) and
    /// pitch (up positive), in radians.
    pub look_delta: [f32; 2],
    /// Horizontal speed in m/s.
    pub speed: f32,
    /// Stride phase (see `anim::advance_phase`).
    pub phase: f32,
    pub grounded: bool,
}

/// The three springs of the first-person weapon.
#[derive(Debug, Clone, PartialEq)]
pub struct Viewmodel {
    /// Sway: `(yaw, pitch, 0)` rotation offsets, rad.
    pub sway: Spring,
    /// Step: translation offset, m.
    pub step: Spring,
    /// Recoil: `(side, rise, back)` offset (see the module docs).
    pub recoil: Spring,
    seed: u64,
    shot: u32,
    since_shot: f32,
    accumulator: f32,
}

impl Viewmodel {
    /// `seed` feeds the random part of the recoil; the server uses the same.
    pub fn new(seed: u64) -> Self {
        Self {
            sway: Spring::new(SWAY_OMEGA),
            step: Spring::new(STEP_OMEGA),
            recoil: Spring::new(14.0),
            seed,
            shot: 0,
            since_shot: f32::INFINITY,
            accumulator: 0.0,
        }
    }

    /// Adds the recoil of the next shot of the burst.
    pub fn on_shot(&mut self, pattern: &RecoilPattern) {
        if self.since_shot > pattern.reset_after {
            self.shot = 0;
        }
        self.recoil.position += recoil_kick(pattern, self.shot, self.seed);
        self.recoil.omega = pattern.return_omega;
        self.shot = self.shot.saturating_add(1);
        self.since_shot = 0.0;
    }

    /// Advances by `dt` seconds (any size; integrated at [`FIXED_DT`]) and
    /// returns the weapon offset.
    pub fn update(&mut self, dt: f32, inputs: &ViewmodelInputs) -> Transform {
        let dt = dt.max(0.0);
        // Sway follows the look rotation speed, against the motion.
        let rate = if dt > 0.0 {
            Vec3::new(inputs.look_delta[0], inputs.look_delta[1], 0.0) / dt
        } else {
            Vec3::ZERO
        };
        let sway_target = (-rate * SWAY_GAIN).clamp(Vec3::splat(-SWAY_MAX), Vec3::splat(SWAY_MAX));
        let s = (inputs.speed / RUN_SPEED).clamp(0.0, 1.0);
        let step_target = if inputs.grounded {
            Vec3::new(
                inputs.phase.sin() * STEP_AMPLITUDE * s,
                -(2.0 * inputs.phase).sin().abs() * STEP_AMPLITUDE * s,
                0.0,
            )
        } else {
            Vec3::new(0.0, STEP_AMPLITUDE, 0.0)
        };
        self.accumulator += dt;
        while self.accumulator >= FIXED_DT {
            self.accumulator -= FIXED_DT;
            self.sway.step(FIXED_DT, sway_target);
            self.step.step(FIXED_DT, step_target);
            self.recoil.step(FIXED_DT, Vec3::ZERO);
            self.since_shot += FIXED_DT;
        }
        self.transform()
    }

    /// Current offset without advancing.
    pub fn transform(&self) -> Transform {
        let r = self.recoil.position;
        let sway = self.sway.position;
        let rotation = Quat::from_euler(
            EulerRot::YXZ,
            sway.x,
            sway.y + r.y * RECOIL_PITCH_PER_UNIT,
            -sway.x * 0.4,
        );
        Transform {
            translation: self.step.position + Vec3::new(r.x, 0.0, r.z),
            rotation,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(vm: &mut Viewmodel, seconds: f32, inputs: &ViewmodelInputs) -> Transform {
        let mut t = Transform::IDENTITY;
        for _ in 0..(seconds * 60.0).round() as usize {
            t = vm.update(1.0 / 60.0, inputs);
        }
        t
    }

    fn pitch(t: &Transform) -> f32 {
        t.rotation.to_euler(EulerRot::YXZ).1
    }

    #[test]
    fn a_shot_kicks_back_and_returns_within_a_second() {
        let pattern = RecoilPattern::test_rifle();
        let idle = ViewmodelInputs {
            grounded: true,
            ..Default::default()
        };
        let mut vm = Viewmodel::new(7);
        vm.on_shot(&pattern);
        let kicked = vm.update(1.0 / 60.0, &idle);
        assert!(kicked.translation.z > 0.0, "kicks back");
        assert!(pitch(&kicked) > 0.0, "muzzle rises");
        let settled = run(&mut vm, 1.0, &idle);
        assert!(
            settled.translation.length() < 1e-3,
            "{:?}",
            settled.translation
        );
        assert!(pitch(&settled).abs() < 1e-3, "{}", pitch(&settled));
    }

    #[test]
    fn kicks_are_pure_and_follow_the_pattern() {
        let p = RecoilPattern::test_rifle();
        assert_eq!(recoil_kick(&p, 1, 9), recoil_kick(&p, 1, 9));
        assert_ne!(recoil_kick(&p, 1, 9), recoil_kick(&p, 1, 10));
        let k = recoil_kick(&p, 50, 9);
        assert!((k.z - 0.036).abs() <= 0.003 + 1e-6, "last entry repeats");
    }

    #[test]
    fn bursts_restart_after_the_reset_delay() {
        let p = RecoilPattern {
            random: [0.0; 3],
            ..RecoilPattern::test_rifle()
        };
        let idle = ViewmodelInputs::default();
        let mut vm = Viewmodel::new(1);
        vm.on_shot(&p);
        vm.update(1.0, &idle);
        vm.on_shot(&p);
        assert!((vm.recoil.position.z - 0.030).abs() < 1e-3, "restarted");
    }

    #[test]
    fn frame_rate_does_not_change_the_result() {
        let p = RecoilPattern::test_rifle();
        let inputs = ViewmodelInputs {
            speed: 4.0,
            phase: 1.0,
            grounded: true,
            ..Default::default()
        };
        let mut a = Viewmodel::new(3);
        let mut b = Viewmodel::new(3);
        a.on_shot(&p);
        b.on_shot(&p);
        for _ in 0..60 {
            a.update(1.0 / 60.0, &inputs);
        }
        for _ in 0..120 {
            b.update(1.0 / 120.0, &inputs);
        }
        assert_eq!(a.recoil.position, b.recoil.position);
    }

    #[test]
    fn looking_sways_and_moving_steps() {
        let mut vm = Viewmodel::new(0);
        let look = ViewmodelInputs {
            look_delta: [0.05, 0.0],
            grounded: true,
            ..Default::default()
        };
        let t = vm.update(1.0 / 60.0, &look);
        let yaw = t.rotation.to_euler(EulerRot::YXZ).0;
        assert!(yaw < 0.0, "weapon lags against a left turn");
        let mut vm = Viewmodel::new(0);
        let run_in = ViewmodelInputs {
            speed: 6.0,
            phase: 1.2,
            grounded: true,
            ..Default::default()
        };
        let t = run(&mut vm, 0.5, &run_in);
        assert!(t.translation.length() > 1e-3);
    }
}

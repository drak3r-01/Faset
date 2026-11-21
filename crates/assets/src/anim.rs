//! Procedural animation maths: critically damped springs and the sinusoidal
//! limb poses of a running, jumping or sliding character.
//!
//! Everything here is pure and deterministic (only additions, products and
//! `sin`), so the client can predict and the server can recompute it. Units
//! are metres, seconds and radians.

use std::f32::consts::TAU;

use faset_core::glam::Vec3;

/// Fixed step the viewmodel integrates springs with, in seconds.
pub const FIXED_DT: f32 = 1.0 / 120.0;

/// Spring towards a target, critically damped:
/// `x'' = -w^2 (x - target) - 2 w x'` (plan section 12).
///
/// Integrated with semi-implicit Euler, which is stable for `w * dt < 1`;
/// call [`Spring::step`] with [`FIXED_DT`] to stay deterministic. For a
/// start offset `d0` at rest the offset decays as `d0 (1 + w t) e^(-w t)`:
/// with `w = 12` it is below `1e-3 * d0` after 1 s.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spring {
    pub position: Vec3,
    pub velocity: Vec3,
    /// Natural angular frequency in rad/s.
    pub omega: f32,
}

impl Spring {
    /// A spring at rest on the origin.
    pub fn new(omega: f32) -> Self {
        Self {
            position: Vec3::ZERO,
            velocity: Vec3::ZERO,
            omega,
        }
    }

    /// Advances by `dt` seconds towards `target`.
    pub fn step(&mut self, dt: f32, target: Vec3) {
        let w = self.omega;
        let accel = -w * w * (self.position - target) - 2.0 * w * self.velocity;
        self.velocity += accel * dt;
        self.position += self.velocity * dt;
    }
}

/// Horizontal speed (m/s) at which the limb amplitudes reach their maximum.
pub const RUN_SPEED: f32 = 6.0;
/// Distance covered by one full stride cycle (two steps), in metres.
pub const STRIDE_LENGTH: f32 = 2.8;
/// Maximum leg swing (rad) at [`RUN_SPEED`].
pub const LEG_AMPLITUDE: f32 = 0.85;
/// Maximum arm swing (rad) at [`RUN_SPEED`], opposite to the legs.
pub const ARM_AMPLITUDE: f32 = 0.7;
/// Maximum forward lean of the upper body (rad) at [`RUN_SPEED`].
pub const LEAN_AMPLITUDE: f32 = 0.2;
/// Vertical bounce (m) at [`RUN_SPEED`]; two bounces per stride cycle.
pub const BOB_AMPLITUDE: f32 = 0.04;

/// Joint angles of one animation frame. Limb angles are pitch rotations
/// about the joint's X axis, positive swinging the limb forward (-Z).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Pose {
    pub arm_l: f32,
    pub arm_r: f32,
    pub leg_l: f32,
    pub leg_r: f32,
    /// Forward lean of head, torso and arms about the hip (rad).
    pub lean: f32,
    /// Vertical offset of the whole body (m).
    pub bob: f32,
}

impl Pose {
    /// The standing pose.
    pub const NEUTRAL: Self = Self {
        arm_l: 0.0,
        arm_r: 0.0,
        leg_l: 0.0,
        leg_r: 0.0,
        lean: 0.0,
        bob: 0.0,
    };

    /// Largest absolute limb angle.
    pub fn amplitude(&self) -> f32 {
        [self.arm_l, self.arm_r, self.leg_l, self.leg_r]
            .iter()
            .fold(0.0, |m, a| a.abs().max(m))
    }

    /// Linear blend towards `other` (`t` in 0..=1), to ease state changes.
    pub fn lerp(&self, other: &Self, t: f32) -> Self {
        let l = |a: f32, b: f32| a + (b - a) * t;
        Self {
            arm_l: l(self.arm_l, other.arm_l),
            arm_r: l(self.arm_r, other.arm_r),
            leg_l: l(self.leg_l, other.leg_l),
            leg_r: l(self.leg_r, other.leg_r),
            lean: l(self.lean, other.lean),
            bob: l(self.bob, other.bob),
        }
    }
}

/// Phase rate (rad/s) of the stride cycle at a horizontal speed.
pub fn phase_rate(speed: f32) -> f32 {
    TAU * speed.max(0.0) / STRIDE_LENGTH
}

/// Advances a stride phase by `dt` seconds, wrapped to `0..TAU`.
pub fn advance_phase(phase: f32, speed: f32, dt: f32) -> f32 {
    (phase + phase_rate(speed) * dt).rem_euclid(TAU)
}

fn speed_scale(speed: f32) -> f32 {
    (speed / RUN_SPEED).clamp(0.0, 1.0)
}

/// Running pose from the horizontal speed and the stride phase: limbs swing
/// sinusoidally with an amplitude proportional to the speed (arms against
/// legs); neutral at rest.
pub fn limb_pose(speed: f32, phase: f32) -> Pose {
    let s = speed_scale(speed);
    let swing = phase.sin() * s;
    Pose {
        leg_l: LEG_AMPLITUDE * swing,
        leg_r: -LEG_AMPLITUDE * swing,
        arm_l: -ARM_AMPLITUDE * swing,
        arm_r: ARM_AMPLITUDE * swing,
        lean: LEAN_AMPLITUDE * s,
        bob: BOB_AMPLITUDE * s * (1.0 - (2.0 * phase).cos()) * 0.5,
    }
}

/// Pose in the air: arms raised, legs tucked and swinging slightly (`phase`
/// advances with time). The horizontal speed leans the body a little.
pub fn jump_pose(speed: f32, phase: f32) -> Pose {
    let s = speed_scale(speed);
    let sway = 0.12 * phase.sin();
    Pose {
        leg_l: 0.65 + sway,
        leg_r: 0.25 - sway,
        arm_l: 0.9 - sway,
        arm_r: 0.9 + sway,
        lean: 0.1 * s,
        bob: 0.0,
    }
}

/// Slide pose: legs thrust forward, body leaned back, arms trailing.
pub fn slide_pose(phase: f32) -> Pose {
    let tremble = 0.04 * phase.sin();
    Pose {
        leg_l: 1.3 + tremble,
        leg_r: 1.2 - tremble,
        arm_l: -0.6,
        arm_r: -0.5,
        lean: -0.7,
        bob: -0.35,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spring_converges_within_a_millimetre_after_one_second() {
        for omega in [12.0, 16.0, 24.0] {
            let mut s = Spring::new(omega);
            s.position = Vec3::new(1.0, -0.5, 0.25);
            let target = Vec3::new(0.2, 0.1, 0.0);
            for _ in 0..(1.0 / FIXED_DT) as usize {
                s.step(FIXED_DT, target);
            }
            assert!((s.position - target).length() < 1e-3, "omega {omega}");
        }
    }

    #[test]
    fn spring_does_not_overshoot() {
        let mut s = Spring::new(14.0);
        for _ in 0..240 {
            s.step(FIXED_DT, Vec3::X);
            assert!(s.position.x <= 1.0 + 1e-4);
        }
    }

    #[test]
    fn spring_is_deterministic() {
        let run = || {
            let mut s = Spring::new(14.0);
            for i in 0..500 {
                s.step(FIXED_DT, Vec3::splat((i % 7) as f32));
            }
            s
        };
        assert_eq!(run(), run());
    }

    #[test]
    fn pose_at_rest_is_neutral() {
        for k in 0..8 {
            assert_eq!(limb_pose(0.0, k as f32), Pose::NEUTRAL);
        }
    }

    #[test]
    fn amplitude_grows_with_speed() {
        let phase = std::f32::consts::FRAC_PI_2;
        let a: Vec<f32> = [1.0, 2.5, 4.0, 6.0]
            .iter()
            .map(|&v| limb_pose(v, phase).amplitude())
            .collect();
        assert!(a.windows(2).all(|w| w[0] < w[1]), "{a:?}");
        assert!((a[3] - LEG_AMPLITUDE).abs() < 1e-6);
        assert_eq!(limb_pose(60.0, phase), limb_pose(RUN_SPEED, phase));
    }

    #[test]
    fn limbs_swing_in_opposition() {
        let p = limb_pose(4.0, 1.0);
        assert!((p.leg_l + p.leg_r).abs() < 1e-6);
        assert!(p.leg_l * p.arm_l < 0.0);
    }

    #[test]
    fn phase_wraps_and_follows_speed() {
        let p = advance_phase(6.0, 6.0, 1.0);
        assert!((0.0..TAU).contains(&p));
        assert_eq!(advance_phase(1.0, 0.0, 5.0), 1.0);
        assert!(phase_rate(6.0) > phase_rate(3.0));
    }

    #[test]
    fn air_and_slide_differ_from_running() {
        assert_ne!(jump_pose(4.0, 0.5), limb_pose(4.0, 0.5));
        assert!(slide_pose(0.0).lean < 0.0);
    }
}

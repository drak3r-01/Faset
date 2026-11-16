//! Quantizers. Each `quantize_*` returns the raw wire integer, each
//! `dequantize_*` maps it back. Errors are at most half a step; out-of-range
//! inputs saturate, NaN maps to 0 (or mid-range for snapshot pitch).
//!
//! Decisions: the plan gives no ranges for position/velocity, so they are
//! derived from step and width; snapshot pitch covers [-pi/2, pi/2] in 12
//! bits (step pi/4095); command pitch is `i16` with the yaw step 2pi/65536.

use std::f32::consts::{FRAC_PI_2, PI, TAU};

/// Position step: 1/256 m, 24 bits signed per axis.
pub const POS_STEP: f32 = 1.0 / 256.0;
pub const POS_BITS: u32 = 24;
/// Velocity step: 1/16 m/s, 16 bits signed per axis.
pub const VEL_STEP: f32 = 1.0 / 16.0;
pub const VEL_BITS: u32 = 16;
/// Yaw / command pitch step: 2pi/65536 rad.
pub const ANGLE_STEP: f32 = TAU / 65536.0;
pub const PITCH_SNAPSHOT_BITS: u32 = 12;
/// Snapshot pitch step: pi / 4095 rad over [-pi/2, pi/2].
pub const PITCH_SNAPSHOT_STEP: f32 = PI / 4095.0;

fn signed(v: f32, step: f32, bits: u32) -> i32 {
    if v.is_nan() {
        return 0;
    }
    let max = (1i64 << (bits - 1)) - 1;
    let min = -(1i64 << (bits - 1));
    let q = (f64::from(v) / f64::from(step)).round() as i64;
    q.clamp(min, max) as i32
}

#[must_use]
pub fn quantize_position(v: f32) -> i32 {
    signed(v, POS_STEP, POS_BITS)
}

#[must_use]
pub fn dequantize_position(q: i32) -> f32 {
    q as f32 * POS_STEP
}

#[must_use]
pub fn quantize_velocity(v: f32) -> i16 {
    signed(v, VEL_STEP, VEL_BITS) as i16
}

#[must_use]
pub fn dequantize_velocity(q: i16) -> f32 {
    f32::from(q) * VEL_STEP
}

/// Yaw wraps modulo 2pi.
#[must_use]
pub fn quantize_yaw(v: f32) -> u16 {
    if !v.is_finite() {
        return 0;
    }
    let q = (f64::from(v) / f64::from(ANGLE_STEP)).round() as i64;
    q.rem_euclid(65536) as u16
}

#[must_use]
pub fn dequantize_yaw(q: u16) -> f32 {
    f32::from(q) * ANGLE_STEP
}

/// Snapshot pitch, clamped to [-pi/2, pi/2], 12 bits unsigned.
#[must_use]
pub fn quantize_pitch_snapshot(v: f32) -> u16 {
    if v.is_nan() {
        return 2048;
    }
    let t = (v.clamp(-FRAC_PI_2, FRAC_PI_2) + FRAC_PI_2) / PITCH_SNAPSHOT_STEP;
    (t.round() as u16).min(4095)
}

#[must_use]
pub fn dequantize_pitch_snapshot(q: u16) -> f32 {
    f32::from(q.min(4095)) * PITCH_SNAPSHOT_STEP - FRAC_PI_2
}

/// Command pitch, signed 16 bits, saturating.
#[must_use]
pub fn quantize_pitch_command(v: f32) -> i16 {
    signed(v, ANGLE_STEP, 16) as i16
}

#[must_use]
pub fn dequantize_pitch_command(q: i16) -> f32 {
    f32::from(q) * ANGLE_STEP
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn position_within_half_step(v in -32768.0f32..32767.0) {
            let e = f64::from(dequantize_position(quantize_position(v))) - f64::from(v);
            prop_assert!(e.abs() <= 1.0 / 512.0);
        }

        #[test]
        fn velocity_within_half_step(v in -2048.0f32..2047.0) {
            let e = f64::from(dequantize_velocity(quantize_velocity(v))) - f64::from(v);
            prop_assert!(e.abs() <= f64::from(VEL_STEP) / 2.0);
        }

        #[test]
        fn yaw_within_half_step(v in 0.0f32..TAU) {
            let d = f64::from(dequantize_yaw(quantize_yaw(v)));
            let e = (d - f64::from(v)).abs();
            let e = e.min(f64::from(TAU) - e);
            prop_assert!(e <= f64::from(ANGLE_STEP) / 2.0 + 1e-6);
        }

        #[test]
        fn pitch_snapshot_within_half_step(v in -FRAC_PI_2..FRAC_PI_2) {
            let e = f64::from(dequantize_pitch_snapshot(quantize_pitch_snapshot(v))) - f64::from(v);
            prop_assert!(e.abs() <= f64::from(PITCH_SNAPSHOT_STEP) / 2.0 + 1e-6);
        }

        #[test]
        fn pitch_command_within_half_step(v in -3.0f32..3.0) {
            let e = f64::from(dequantize_pitch_command(quantize_pitch_command(v))) - f64::from(v);
            prop_assert!(e.abs() <= f64::from(ANGLE_STEP) / 2.0 + 1e-6);
        }

        #[test]
        fn raw_round_trip(q in -(1i32 << 23)..(1i32 << 23)) {
            prop_assert_eq!(quantize_position(dequantize_position(q)), q);
        }
    }

    #[test]
    fn saturates_and_handles_nan() {
        assert_eq!(quantize_position(1e9), (1 << 23) - 1);
        assert_eq!(quantize_position(-1e9), -(1 << 23));
        assert_eq!(quantize_position(f32::NAN), 0);
        assert_eq!(quantize_velocity(1e9), i16::MAX);
    }
}

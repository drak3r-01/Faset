//! Feel parameters of the first-person view and the mouse conversions.
//!
//! Every number that shapes the feel lives in [`FeelParams`], which the user's
//! TOML settings fill (`#[serde(default)]`: a missing key keeps its default).
//! Nothing in [`FirstPersonCamera`](crate::FirstPersonCamera) is hard-coded.
//! Lengths are metres, angles degrees in the settings (radians in poses),
//! rates are 1/s.

use serde::{Deserialize, Serialize};

/// Degrees of view rotation per raw mouse count at sensitivity 1.
pub const DEGREES_PER_COUNT: f32 = 0.022;

/// Settings of the first-person camera and the mouse.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FeelParams {
    /// Eye height above the feet when standing (capsule 1.8 m).
    pub eye_height_standing: f32,
    /// Eye height above the feet while sliding (capsule 1.0 m).
    pub eye_height_sliding: f32,
    /// Rate of the exponential approach of the eye height to its target.
    pub eye_rate: f32,

    /// Vertical field of view at rest and while running, degrees.
    pub fov_base_deg: f32,
    /// Extra degrees reached at `fov_full_speed`.
    pub fov_extra_deg: f32,
    /// Horizontal speed (m/s) where the field of view starts to widen:
    /// the run speed.
    pub fov_start_speed: f32,
    /// Horizontal speed (m/s) of the full widening.
    pub fov_full_speed: f32,
    /// Rate of the exponential approach of the field of view.
    pub fov_rate: f32,

    /// Roll at `roll_full_speed` of lateral speed, degrees.
    pub roll_max_deg: f32,
    /// Lateral speed (m/s) giving the full roll.
    pub roll_full_speed: f32,
    /// Rate of the exponential approach of the roll.
    pub roll_rate: f32,

    /// Angular frequency of the landing spring, rad/s (critically damped).
    pub landing_omega: f32,
    /// Eye speed kicked downward per m/s of fall speed, in m/s per m/s.
    pub landing_gain: f32,
    /// Fall speed (m/s) under which a landing does not dip the eye.
    pub landing_min_fall: f32,
    /// Deepest dip of the eye, metres.
    pub landing_max_dip: f32,

    /// Mouse sensitivity multiplier (1 = [`DEGREES_PER_COUNT`] per count).
    pub sensitivity: f32,
    /// Mouse resolution in counts per inch, for the cm/360 readout.
    pub mouse_dpi: f32,
    /// Moving the mouse up looks down.
    pub invert_y: bool,
}

impl Default for FeelParams {
    fn default() -> Self {
        Self {
            eye_height_standing: 1.6,
            eye_height_sliding: 0.8,
            eye_rate: 14.0,
            fov_base_deg: 75.0,
            fov_extra_deg: 10.0,
            fov_start_speed: 7.0,
            fov_full_speed: 20.0,
            fov_rate: 6.0,
            roll_max_deg: 1.5,
            roll_full_speed: 7.0,
            roll_rate: 10.0,
            landing_omega: 18.0,
            landing_gain: 0.5,
            landing_min_fall: 3.0,
            landing_max_dip: 0.3,
            sensitivity: 1.0,
            mouse_dpi: 800.0,
            invert_y: false,
        }
    }
}

impl FeelParams {
    /// View rotation per raw mouse count, degrees.
    #[must_use]
    pub fn degrees_per_count(&self) -> f32 {
        degrees_per_count(self.sensitivity)
    }

    /// Mouse travel for a full turn, centimetres.
    #[must_use]
    pub fn cm_per_360(&self) -> f32 {
        cm_per_360(self.sensitivity, self.mouse_dpi)
    }

    /// Applies a raw mouse movement (`dx` right, `dy` down, in counts) to
    /// `(yaw, pitch)` in radians. Moving the mouse right turns right (yaw
    /// decreases: positive yaw is a left turn); pitch is clamped just short of
    /// straight up and down.
    #[must_use]
    pub fn apply_mouse(&self, yaw: f32, pitch: f32, dx: f32, dy: f32) -> (f32, f32) {
        let step = self.degrees_per_count().to_radians();
        let sign = if self.invert_y { -1.0 } else { 1.0 };
        let limit = 89.0f32.to_radians();
        (
            (yaw - dx * step).rem_euclid(std::f32::consts::TAU),
            (pitch - dy * step * sign).clamp(-limit, limit),
        )
    }
}

/// Degrees of rotation per raw count: `0.022 * sensitivity`.
#[must_use]
pub fn degrees_per_count(sensitivity: f32) -> f32 {
    DEGREES_PER_COUNT * sensitivity
}

/// Centimetres of mouse travel for 360 degrees:
/// `360 * 2.54 / (0.022 * sensitivity * dpi)`.
#[must_use]
pub fn cm_per_360(sensitivity: f32, dpi: f32) -> f32 {
    360.0 * 2.54 / (degrees_per_count(sensitivity) * dpi)
}

/// A critically damped spring, `x'' = -w^2 (x - target) - 2 w x'`, advanced
/// with its closed form `d(t) = (d0 + (v0 + w d0) t) e^{-w t}` so it is exact
/// for any step and never unstable. The weapon view spring of lot 4.4
/// (`RecoilState`) has the same form.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CriticalSpring {
    /// Position.
    pub x: f32,
    /// Velocity.
    pub v: f32,
}

impl CriticalSpring {
    /// Advances the spring by `dt` seconds toward `target`.
    pub fn step(&mut self, target: f32, omega: f32, dt: f32) {
        let d = self.x - target;
        let e = (-omega * dt).exp();
        let c = self.v + omega * d;
        self.x = target + (d + c * dt) * e;
        self.v = (self.v - omega * c * dt) * e;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sensitivity_one_at_800_dpi_is_51_95_cm_per_turn() {
        let cm = cm_per_360(1.0, 800.0);
        println!("sens 1, 800 dpi: {cm:.2} cm/360");
        assert!((cm - 914.4 / 17.6).abs() < 1e-3, "{cm}");
        assert!((cm - 51.95).abs() < 0.01);
    }

    #[test]
    fn a_count_turns_by_0_022_degrees_times_sensitivity() {
        let p = FeelParams {
            sensitivity: 2.0,
            ..FeelParams::default()
        };
        assert!((p.degrees_per_count() - 0.044).abs() < 1e-6);
        let (yaw, pitch) = p.apply_mouse(0.0, 0.0, -100.0, -50.0);
        assert!(
            (yaw.to_degrees() - 4.4).abs() < 1e-3,
            "left turn is positive"
        );
        assert!((pitch.to_degrees() - 2.2).abs() < 1e-3, "mouse up looks up");
    }

    #[test]
    fn pitch_is_clamped_and_yaw_wraps() {
        let p = FeelParams::default();
        let (yaw, pitch) = p.apply_mouse(0.0, 0.0, 1.0, -1.0e6);
        assert!(pitch < 90f32.to_radians() && pitch > 88f32.to_radians());
        assert!((0.0..std::f32::consts::TAU).contains(&yaw));
    }

    #[test]
    fn spring_is_critically_damped_and_step_independent() {
        let run = |dt: f32, steps: u32| {
            let mut s = CriticalSpring { x: -0.2, v: -1.0 };
            for _ in 0..steps {
                s.step(0.0, 18.0, dt);
            }
            s
        };
        let a = run(0.01, 50);
        let b = run(0.05, 10);
        assert!((a.x - b.x).abs() < 1e-5 && (a.v - b.v).abs() < 1e-4);
        // Critical damping never overshoots the target.
        let mut s = CriticalSpring { x: -0.2, v: 0.0 };
        for _ in 0..200 {
            s.step(0.0, 18.0, 0.005);
            assert!(s.x <= 1e-6);
        }
        assert!(s.x.abs() < 1e-3);
    }

    #[test]
    fn params_round_trip_through_toml_and_missing_keys_default() {
        let p = FeelParams {
            fov_base_deg: 90.0,
            invert_y: true,
            ..FeelParams::default()
        };
        let text = toml::to_string(&p).unwrap();
        assert_eq!(toml::from_str::<FeelParams>(&text).unwrap(), p);
        let partial: FeelParams = toml::from_str("sensitivity = 0.5").unwrap();
        assert_eq!(partial.sensitivity, 0.5);
        assert_eq!(partial.eye_height_standing, 1.6);
    }
}

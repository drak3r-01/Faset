//! Recoil: a view offset kicked per shot and returned by a critically
//! damped spring, `x'' = -omega^2 x - 2 omega x'`.
//!
//! The spring is integrated with its closed form, so the result does not
//! depend on the step size beyond float rounding and never goes unstable.

use crate::weapon::RecoilDef;

/// View offset in radians (added to the camera yaw and pitch) and its
/// velocity.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RecoilState {
    pub yaw: f32,
    pub pitch: f32,
    vel_yaw: f32,
    vel_pitch: f32,
}

impl RecoilState {
    /// Adds the kick of shot number `index` of the current spray (the last
    /// pattern entry repeats). `u_yaw` and `u_pitch` are uniform in
    /// `[0, 1)`: each component is `pattern * (1 - f + 2 f u)`, so the random
    /// fraction `f` keeps the mean kick equal to the pattern.
    ///
    /// Returns the kick applied, `(yaw, pitch)`.
    pub fn kick(&mut self, def: &RecoilDef, index: usize, u_yaw: f32, u_pitch: f32) -> (f32, f32) {
        let (py, pp) = def.pattern[index.min(def.pattern.len() - 1)];
        let f = def.random_fraction;
        let ky = py * (1.0 - f + 2.0 * f * u_yaw);
        let kp = pp * (1.0 - f + 2.0 * f * u_pitch);
        self.yaw += ky;
        self.pitch += kp;
        (ky, kp)
    }

    /// Advances the spring by `dt` seconds toward zero.
    pub fn step(&mut self, omega: f32, dt: f32) {
        (self.yaw, self.vel_yaw) = spring(self.yaw, self.vel_yaw, omega, dt);
        (self.pitch, self.vel_pitch) = spring(self.pitch, self.vel_pitch, omega, dt);
    }
}

/// Exact step of a critically damped spring with target 0:
/// `x(t) = (x0 + (v0 + w x0) t) e^{-w t}`.
fn spring(x: f32, v: f32, omega: f32, dt: f32) -> (f32, f32) {
    let e = libm::expf(-omega * dt);
    let c = v + omega * x;
    ((x + c * dt) * e, (v - omega * c * dt) * e)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def() -> RecoilDef {
        RecoilDef {
            pattern: vec![(0.01, 0.02), (-0.01, 0.03)],
            random_fraction: 0.5,
            omega: 14.0,
        }
    }

    #[test]
    fn pattern_is_followed_and_last_entry_repeats() {
        let mut r = RecoilState::default();
        // u = 0.5 gives exactly the pattern value.
        assert_eq!(r.kick(&def(), 0, 0.5, 0.5), (0.01, 0.02));
        assert_eq!(r.kick(&def(), 1, 0.5, 0.5), (-0.01, 0.03));
        assert_eq!(r.kick(&def(), 9, 0.5, 0.5), (-0.01, 0.03));
        assert!((r.pitch - 0.08).abs() < 1e-6);
        // The random part spans [1 - f, 1 + f] of the pattern.
        let mut r = RecoilState::default();
        assert!((r.kick(&def(), 0, 0.0, 0.0).1 - 0.01).abs() < 1e-7);
        assert!((r.kick(&def(), 0, 1.0, 1.0).1 - 0.03).abs() < 1e-7);
    }

    #[test]
    fn spring_returns_to_zero_without_overshoot() {
        let mut r = RecoilState::default();
        r.kick(&def(), 0, 0.5, 0.5);
        let mut last = r.pitch;
        for _ in 0..600 {
            r.step(14.0, 1.0 / 60.0);
            assert!(r.pitch >= -1e-6, "critically damped: no overshoot");
            assert!(r.pitch <= last + 1e-9, "monotonic return");
            last = r.pitch;
        }
        assert!(r.pitch.abs() < 1e-6 && r.yaw.abs() < 1e-6);
    }

    #[test]
    fn step_size_does_not_matter() {
        let mut a = RecoilState::default();
        let mut b = a;
        a.kick(&def(), 0, 0.5, 0.5);
        b.kick(&def(), 0, 0.5, 0.5);
        a.step(14.0, 0.1);
        for _ in 0..10 {
            b.step(14.0, 0.01);
        }
        assert!((a.pitch - b.pitch).abs() < 1e-5);
    }
}

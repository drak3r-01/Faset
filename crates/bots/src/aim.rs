//! Aim: ballistic lead, gravity compensation, lob arcs, the angular spring
//! and the time-correlated aim error.
//!
//! Conventions: yaw 0 faces -Z and grows towards -X (as the controller),
//! positive pitch looks up. All vectors are relative to the shooter's eye.

use std::f32::consts::{PI, TAU};

use faset_core::glam::{DVec3, Vec3};
use faset_sim::rng::Pcg32;

use crate::level::LevelParams;

/// Smallest positive root `t` of `(v.v - s^2) t^2 + 2 (d.v) t + d.d = 0`: the
/// time a projectile of speed `s` needs to meet a target at relative position
/// `d` moving at `v`. `None` when the target cannot be reached.
#[must_use]
pub fn lead_time(d: Vec3, v: Vec3, s: f32) -> Option<f32> {
    let (d, v, s) = (d.as_dvec3(), v.as_dvec3(), f64::from(s));
    let a = v.dot(v) - s * s;
    let b = 2.0 * d.dot(v);
    let c = d.dot(d);
    if c <= 0.0 {
        return Some(0.0);
    }
    if a.abs() < 1e-9 {
        if b.abs() < 1e-12 {
            return None;
        }
        let t = -c / b;
        return (t > 0.0).then_some(t as f32);
    }
    let disc = b * b - 4.0 * a * c;
    if disc < 0.0 {
        return None;
    }
    let sq = disc.sqrt();
    let (t1, t2) = ((-b - sq) / (2.0 * a), (-b + sq) / (2.0 * a));
    let (lo, hi) = if t1 < t2 { (t1, t2) } else { (t2, t1) };
    if lo > 0.0 {
        Some(lo as f32)
    } else if hi > 0.0 {
        Some(hi as f32)
    } else {
        None
    }
}

/// A launch direction and the flight time it was computed for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AimSolution {
    /// Unit launch direction.
    pub dir: Vec3,
    /// Flight time to the meeting point, seconds.
    pub time: f32,
    /// Meeting point relative to the shooter, metres.
    pub point: Vec3,
}

/// Direct-fire solution with gravity `g` (m/s^2, positive, pulls down).
///
/// The projectile leaves along `u` at speed `s` and meets the target at `t`
/// when `s u t = D + v t + (0, g t^2 / 2, 0)`, so `t` is a root of
/// `f(t) = |W(t)|^2 - s^2 t^2` with `W(t) = D + v t + up g t^2 / 2`. The
/// search starts from the gravity-free [`lead_time`] and refines it with
/// `iterations` Newton steps (the bot uses 2).
#[must_use]
pub fn solve_direct(d: Vec3, v: Vec3, s: f32, g: f32, iterations: usize) -> Option<AimSolution> {
    let mut t = f64::from(lead_time(d, v, s)?);
    let (dd, vv, s, g) = (d.as_dvec3(), v.as_dvec3(), f64::from(s), f64::from(g));
    let w = |t: f64| dd + vv * t + DVec3::Y * (0.5 * g * t * t);
    for _ in 0..iterations {
        let wt = w(t);
        let dw = vv + DVec3::Y * (g * t);
        let f = wt.dot(wt) - s * s * t * t;
        let df = 2.0 * wt.dot(dw) - 2.0 * s * s * t;
        if df.abs() < 1e-12 {
            break;
        }
        let next = t - f / df;
        if next <= 0.0 || !next.is_finite() {
            break;
        }
        t = next;
    }
    let p = w(t);
    let dir = p.normalize_or_zero();
    (dir != DVec3::ZERO).then(|| AimSolution {
        dir: dir.as_vec3(),
        time: t as f32,
        point: p.as_vec3(),
    })
}

/// Residual `| |W(t)| - s t |` of [`solve_direct`] after `iterations` steps
/// (metres); zero for an exact solution.
#[must_use]
pub fn direct_residual(d: Vec3, v: Vec3, s: f32, g: f32, iterations: usize) -> Option<f32> {
    let sol = solve_direct(d, v, s, g, iterations)?;
    Some((sol.point.length() - s * sol.time).abs())
}

/// Elevation angles for a target at horizontal distance `x` and height
/// difference `y`, as `(direct, lob)`: `atan((s^2 -/+ sqrt(s^4 - g (g x^2 +
/// 2 y s^2))) / (g x))`. The minus root is the direct arc and the plus root
/// the lob (interpretation of the plan, which leaves the sign open). `None`
/// when the target is out of range.
#[must_use]
pub fn lob_angles(s: f32, g: f32, x: f32, y: f32) -> Option<(f32, f32)> {
    if g <= 0.0 || x <= 0.0 {
        return None;
    }
    let (s2, g, x, y) = (
        f64::from(s) * f64::from(s),
        f64::from(g),
        f64::from(x),
        f64::from(y),
    );
    let disc = s2 * s2 - g * (g * x * x + 2.0 * y * s2);
    if disc < 0.0 {
        return None;
    }
    let sq = disc.sqrt();
    Some((
        ((s2 - sq) / (g * x)).atan() as f32,
        ((s2 + sq) / (g * x)).atan() as f32,
    ))
}

/// Lob solution towards a moving target: the arc is solved for the target's
/// predicted position, and the flight time `x / (s cos theta)` feeds the next
/// prediction (`iterations` times, the bot uses 2).
#[must_use]
pub fn solve_lob(d: Vec3, v: Vec3, s: f32, g: f32, iterations: usize) -> Option<AimSolution> {
    let mut t = lead_time(d, v, s).unwrap_or(0.0);
    let mut out = None;
    for _ in 0..=iterations {
        let p = d + v * t;
        let x = p.x.hypot(p.z);
        let (_, theta) = lob_angles(s, g, x, p.y)?;
        let horiz = if x > 1e-4 {
            Vec3::new(p.x / x, 0.0, p.z / x)
        } else {
            Vec3::NEG_Z
        };
        let dir = horiz * theta.cos() + Vec3::Y * theta.sin();
        t = x / (s * theta.cos()).max(1e-3);
        out = Some(AimSolution {
            dir,
            time: t,
            point: p,
        });
    }
    out
}

/// Yaw of a direction (0 faces -Z, grows towards -X).
#[must_use]
pub fn yaw_of(dir: Vec3) -> f32 {
    (-dir.x).atan2(-dir.z)
}

/// Pitch of a direction (positive looks up).
#[must_use]
pub fn pitch_of(dir: Vec3) -> f32 {
    let h = dir.x.hypot(dir.z);
    dir.y.atan2(h)
}

/// Unit view direction of a yaw and a pitch.
#[must_use]
pub fn dir_of(yaw: f32, pitch: f32) -> Vec3 {
    let (sy, cy) = yaw.sin_cos();
    let (sp, cp) = pitch.sin_cos();
    Vec3::new(-sy * cp, sp, -cy * cp)
}

/// Difference `a - b` wrapped to `[-PI, PI]`.
#[must_use]
pub fn angle_diff(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(TAU);
    if d > PI {
        d - TAU
    } else {
        d
    }
}

/// One step of the time-correlated error:
/// `e <- e exp(-dt/tau) + sigma sqrt(1 - exp(-2 dt/tau)) n`, with `n` a
/// sample of N(0, 1).
#[must_use]
pub fn ou_step(e: f32, sigma: f32, tau: f32, dt: f32, n: f32) -> f32 {
    let k = libm::expf(-dt / tau.max(1e-4));
    e * k + sigma * libm::sqrtf((1.0 - k * k).max(0.0)) * n
}

/// A sample of N(0, 1) (Box-Muller) from a seeded generator.
pub fn gaussian(rng: &mut Pcg32) -> f32 {
    let u1 = 1.0 - rng.next_f32(); // (0, 1]
    let u2 = rng.next_f32();
    libm::sqrtf(-2.0 * libm::logf(u1)) * libm::cosf(TAU * u2)
}

/// The bot's view direction: an angular spring towards the target with a
/// capped rotation speed, plus the correlated error while aiming.
#[derive(Debug, Clone)]
pub struct AimTracker {
    yaw: f32,
    pitch: f32,
    err_yaw: f32,
    err_pitch: f32,
    spring: f32,
    max_speed: f32,
    sigma: f32,
    tau: f32,
    rng: Pcg32,
}

impl AimTracker {
    /// A tracker looking along `yaw`/`pitch`, drawing its noise from `seed`.
    #[must_use]
    pub fn new(params: &LevelParams, seed: u64, yaw: f32, pitch: f32) -> Self {
        Self {
            yaw,
            pitch,
            err_yaw: 0.0,
            err_pitch: 0.0,
            spring: params.aim_spring,
            max_speed: params.aim_max_speed,
            sigma: params.aim_sigma,
            tau: params.aim_tau,
            rng: Pcg32::new(seed, 0xA1),
        }
    }

    #[must_use]
    pub fn yaw(&self) -> f32 {
        self.yaw
    }

    #[must_use]
    pub fn pitch(&self) -> f32 {
        self.pitch
    }

    /// Current error offsets `(yaw, pitch)`.
    #[must_use]
    pub fn error(&self) -> (f32, f32) {
        (self.err_yaw, self.err_pitch)
    }

    /// Looks along `yaw`/`pitch` immediately.
    pub fn snap(&mut self, yaw: f32, pitch: f32) {
        self.yaw = yaw.rem_euclid(TAU);
        self.pitch = pitch;
    }

    /// Advances by `dt`. The error always evolves; it only offsets the target
    /// when `aiming` (a bot walking down a corridor does not tremble).
    pub fn step(&mut self, target_yaw: f32, target_pitch: f32, aiming: bool, dt: f32) {
        let ny = gaussian(&mut self.rng);
        let np = gaussian(&mut self.rng);
        self.err_yaw = ou_step(self.err_yaw, self.sigma, self.tau, dt, ny);
        self.err_pitch = ou_step(self.err_pitch, self.sigma, self.tau, dt, np);
        let (ey, ep) = if aiming {
            (self.err_yaw, self.err_pitch)
        } else {
            (0.0, 0.0)
        };
        let dy = angle_diff(target_yaw + ey, self.yaw);
        let dp = (target_pitch + ep) - self.pitch;
        let cap = self.max_speed * dt;
        let sy = (dy * self.spring * dt).clamp(-cap, cap);
        let sp = (dp * self.spring * dt).clamp(-cap, cap);
        self.yaw += sy;
        self.pitch += sp;
        self.yaw = self.yaw.rem_euclid(TAU);
        self.pitch = self.pitch.clamp(-1.5, 1.5);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::level::Level;

    #[test]
    fn lead_time_solves_the_quadratic() {
        let cases = [
            (Vec3::new(30.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 6.0), 40.0),
            (Vec3::new(-12.0, 3.0, 20.0), Vec3::new(5.0, 0.0, -4.0), 35.0),
            (Vec3::new(8.0, 0.0, 0.0), Vec3::new(-6.0, 0.0, 0.0), 20.0),
            (Vec3::new(50.0, 5.0, 10.0), Vec3::new(3.0, 0.0, 3.0), 90.0),
        ];
        for (d, v, s) in cases {
            let t = lead_time(d, v, s).unwrap();
            // The projectile and the target are at the same place at t.
            let miss = (d + v * t).length() - s * t;
            assert!(
                miss.abs() < 0.01 * s * t,
                "case {d:?}: residual {miss} for time {t}"
            );
            // The closed-form root, evaluated independently in f64.
            let (dd, vv, ss) = (d.as_dvec3(), v.as_dvec3(), f64::from(s));
            let (a, b, c) = (vv.dot(vv) - ss * ss, 2.0 * dd.dot(vv), dd.dot(dd));
            let disc = (b * b - 4.0 * a * c).sqrt();
            let roots = [(-b - disc) / (2.0 * a), (-b + disc) / (2.0 * a)];
            let want = roots
                .iter()
                .copied()
                .filter(|r| *r > 0.0)
                .fold(f64::INFINITY, f64::min);
            assert!(
                ((f64::from(t) - want) / want).abs() < 0.01,
                "lead time {t} vs root {want}"
            );
        }
    }

    #[test]
    fn lead_time_none_when_target_outruns_projectile() {
        // Receding faster than the projectile flies.
        assert!(lead_time(Vec3::new(10.0, 0.0, 0.0), Vec3::new(30.0, 0.0, 0.0), 20.0).is_none());
        // Stationary target: plain distance over speed.
        let t = lead_time(Vec3::new(0.0, 0.0, -40.0), Vec3::ZERO, 20.0).unwrap();
        assert!((t - 2.0).abs() < 1e-4);
    }

    /// Integrates the real parabola and returns the miss distance at the
    /// flight time of the solution.
    fn fly(d: Vec3, v: Vec3, s: f32, g: f32, sol: &AimSolution) -> f32 {
        let steps = 2000;
        let dt = sol.time / steps as f32;
        let (mut p, mut u) = (Vec3::ZERO, sol.dir * s);
        for _ in 0..steps {
            u.y -= g * dt;
            p += u * dt;
        }
        (p - (d + v * sol.time)).length()
    }

    #[test]
    fn gravity_compensation_converges_in_two_iterations() {
        let (d, v, s, g) = (
            Vec3::new(45.0, 2.0, -30.0),
            Vec3::new(-3.0, 0.0, 4.0),
            38.0,
            9.8,
        );
        let r0 = direct_residual(d, v, s, g, 0).unwrap();
        let r1 = direct_residual(d, v, s, g, 1).unwrap();
        let r2 = direct_residual(d, v, s, g, 2).unwrap();
        assert!(r0 > 0.5, "gravity matters here: {r0}");
        assert!(r1 < r0 && r2 < r1);
        assert!(r2 < 0.02, "two Newton steps leave {r2} m");
        let sol = solve_direct(d, v, s, g, 2).unwrap();
        let miss = fly(d, v, s, g, &sol);
        assert!(miss < 0.1, "flight check misses by {miss} m");
        // Without compensation the shot is far off.
        let flat = solve_direct(d, v, s, g, 0).unwrap();
        assert!(fly(d, v, s, g, &flat) > 5.0 * miss);
    }

    #[test]
    fn gravity_free_solution_is_exact() {
        let (d, v) = (Vec3::new(10.0, 0.0, -20.0), Vec3::new(2.0, 0.0, 1.0));
        let sol = solve_direct(d, v, 30.0, 0.0, 2).unwrap();
        assert!(fly(d, v, 30.0, 0.0, &sol) < 1e-2);
    }

    #[test]
    fn lob_arcs_hit_the_target() {
        let (s, g, x, y) = (20.0_f32, 9.8_f32, 25.0_f32, 3.0_f32);
        let (direct, lob) = lob_angles(s, g, x, y).unwrap();
        assert!(direct < lob, "the plus root is the high arc");
        for theta in [direct, lob] {
            // Height at horizontal distance x on the parabola.
            let t = x / (s * theta.cos());
            let h = s * theta.sin() * t - 0.5 * g * t * t;
            assert!((h - y).abs() < 0.01, "theta {theta}: height {h}");
        }
        assert!(lob_angles(s, g, 80.0, 0.0).is_none(), "out of range");
        let sol = solve_lob(Vec3::new(25.0, 3.0, 0.0), Vec3::ZERO, s, g, 2).unwrap();
        assert!(pitch_of(sol.dir) > 0.7, "the lob points high");
    }

    #[test]
    fn angle_helpers_round_trip() {
        for (yaw, pitch) in [(0.3, 0.2), (-2.0, -0.4), (3.0, 1.0)] {
            let d = dir_of(yaw, pitch);
            assert!(angle_diff(yaw_of(d), yaw).abs() < 1e-5);
            assert!((pitch_of(d) - pitch).abs() < 1e-5);
        }
        assert!((angle_diff(0.1, TAU - 0.1) - 0.2).abs() < 1e-5);
        // Yaw 0 faces -Z.
        assert!(dir_of(0.0, 0.0).distance(Vec3::NEG_Z) < 1e-6);
    }

    #[test]
    fn error_is_correlated_and_stationary() {
        // Stationary standard deviation equals sigma; the lag-1 correlation of
        // 1/60 s steps is exp(-dt/tau), unlike white noise.
        let (sigma, tau, dt) = (0.05_f32, 0.3_f32, 1.0 / 60.0);
        let mut rng = Pcg32::new(7, 1);
        let mut e = 0.0;
        let mut xs = Vec::new();
        for _ in 0..200_000 {
            e = ou_step(e, sigma, tau, dt, gaussian(&mut rng));
            xs.push(e);
        }
        let n = xs.len() as f32;
        let var = xs.iter().map(|x| x * x).sum::<f32>() / n;
        assert!(
            (var.sqrt() - sigma).abs() < 0.05 * sigma,
            "std {}",
            var.sqrt()
        );
        let lag1 = xs.windows(2).map(|w| w[0] * w[1]).sum::<f32>() / n / var;
        let want = (-dt / tau).exp();
        assert!((lag1 - want).abs() < 0.02, "lag-1 {lag1}, expected {want}");
    }

    #[test]
    fn tracker_caps_rotation_speed_and_settles() {
        let p = Level::Normal.params();
        let mut t = AimTracker::new(&p, 3, 0.0, 0.0);
        let dt = 1.0 / 60.0;
        t.step(2.0, 0.0, false, dt);
        assert!(t.yaw() <= p.aim_max_speed * dt + 1e-6, "cap respected");
        for _ in 0..300 {
            t.step(2.0, 0.3, false, dt);
        }
        assert!(angle_diff(t.yaw(), 2.0).abs() < 1e-3);
        assert!((t.pitch() - 0.3).abs() < 1e-3);
        // Aiming adds the error, bounded by a few sigma.
        let mut off = 0.0_f32;
        for _ in 0..600 {
            t.step(2.0, 0.3, true, dt);
            off = off.max(angle_diff(t.yaw(), 2.0).abs());
        }
        assert!(off > 0.0 && off < 6.0 * p.aim_sigma + 0.01);
    }
}

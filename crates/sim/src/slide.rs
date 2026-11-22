//! Slide and slide-hop (lot 4.3, plan section 15).
//!
//! The controller ([`crate::controller::step_player`]) calls these on its
//! tick: [`update`] after the ground test (entry and exit), [`friction`],
//! [`steer`] and [`slope_gravity`] in place of ground friction and
//! acceleration while sliding, and [`start`] on a landing with crouch held.
//!
//! - Entry: crouch held on the ground with a horizontal speed of at least
//!   [`MoveParams::slide_min_speed`] (5 m/s), or landing with crouch held.
//! - Impulse: `v_h ← v_h + b v̂_h` with `b` = [`MoveParams::slide_impulse`]
//!   (2.5 m/s) on a ground entry, `b = min(k |v_y|, b_max)` on a landing.
//! - During the slide: friction [`MoveParams::slide_friction`] (0.8 1/s) in
//!   the ground-friction form, weak steering, tangential gravity on slopes
//!   `v ← v + (g − (g·n) n) Δt`; the capsule is the crouched one (1.0 m, eye
//!   0.8 m) since crouch is held.
//! - Exit: horizontal speed below [`MoveParams::slide_exit_speed`] (3 m/s),
//!   crouch released, [`MoveParams::slide_duration`] (1.5 s) elapsed, or the
//!   player leaves the ground. A jump during a slide (slide-hop) only ends
//!   the slide: no friction is applied on that tick, so all speed is kept.
//!
//! Decisions (unspecified by the plan):
//! - The 1 s cooldown ([`MoveParams::slide_cooldown`]) gates the impulse,
//!   not the slide itself: a slide started during the cooldown gets no
//!   impulse.
//! - A ground entry needs a new crouch press after a slide (the latch is
//!   cleared when crouch is released), so holding crouch down a long slope
//!   does not chain slides. Landings ignore the latch.
//! - `k` = [`MoveParams::slide_land_factor`] = 0.25 and `b_max` =
//!   [`MoveParams::slide_land_max`] = 4 m/s: a plain jump landing (6.5 m/s)
//!   gives about 1.6 m/s, a 3 m drop about 2.7 m/s.
//! - Steering blends the horizontal direction toward the wish direction at
//!   [`MoveParams::slide_steer`] (1.5 1/s) and keeps the speed: it never
//!   accelerates nor brakes.
//! - The impulse follows the horizontal velocity, or the wish direction when
//!   the player has none (vertical landing), or is skipped.

use faset_core::glam::Vec3;

use crate::controller::{apply_friction, MoveParams, PlayerState};

/// Whether the player is sliding.
#[must_use]
pub fn is_sliding(s: &PlayerState) -> bool {
    s.slide_timer > 0.0
}

fn horizontal(v: Vec3) -> Vec3 {
    Vec3::new(v.x, 0.0, v.z)
}

/// Entry and exit after the ground test. `active` is the timer threshold
/// (half a tick). Returns whether a slide started.
pub fn update(s: &mut PlayerState, crouch: bool, p: &MoveParams, active: f32) -> bool {
    if !crouch {
        s.slide_latch = false;
    }
    let speed = horizontal(s.vel).length();
    if is_sliding(s) {
        if !s.on_ground || !crouch || speed < p.slide_exit_speed || s.slide_timer <= active {
            s.slide_timer = 0.0;
        }
        false
    } else if crouch && s.on_ground && !s.slide_latch && speed >= p.slide_min_speed {
        start(s, p.slide_impulse, Vec3::ZERO, p, active);
        true
    } else {
        false
    }
}

/// Starts a slide with impulse `impulse` (m/s) along the horizontal
/// velocity, or along `fallback` (unit or zero) when there is none.
pub fn start(s: &mut PlayerState, impulse: f32, fallback: Vec3, p: &MoveParams, active: f32) {
    s.slide_timer = p.slide_duration;
    s.slide_latch = true;
    if impulse <= 0.0 || s.slide_cooldown > active {
        return;
    }
    let dir = horizontal(s.vel).try_normalize().unwrap_or(fallback);
    if dir != Vec3::ZERO {
        s.vel += dir * impulse;
        s.slide_cooldown = p.slide_cooldown;
    }
}

/// Landing slide impulse `min(k |v_y|, b_max)` for a fall speed in m/s.
#[must_use]
pub fn landing_impulse(fall_speed: f32, p: &MoveParams) -> f32 {
    (p.slide_land_factor * fall_speed.abs()).min(p.slide_land_max)
}

/// Slide friction, in the ground-friction form.
pub fn friction(vel: &mut Vec3, p: &MoveParams, dt: f32) {
    apply_friction(vel, p.slide_friction, p.stop_speed, dt);
}

/// Weak steering: turns the horizontal velocity toward `wish_dir`, same speed.
#[must_use]
pub fn steer(vel: Vec3, wish_dir: Vec3, p: &MoveParams, dt: f32) -> Vec3 {
    let h = horizontal(vel);
    let speed = h.length();
    if wish_dir == Vec3::ZERO || speed <= 0.0 {
        return vel;
    }
    let d = h / speed;
    let turned = (d + wish_dir * (p.slide_steer * dt))
        .try_normalize()
        .unwrap_or(d);
    Vec3::new(turned.x * speed, vel.y, turned.z * speed)
}

/// Adds the part of gravity (`gravity` m/s², downward) tangent to the
/// ground of normal `n`: descents accelerate, climbs slow down.
#[must_use]
pub fn slope_gravity(vel: Vec3, n: Vec3, gravity: f32, dt: f32) -> Vec3 {
    let g = Vec3::NEG_Y * gravity;
    vel + (g - n * g.dot(n)) * dt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn landing_impulse_grows_with_fall_speed_and_caps() {
        let p = MoveParams::default();
        assert!((landing_impulse(4.0, &p) - 1.0).abs() < 1e-6);
        assert!((landing_impulse(-8.0, &p) - 2.0).abs() < 1e-6);
        assert_eq!(landing_impulse(30.0, &p), 4.0);
    }

    #[test]
    fn steering_keeps_speed_and_flat_ground_adds_nothing() {
        let p = MoveParams::default();
        let v = Vec3::new(0.0, 0.0, -8.0);
        let out = steer(v, Vec3::X, &p, 1.0 / 60.0);
        assert!((horizontal(out).length() - 8.0).abs() < 1e-5);
        assert!(out.x > 0.0);
        assert_eq!(slope_gravity(v, Vec3::Y, 20.0, 1.0 / 60.0), v);
        let n = Vec3::new(0.5, 3f32.sqrt() * 0.5, 0.0); // 30° slope going down +X
        let down = slope_gravity(Vec3::ZERO, n, 20.0, 1.0);
        assert!(down.x > 0.0 && down.y < 0.0 && down.dot(n).abs() < 1e-5);
    }

    #[test]
    fn impulse_has_a_cooldown() {
        let p = MoveParams::default();
        let mut s = PlayerState::spawn(Vec3::ZERO);
        s.vel = Vec3::new(6.0, 0.0, 0.0);
        start(&mut s, 2.5, Vec3::ZERO, &p, 1.0 / 120.0);
        assert!((s.vel.x - 8.5).abs() < 1e-6 && is_sliding(&s));
        start(&mut s, 2.5, Vec3::ZERO, &p, 1.0 / 120.0);
        assert!((s.vel.x - 8.5).abs() < 1e-6, "second impulse within 1 s");
    }
}

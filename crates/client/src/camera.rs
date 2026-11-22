//! The first-person camera: eye height, field of view, roll and landing dip
//! computed from the player's state each frame.
//!
//! The camera reads a [`PlayerState`] and the look angles and returns a
//! [`CameraPose`]; it never touches the renderer. The client turns the pose
//! into a render camera when it extracts the `RenderWorld`.

use faset_core::glam::{Quat, Vec3};
use faset_sim::controller::PlayerState;
use faset_sim::slide::is_sliding;

use crate::feel::{CriticalSpring, FeelParams};

/// Where the player looks, radians. Positive yaw turns left (forward is -Z at
/// zero), positive pitch looks up.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Look {
    pub yaw: f32,
    pub pitch: f32,
}

/// What the renderer needs to place the view.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraPose {
    /// Eye position, world space, metres.
    pub eye: Vec3,
    /// View orientation (yaw, then pitch, then roll); -Z is forward.
    pub orientation: Quat,
    /// Vertical field of view, radians.
    pub fov_y: f32,
    /// Roll, radians; positive is anticlockwise seen from behind the camera.
    pub roll: f32,
}

/// First-person camera with smoothed eye height, speed-driven field of view,
/// lateral roll and a landing dip.
#[derive(Debug, Clone)]
pub struct FirstPersonCamera {
    params: FeelParams,
    /// Smoothed eye height above the feet.
    eye_height: f32,
    fov_deg: f32,
    roll: f32,
    /// Vertical eye offset of the landing dip (negative is down).
    dip: CriticalSpring,
    was_on_ground: bool,
    /// Vertical speed of the last airborne frame.
    last_air_vy: f32,
}

impl FirstPersonCamera {
    /// A camera that starts at the standing pose.
    #[must_use]
    pub fn new(params: FeelParams) -> Self {
        Self {
            eye_height: params.eye_height_standing,
            fov_deg: params.fov_base_deg,
            roll: 0.0,
            dip: CriticalSpring::default(),
            was_on_ground: true,
            last_air_vy: 0.0,
            params,
        }
    }

    /// The settings in use.
    #[must_use]
    pub fn params(&self) -> &FeelParams {
        &self.params
    }

    /// Replaces the settings (the settings screen); the state is kept.
    pub fn set_params(&mut self, params: FeelParams) {
        self.params = params;
    }

    /// Smoothed eye height above the feet, metres.
    #[must_use]
    pub fn eye_height(&self) -> f32 {
        self.eye_height
    }

    /// Advances the camera by `dt` seconds and returns the pose to render.
    pub fn update(&mut self, dt: f32, player: &PlayerState, look: Look) -> CameraPose {
        let p = &self.params;
        let dt = dt.max(0.0);

        // Eye height: exponential approach, never an instant jump.
        let target = if player.crouched || is_sliding(player) {
            p.eye_height_sliding
        } else {
            p.eye_height_standing
        };
        self.eye_height += (target - self.eye_height) * approach(p.eye_rate, dt);

        // Field of view widens with horizontal speed beyond the run speed.
        let speed = Vec3::new(player.vel.x, 0.0, player.vel.z).length();
        let t = smoothstep((speed - p.fov_start_speed) / (p.fov_full_speed - p.fov_start_speed));
        let fov_target = p.fov_base_deg + p.fov_extra_deg * t;
        self.fov_deg += (fov_target - self.fov_deg) * approach(p.fov_rate, dt);

        // Roll toward the side of the lateral movement.
        let (sin, cos) = look.yaw.sin_cos();
        let right = Vec3::new(cos, 0.0, -sin);
        let lateral = (player.vel.dot(right) / p.roll_full_speed).clamp(-1.0, 1.0);
        let roll_target = -lateral * p.roll_max_deg.to_radians();
        self.roll += (roll_target - self.roll) * approach(p.roll_rate, dt);

        // Landing: the eye is kicked down in proportion to the fall speed.
        if !player.on_ground {
            self.last_air_vy = player.vel.y;
        } else if !self.was_on_ground {
            let fall = -self.last_air_vy;
            if fall >= p.landing_min_fall {
                self.dip.v -= p.landing_gain * fall;
            }
        }
        self.was_on_ground = player.on_ground;
        self.dip.step(0.0, p.landing_omega, dt);
        self.dip.x = self.dip.x.clamp(-p.landing_max_dip, 0.0);

        let eye = player.feet() + Vec3::Y * (self.eye_height + self.dip.x);
        let orientation = Quat::from_rotation_y(look.yaw)
            * Quat::from_rotation_x(look.pitch)
            * Quat::from_rotation_z(self.roll);
        CameraPose {
            eye,
            orientation,
            fov_y: self.fov_deg.to_radians(),
            roll: self.roll,
        }
    }
}

/// Fraction of the remaining distance covered in `dt` at `rate`.
fn approach(rate: f32, dt: f32) -> f32 {
    1.0 - (-rate * dt).exp()
}

fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 1.0 / 60.0;

    fn standing() -> PlayerState {
        let mut s = PlayerState::spawn(Vec3::new(0.0, 0.9, 0.0));
        s.on_ground = true;
        s
    }

    fn sliding() -> PlayerState {
        let mut s = standing();
        s.crouched = true;
        s.slide_timer = 1.0;
        s.pos.y = 0.5;
        s
    }

    fn settle(cam: &mut FirstPersonCamera, s: &PlayerState) -> CameraPose {
        let mut pose = cam.update(DT, s, Look::default());
        for _ in 0..120 {
            pose = cam.update(DT, s, Look::default());
        }
        pose
    }

    #[test]
    fn eye_is_1_6_standing_and_0_8_sliding_after_the_transition() {
        let mut cam = FirstPersonCamera::new(FeelParams::default());
        let pose = settle(&mut cam, &standing());
        assert!((pose.eye.y - 1.6).abs() < 1e-3, "{}", pose.eye.y);
        let pose = settle(&mut cam, &sliding());
        assert!((cam.eye_height() - 0.8).abs() < 1e-3);
        assert!((pose.eye.y - 0.8).abs() < 1e-3, "{}", pose.eye.y);
        let pose = settle(&mut cam, &standing());
        assert!((pose.eye.y - 1.6).abs() < 1e-3);
    }

    #[test]
    fn the_transition_is_smooth_not_instant() {
        let mut cam = FirstPersonCamera::new(FeelParams::default());
        settle(&mut cam, &standing());
        // One frame after the slide starts the eye has barely moved.
        cam.update(DT, &sliding(), Look::default());
        assert!(cam.eye_height() > 1.2, "eye jumped to {}", cam.eye_height());
        let mut prev = cam.eye_height();
        for _ in 0..30 {
            cam.update(DT, &sliding(), Look::default());
            assert!(cam.eye_height() <= prev);
            prev = cam.eye_height();
        }
    }

    #[test]
    fn fov_widens_beyond_run_speed_only() {
        let mut cam = FirstPersonCamera::new(FeelParams::default());
        let mut s = standing();
        s.vel = Vec3::new(0.0, 0.0, -7.0);
        let run = settle(&mut cam, &s).fov_y;
        assert!((run.to_degrees() - 75.0).abs() < 1e-2);
        s.vel = Vec3::new(0.0, 0.0, -20.0);
        let fast = settle(&mut cam, &s).fov_y;
        assert!(
            (fast.to_degrees() - 85.0).abs() < 0.05,
            "{}",
            fast.to_degrees()
        );
        s.vel = Vec3::new(0.0, 0.0, -13.0);
        let mid = settle(&mut cam, &s).fov_y;
        assert!(run < mid && mid < fast);
    }

    #[test]
    fn strafing_right_rolls_clockwise_and_forward_does_not_roll() {
        let mut cam = FirstPersonCamera::new(FeelParams::default());
        let mut s = standing();
        s.vel = Vec3::new(0.0, 0.0, -7.0);
        assert!(settle(&mut cam, &s).roll.abs() < 1e-4);
        s.vel = Vec3::new(7.0, 0.0, 0.0);
        let roll = settle(&mut cam, &s).roll;
        assert!(
            (roll.to_degrees() + 1.5).abs() < 0.01,
            "{}",
            roll.to_degrees()
        );
    }

    #[test]
    fn landing_dips_the_eye_in_proportion_to_the_fall_and_recovers() {
        let dip_for = |fall: f32| {
            let mut cam = FirstPersonCamera::new(FeelParams::default());
            let mut s = standing();
            settle(&mut cam, &s);
            s.on_ground = false;
            s.vel.y = -fall;
            cam.update(DT, &s, Look::default());
            s.on_ground = true;
            s.vel.y = 0.0;
            let mut deepest = 0.0f32;
            for _ in 0..90 {
                let pose = cam.update(DT, &s, Look::default());
                deepest = deepest.min(pose.eye.y - 1.6);
            }
            let end = cam.update(DT, &s, Look::default()).eye.y;
            assert!((end - 1.6).abs() < 2e-3, "does not recover: {end}");
            deepest
        };
        let small = dip_for(2.0);
        let medium = dip_for(6.0);
        let large = dip_for(12.0);
        assert_eq!(small, 0.0, "a short step does not dip");
        assert!(large < medium && medium < -0.01, "{medium} {large}");
        assert!(large >= -0.3 - 1e-4);
    }

    #[test]
    fn orientation_faces_minus_z_and_follows_yaw_and_pitch() {
        let mut cam = FirstPersonCamera::new(FeelParams::default());
        let s = standing();
        let pose = cam.update(DT, &s, Look::default());
        assert!((pose.orientation * Vec3::NEG_Z - Vec3::NEG_Z).length() < 1e-5);
        let quarter = std::f32::consts::FRAC_PI_2;
        let pose = cam.update(
            DT,
            &s,
            Look {
                yaw: quarter,
                pitch: 0.0,
            },
        );
        assert!((pose.orientation * Vec3::NEG_Z - Vec3::NEG_X).length() < 1e-4);
        let pose = cam.update(
            DT,
            &s,
            Look {
                yaw: 0.0,
                pitch: quarter,
            },
        );
        assert!((pose.orientation * Vec3::NEG_Z - Vec3::Y).length() < 1e-4);
    }
}

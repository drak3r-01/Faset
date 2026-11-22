//! Editor camera: free fly and orthographic top view.

use faset_core::glam::{Mat4, Quat, Vec2, Vec3, Vec4};
use faset_render::{perspective_reverse_z, Camera};

use crate::math::Ray;

/// Which view the editor uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CameraMode {
    /// Perspective camera flying freely.
    #[default]
    FreeFly,
    /// Orthographic view from above, north (-Z) at the top of the screen.
    TopOrtho,
}

/// Camera input of one frame.
#[derive(Debug, Clone, Copy, Default)]
pub struct CameraInput {
    /// Movement request: x right, y up, z forward, each in -1..=1. In the
    /// top view only x and z are used, as a pan.
    pub motion: Vec3,
    /// Look delta in radians (x yaw to the left, y pitch up); ignored in the
    /// top view.
    pub look: Vec2,
    /// Zoom request: positive zooms in the top view and moves forward in free
    /// fly (in metres).
    pub zoom: f32,
    /// Hold to move faster.
    pub fast: bool,
}

/// Position and orientation of the editor viewpoint.
#[derive(Debug, Clone, PartialEq)]
pub struct EditorCamera {
    pub mode: CameraMode,
    /// Free-fly eye position.
    pub eye: Vec3,
    /// Free-fly yaw, radians, positive turns left (forward is -Z at zero).
    pub yaw: f32,
    /// Free-fly pitch, radians, positive looks up.
    pub pitch: f32,
    /// Vertical field of view of the free-fly view, radians.
    pub fov_y: f32,
    /// Look-at point of the top view on the ground plane (x, z).
    pub top_centre: Vec2,
    /// Half of the visible height of the top view, metres.
    pub top_half_height: f32,
    /// Free-fly speed in m/s.
    pub speed: f32,
    prev_view_proj: Option<Mat4>,
}

/// Height of the top view's eye above the origin plane, metres.
const TOP_EYE_Y: f32 = 500.0;
const TOP_FAR: f32 = 1000.0;
const NEAR: f32 = 0.1;
/// How far in front of the free-fly camera [`EditorCamera::focus`] puts the
/// framed point, in metres (decision).
pub const FOCUS_DISTANCE: f32 = 10.0;

impl Default for EditorCamera {
    /// A free-fly camera 8 m above the origin looking along -Z and down.
    fn default() -> Self {
        Self {
            mode: CameraMode::FreeFly,
            eye: Vec3::new(0.0, 8.0, 12.0),
            yaw: 0.0,
            pitch: -0.4,
            fov_y: 70f32.to_radians(),
            top_centre: Vec2::ZERO,
            top_half_height: 30.0,
            speed: 12.0,
            prev_view_proj: None,
        }
    }
}

impl EditorCamera {
    /// Orientation of the free-fly view (yaw about +Y, then pitch).
    #[must_use]
    pub fn orientation(&self) -> Quat {
        Quat::from_rotation_y(self.yaw) * Quat::from_rotation_x(self.pitch)
    }

    /// Switches between free fly and the top view. The top view centres on
    /// where the free camera is looking at the ground (or just below it).
    pub fn toggle_mode(&mut self) {
        match self.mode {
            CameraMode::FreeFly => {
                let forward = self.orientation() * Vec3::NEG_Z;
                let centre = if forward.y < -0.05 {
                    self.eye + forward * (-self.eye.y / forward.y)
                } else {
                    self.eye
                };
                self.top_centre = Vec2::new(centre.x, centre.z);
                self.mode = CameraMode::TopOrtho;
            }
            CameraMode::TopOrtho => self.mode = CameraMode::FreeFly,
        }
    }

    /// Frames `p`: the top view centres on it, the free-fly view backs off
    /// [`FOCUS_DISTANCE`] metres along its current direction so that `p`
    /// sits at the centre of the screen.
    pub fn focus(&mut self, p: Vec3) {
        match self.mode {
            CameraMode::TopOrtho => self.top_centre = Vec2::new(p.x, p.z),
            CameraMode::FreeFly => {
                self.eye = p - self.orientation() * Vec3::NEG_Z * FOCUS_DISTANCE;
            }
        }
    }

    /// The point at the centre of the screen: [`FOCUS_DISTANCE`] metres
    /// ahead in free fly, the ground point under the centre in the top view.
    #[must_use]
    pub fn target(&self) -> Vec3 {
        match self.mode {
            CameraMode::TopOrtho => Vec3::new(self.top_centre.x, 0.0, self.top_centre.y),
            CameraMode::FreeFly => self.eye + self.orientation() * Vec3::NEG_Z * FOCUS_DISTANCE,
        }
    }

    /// Advances the camera by `dt` seconds.
    pub fn update(&mut self, dt: f32, input: &CameraInput) {
        let boost = if input.fast { 3.0 } else { 1.0 };
        match self.mode {
            CameraMode::FreeFly => {
                self.yaw += input.look.x;
                self.pitch = (self.pitch + input.look.y).clamp(-1.55, 1.55);
                let local = Vec3::new(input.motion.x, 0.0, -input.motion.z);
                let mut v = self.orientation() * local;
                v.y += input.motion.y;
                self.eye += v * self.speed * boost * dt;
                self.eye += self.orientation() * Vec3::NEG_Z * input.zoom;
            }
            CameraMode::TopOrtho => {
                let pan = Vec2::new(input.motion.x, -input.motion.z);
                self.top_centre += pan * self.top_half_height * 1.2 * boost * dt;
                if input.zoom != 0.0 {
                    self.top_half_height =
                        (self.top_half_height * (1.0 - 0.1 * input.zoom)).clamp(2.0, 400.0);
                }
            }
        }
    }

    /// The render camera for a viewport of `aspect` = width / height. The
    /// previous frame's matrix is remembered for motion vectors.
    pub fn render_camera(&mut self, aspect: f32) -> Camera {
        let (view, proj) = self.matrices(aspect);
        let current = proj * view;
        let mut camera = Camera::new(view, proj);
        camera.prev_view_proj = self.prev_view_proj.unwrap_or(current);
        self.prev_view_proj = Some(current);
        camera
    }

    /// View and projection matrices (reverse Z, Vulkan y flip) of the
    /// current mode.
    #[must_use]
    pub fn matrices(&self, aspect: f32) -> (Mat4, Mat4) {
        match self.mode {
            CameraMode::FreeFly => (
                Mat4::from_rotation_translation(self.orientation(), self.eye).inverse(),
                perspective_reverse_z(self.fov_y, aspect, NEAR),
            ),
            CameraMode::TopOrtho => {
                let eye = Vec3::new(self.top_centre.x, TOP_EYE_Y, self.top_centre.y);
                // Looking down -Y with -Z up on the screen.
                let view = faset_core::glam::camera::rh::view::look_at_mat4(
                    eye,
                    Vec3::new(eye.x, 0.0, eye.z),
                    Vec3::NEG_Z,
                );
                let half_h = self.top_half_height;
                (
                    view,
                    orthographic_reverse_z(half_h * aspect, half_h, NEAR, TOP_FAR),
                )
            }
        }
    }

    /// The world ray under a pixel of a `size` viewport (pixel 0,0 is the
    /// top-left corner).
    #[must_use]
    pub fn ray_from_pixel(&self, pixel: Vec2, size: Vec2) -> Ray {
        let aspect = size.x / size.y.max(1.0);
        let (view, proj) = self.matrices(aspect);
        let ndc = Vec2::new(
            2.0 * pixel.x / size.x.max(1.0) - 1.0,
            2.0 * pixel.y / size.y.max(1.0) - 1.0,
        );
        ray_from_ndc(proj * view, ndc)
    }

    /// Distance from the viewpoint to `p`, used to keep handles a constant
    /// size on screen. In the top view this is the half height scaled to the
    /// equivalent perspective distance.
    #[must_use]
    pub fn distance_to(&self, p: Vec3) -> f32 {
        match self.mode {
            CameraMode::FreeFly => self.eye.distance(p),
            CameraMode::TopOrtho => self.top_half_height / (0.5 * self.fov_y).tan(),
        }
    }
}

/// Reverse-Z orthographic projection with the Vulkan y flip: NDC depth is 1
/// at `near` and 0 at `far` (distances along -Z in view space).
#[must_use]
pub fn orthographic_reverse_z(half_w: f32, half_h: f32, near: f32, far: f32) -> Mat4 {
    let range = far - near;
    Mat4::from_cols(
        Vec4::new(1.0 / half_w, 0.0, 0.0, 0.0),
        Vec4::new(0.0, -1.0 / half_h, 0.0, 0.0),
        Vec4::new(0.0, 0.0, 1.0 / range, 0.0),
        Vec4::new(0.0, 0.0, far / range, 1.0),
    )
}

/// The world ray through an NDC point (x right, y down, as in Vulkan) for a
/// clip-from-world matrix `view_proj` with reverse Z.
#[must_use]
pub fn ray_from_ndc(view_proj: Mat4, ndc: Vec2) -> Ray {
    let inv = view_proj.inverse();
    let unproject = |z: f32| {
        let p = inv * Vec4::new(ndc.x, ndc.y, z, 1.0);
        p.truncate() / p.w
    };
    let near = unproject(1.0);
    let mid = unproject(0.5);
    Ray::new(near, mid - near)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: Vec2 = Vec2::new(1600.0, 900.0);

    #[test]
    fn centre_pixel_ray_follows_the_view_direction() {
        let cam = EditorCamera::default();
        let ray = cam.ray_from_pixel(SIZE * 0.5, SIZE);
        let forward = cam.orientation() * Vec3::NEG_Z;
        assert!(ray.dir.dot(forward) > 0.9999, "{ray:?}");
        assert!(ray.origin.distance(cam.eye) < 0.2);
        // Right side of the screen points to the right of the view.
        let right = cam.ray_from_pixel(Vec2::new(1500.0, 450.0), SIZE);
        assert!(right.dir.dot(cam.orientation() * Vec3::X) > 0.1);
        // The top of the screen points up.
        let top = cam.ray_from_pixel(Vec2::new(800.0, 10.0), SIZE);
        assert!(top.dir.dot(cam.orientation() * Vec3::Y) > 0.1);
    }

    #[test]
    fn top_view_rays_are_vertical_and_map_pixels_to_the_ground() {
        let cam = EditorCamera {
            mode: CameraMode::TopOrtho,
            top_centre: Vec2::new(10.0, 20.0),
            top_half_height: 30.0,
            ..EditorCamera::default()
        };
        let centre = cam.ray_from_pixel(SIZE * 0.5, SIZE);
        assert!(centre.dir.dot(Vec3::NEG_Y) > 0.99999);
        let t = (0.0 - centre.origin.y) / centre.dir.y;
        let hit = centre.at(t);
        assert!(
            (hit.x - 10.0).abs() < 1e-3 && (hit.z - 20.0).abs() < 1e-3,
            "{hit}"
        );
        // Top of the screen is -Z, right of the screen is +X.
        let top = cam.ray_from_pixel(Vec2::new(800.0, 0.0), SIZE);
        let hit = top.at((0.0 - top.origin.y) / top.dir.y);
        assert!((hit.z - (20.0 - 30.0)).abs() < 1e-2, "{hit}");
        let right = cam.ray_from_pixel(Vec2::new(1600.0, 450.0), SIZE);
        let hit = right.at((0.0 - right.origin.y) / right.dir.y);
        assert!(
            (hit.x - (10.0 + 30.0 * SIZE.x / SIZE.y)).abs() < 1e-2,
            "{hit}"
        );
    }

    #[test]
    fn orthographic_projection_is_reverse_z() {
        let p = orthographic_reverse_z(10.0, 5.0, 0.1, 100.0);
        let ndc = |v: Vec3| {
            let c = p * v.extend(1.0);
            c.truncate() / c.w
        };
        assert!((ndc(Vec3::new(0.0, 0.0, -0.1)).z - 1.0).abs() < 1e-5);
        assert!(ndc(Vec3::new(0.0, 0.0, -100.0)).z.abs() < 1e-5);
        // y up is negative NDC y (Vulkan).
        assert!(
            (ndc(Vec3::new(10.0, 5.0, -1.0))
                - Vec3::new(1.0, -1.0, ndc(Vec3::new(0.0, 0.0, -1.0)).z))
            .length()
                < 1e-5
        );
    }

    #[test]
    fn free_fly_moves_along_the_view_and_clamps_pitch() {
        let mut cam = EditorCamera {
            pitch: 0.0,
            eye: Vec3::ZERO,
            ..EditorCamera::default()
        };
        cam.update(
            1.0,
            &CameraInput {
                motion: Vec3::new(0.0, 0.0, 1.0),
                ..CameraInput::default()
            },
        );
        assert!((cam.eye - Vec3::new(0.0, 0.0, -cam.speed)).length() < 1e-4);
        cam.update(
            0.0,
            &CameraInput {
                look: Vec2::new(0.0, 10.0),
                ..CameraInput::default()
            },
        );
        assert!(cam.pitch <= 1.55);
    }

    #[test]
    fn toggling_modes_keeps_the_looked_at_ground_point() {
        let mut cam = EditorCamera::default();
        cam.toggle_mode();
        assert_eq!(cam.mode, CameraMode::TopOrtho);
        // Default eye (0, 8, 12) pitched down 0.4 rad hits the ground ahead.
        assert!(cam.top_centre.y < 12.0);
        cam.toggle_mode();
        assert_eq!(cam.mode, CameraMode::FreeFly);
    }

    #[test]
    fn render_camera_remembers_the_previous_matrix() {
        let mut cam = EditorCamera::default();
        let a = cam.render_camera(1.5);
        assert_eq!(a.prev_view_proj, a.view_proj());
        cam.eye.x += 1.0;
        let b = cam.render_camera(1.5);
        assert_eq!(b.prev_view_proj, a.view_proj());
        assert_ne!(b.view_proj(), a.view_proj());
    }
}

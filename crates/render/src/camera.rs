//! Camera, reverse-Z infinite projection, sub-pixel jitter and the motion
//! vector formula.

use faset_core::glam::{Mat4, Vec2, Vec4};

/// Number of jitter samples before the sequence repeats.
pub const JITTER_SAMPLES: u32 = 16;

/// What the renderer needs to know about the viewpoint of one frame.
///
/// `proj` and `prev_view_proj` carry no jitter: the renderer applies
/// `jitter_px` itself, and motion vectors use the unjittered matrices.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    /// View-from-world.
    pub view: Mat4,
    /// Clip-from-view, unjittered (see [`perspective_reverse_z`]).
    pub proj: Mat4,
    /// Unjittered clip-from-world of the previous frame.
    pub prev_view_proj: Mat4,
    /// Sub-pixel offset of this frame in pixels, each axis in [-0.5, 0.5]
    /// (see [`jitter`]). x points right, y points down.
    pub jitter_px: Vec2,
}

impl Camera {
    /// A camera whose previous matrix equals the current one, without jitter.
    pub fn new(view: Mat4, proj: Mat4) -> Self {
        Self {
            view,
            proj,
            prev_view_proj: proj * view,
            jitter_px: Vec2::ZERO,
        }
    }

    /// Unjittered clip-from-world of this frame.
    pub fn view_proj(&self) -> Mat4 {
        self.proj * self.view
    }
}

impl Default for Camera {
    /// Identity view, 60 degree square projection, near 0.1 m.
    fn default() -> Self {
        Self::new(
            Mat4::IDENTITY,
            perspective_reverse_z(60f32.to_radians(), 1.0, 0.1),
        )
    }
}

/// Reverse-Z projection with an infinite far plane and the Vulkan y flip.
///
/// `fov_y` in radians, `aspect` = width / height, `near` in metres. View
/// space is right-handed, -Z forward; NDC depth is `near / -z_view`, so the
/// near plane maps to 1 and infinity to 0 (clear 0, compare GREATER_OR_EQUAL).
pub fn perspective_reverse_z(fov_y: f32, aspect: f32, near: f32) -> Mat4 {
    let f = 1.0 / (0.5 * fov_y).tan();
    Mat4::from_cols(
        Vec4::new(f / aspect, 0.0, 0.0, 0.0),
        Vec4::new(0.0, -f, 0.0, 0.0),
        Vec4::new(0.0, 0.0, 0.0, -1.0),
        Vec4::new(0.0, 0.0, near, 0.0),
    )
}

/// The `index`-th value of the radical inverse in `base` (index 0 gives 0).
pub fn halton(mut index: u32, base: u32) -> f32 {
    let mut result = 0.0f64;
    let mut fraction = 1.0f64;
    let inv_base = 1.0 / f64::from(base);
    while index > 0 {
        fraction *= inv_base;
        result += fraction * f64::from(index % base);
        index /= base;
    }
    result as f32
}

/// Sub-pixel jitter of a frame in pixels, each axis in [-0.5, 0.5].
///
/// Halton bases 2 (x) and 3 (y), 16 samples: frame `i` uses H_k with
/// k = (i mod 16) + 1, skipping the H_0 = 0 sample that would sit on a
/// corner. The NDC offset is `2 * jitter / resolution` ([`jitter_ndc`]).
pub fn jitter(frame_index: u32) -> Vec2 {
    let k = frame_index % JITTER_SAMPLES + 1;
    Vec2::new(halton(k, 2) - 0.5, halton(k, 3) - 0.5)
}

/// Converts a pixel jitter to NDC units for a target of `resolution` pixels:
/// j = 2 (H - 0.5) / resolution.
pub fn jitter_ndc(jitter_px: Vec2, resolution: (u32, u32)) -> Vec2 {
    Vec2::new(
        2.0 * jitter_px.x / resolution.0.max(1) as f32,
        2.0 * jitter_px.y / resolution.1.max(1) as f32,
    )
}

/// Applies an NDC jitter to a projection built by [`perspective_reverse_z`].
///
/// Decision (sign): the offset is subtracted from P[2][0] and P[2][1]
/// (column 2, rows 0 and 1). Since clip w = -z_view, this moves every
/// projected point by exactly +`jitter_ndc` in NDC, so a positive jitter
/// shifts the image right (x) and down (y, Vulkan NDC). The opaque shader
/// applies the same offset as `clip.xy += jitter * clip.w`.
pub fn jittered_projection(proj: Mat4, jitter_ndc: Vec2) -> Mat4 {
    let mut p = proj;
    p.z_axis.x -= jitter_ndc.x;
    p.z_axis.y -= jitter_ndc.y;
    p
}

/// Motion vector of a point from its current and previous unjittered clip
/// positions: m = (c.xy / c.w - c'.xy / c'.w) / 2, in UV units (the opaque
/// shader computes the same per pixel).
pub fn motion_vector(clip: Vec4, prev_clip: Vec4) -> Vec2 {
    0.5 * (clip.truncate().truncate() / clip.w - prev_clip.truncate().truncate() / prev_clip.w)
}

#[cfg(test)]
mod tests {
    use super::*;
    use faset_core::glam::Vec3;

    #[test]
    fn halton_matches_reference_for_indices_1_to_16() {
        let base2 = [
            1.0 / 2.0,
            1.0 / 4.0,
            3.0 / 4.0,
            1.0 / 8.0,
            5.0 / 8.0,
            3.0 / 8.0,
            7.0 / 8.0,
            1.0 / 16.0,
            9.0 / 16.0,
            5.0 / 16.0,
            13.0 / 16.0,
            3.0 / 16.0,
            11.0 / 16.0,
            7.0 / 16.0,
            15.0 / 16.0,
            1.0 / 32.0,
        ];
        let base3 = [
            1.0 / 3.0,
            2.0 / 3.0,
            1.0 / 9.0,
            4.0 / 9.0,
            7.0 / 9.0,
            2.0 / 9.0,
            5.0 / 9.0,
            8.0 / 9.0,
            1.0 / 27.0,
            10.0 / 27.0,
            19.0 / 27.0,
            4.0 / 27.0,
            13.0 / 27.0,
            22.0 / 27.0,
            7.0 / 27.0,
            16.0 / 27.0,
        ];
        for k in 1..=16u32 {
            let i = (k - 1) as usize;
            assert!((halton(k, 2) - base2[i]).abs() < 1e-6, "base 2, k = {k}");
            assert!((halton(k, 3) - base3[i]).abs() < 1e-6, "base 3, k = {k}");
        }
    }

    #[test]
    fn jitter_cycles_every_16_frames_and_stays_sub_pixel() {
        for frame in 0..16u32 {
            let j = jitter(frame);
            assert_eq!(j, jitter(frame + 16));
            assert!(j.x.abs() <= 0.5 && j.y.abs() <= 0.5);
            let k = frame + 1;
            assert!((j.x - (halton(k, 2) - 0.5)).abs() < 1e-6);
            assert!((j.y - (halton(k, 3) - 0.5)).abs() < 1e-6);
        }
        // Mean close to zero over a full cycle.
        let sum: Vec2 = (0..16).map(jitter).sum();
        assert!((sum / 16.0).abs().max_element() < 0.05);
    }

    #[test]
    fn projection_is_reverse_z_with_infinite_far_plane() {
        let near = 0.1;
        let p = perspective_reverse_z(90f32.to_radians(), 2.0, near);
        let ndc = |v: Vec3| {
            let c = p * v.extend(1.0);
            c.truncate() / c.w
        };
        assert!((ndc(Vec3::new(0.0, 0.0, -near)).z - 1.0).abs() < 1e-6);
        assert!((ndc(Vec3::new(0.0, 0.0, -10.0)).z - 0.01).abs() < 1e-6);
        let far = ndc(Vec3::new(0.0, 0.0, -1.0e9)).z;
        assert!(far > 0.0 && far < 1e-9);
        // y up in view space lands at negative NDC y (Vulkan y down).
        assert!(ndc(Vec3::new(0.0, 1.0, -1.0)).y < 0.0);
        // f = 1 for 90 degrees; x is divided by the aspect.
        assert!((ndc(Vec3::new(1.0, 0.0, -1.0)).x - 0.5).abs() < 1e-6);
    }

    #[test]
    fn jittered_projection_shifts_ndc_by_the_jitter() {
        let p = perspective_reverse_z(70f32.to_radians(), 16.0 / 9.0, 0.05);
        let j = jitter_ndc(jitter(3), (1920, 1080));
        let pj = jittered_projection(p, j);
        for v in [Vec3::new(0.3, -2.0, -5.0), Vec3::new(-7.0, 1.0, -40.0)] {
            let a = p * v.extend(1.0);
            let b = pj * v.extend(1.0);
            let shift = b.truncate().truncate() / b.w - a.truncate().truncate() / a.w;
            assert!((shift - j).abs().max_element() < 1e-6);
            // Same as the shader form clip.xy += j * w.
            let shader = a.truncate().truncate() + j * a.w;
            assert!((shader - b.truncate().truncate()).abs().max_element() < 1e-5);
            // Depth is untouched.
            assert_eq!(a.z / a.w, b.z / b.w);
        }
    }

    #[test]
    fn static_scene_gives_zero_motion() {
        let cam = Camera::new(
            faset_core::glam::camera::rh::view::look_at_mat4(
                Vec3::new(3.0, 4.0, 5.0),
                Vec3::ZERO,
                Vec3::Y,
            ),
            perspective_reverse_z(1.0, 1.5, 0.1),
        );
        let world = Mat4::from_translation(Vec3::new(1.0, 0.0, -2.0));
        let p = Vec3::new(0.5, 0.5, 0.5).extend(1.0);
        let m = motion_vector(cam.view_proj() * world * p, cam.prev_view_proj * world * p);
        assert_eq!(m, Vec2::ZERO);
    }

    #[test]
    fn translated_camera_gives_expected_motion() {
        let proj = perspective_reverse_z(90f32.to_radians(), 1.0, 0.1);
        let prev_view = Mat4::IDENTITY;
        // The camera moved 0.5 m to the right.
        let d = 0.5;
        let view = Mat4::from_translation(Vec3::new(-d, 0.0, 0.0));
        let cam = Camera {
            view,
            proj,
            prev_view_proj: proj * prev_view,
            jitter_px: Vec2::ZERO,
        };
        let depth = 4.0;
        let p = Vec3::new(1.0, 0.25, -depth).extend(1.0);
        let m = motion_vector(cam.view_proj() * p, cam.prev_view_proj * p);
        // The world appears to move left: m.x = -0.5 * (f / a) * d / depth.
        assert!((m.x - (-0.5 * d / depth)).abs() < 1e-6, "{m}");
        assert!(m.y.abs() < 1e-7);
    }
}

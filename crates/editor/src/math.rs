//! Rays, boxes and the axis-drag formula.

use faset_core::glam::Vec3;

/// A ray: `origin + t * dir`, `dir` normalised.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ray {
    pub origin: Vec3,
    pub dir: Vec3,
}

impl Ray {
    /// A ray with a normalised direction.
    #[must_use]
    pub fn new(origin: Vec3, dir: Vec3) -> Self {
        Self {
            origin,
            dir: dir.normalize_or_zero(),
        }
    }

    /// The point at parameter `t`.
    #[must_use]
    pub fn at(&self, t: f32) -> Vec3 {
        self.origin + self.dir * t
    }
}

/// Axis-aligned box in metres.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    /// The box spanning two corners in any order.
    #[must_use]
    pub fn from_corners(a: Vec3, b: Vec3) -> Self {
        Self {
            min: a.min(b),
            max: a.max(b),
        }
    }

    /// The box grown by `m` metres on every side.
    #[must_use]
    pub fn inflated(&self, m: f32) -> Self {
        Self {
            min: self.min - Vec3::splat(m),
            max: self.max + Vec3::splat(m),
        }
    }

    /// Whether the two boxes overlap (touching counts).
    #[must_use]
    pub fn intersects(&self, other: &Self) -> bool {
        self.min.cmple(other.max).all() && other.min.cmple(self.max).all()
    }

    /// Centre of the box.
    #[must_use]
    pub fn centre(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }

    /// Smallest box containing both.
    #[must_use]
    pub fn union(&self, other: &Self) -> Self {
        Self {
            min: self.min.min(other.min),
            max: self.max.max(other.max),
        }
    }
}

/// Distance along the ray to the box (slab test), `None` on a miss. A ray
/// starting inside the box hits at 0.
#[must_use]
pub fn ray_aabb(origin: Vec3, dir: Vec3, b: &Aabb) -> Option<f32> {
    let mut t_min = 0.0f32;
    let mut t_max = f32::INFINITY;
    for i in 0..3 {
        if dir[i].abs() < 1e-9 {
            if origin[i] < b.min[i] || origin[i] > b.max[i] {
                return None;
            }
        } else {
            let inv = 1.0 / dir[i];
            let (mut t0, mut t1) = ((b.min[i] - origin[i]) * inv, (b.max[i] - origin[i]) * inv);
            if t0 > t1 {
                std::mem::swap(&mut t0, &mut t1);
            }
            t_min = t_min.max(t0);
            t_max = t_max.min(t1);
            if t_min > t_max {
                return None;
            }
        }
    }
    Some(t_min)
}

/// Distance along the ray to the horizontal plane `y = height`; `None` when
/// parallel or behind the origin.
#[must_use]
pub fn ray_horizontal_plane(ray: &Ray, height: f32) -> Option<f32> {
    if ray.dir.y.abs() < 1e-6 {
        return None;
    }
    let t = (height - ray.origin.y) / ray.dir.y;
    (t >= 0.0).then_some(t)
}

/// Axis-drag parameter: the point of the line `P1 + s * d1` closest to the
/// line `P2 + t * d2` (the mouse ray), returned as `s`.
///
/// With `r = P1 - P2`:
///
/// ```text
/// s = ((d1.d2)(d2.r) - (d2.d2)(d1.r)) / ((d1.d1)(d2.d2) - (d1.d2)^2)
/// ```
///
/// Decision: when the denominator is near zero (the mouse ray is parallel to
/// the axis, relative threshold 1e-6) there is no closest point and `None` is
/// returned: the caller keeps the previous position.
#[must_use]
pub fn axis_drag_param(p1: Vec3, d1: Vec3, p2: Vec3, d2: Vec3) -> Option<f32> {
    let r = p1 - p2;
    let (d1d1, d2d2, d1d2) = (d1.dot(d1), d2.dot(d2), d1.dot(d2));
    let den = d1d1 * d2d2 - d1d2 * d1d2;
    if den.abs() <= 1e-6 * (d1d1 * d2d2).max(f32::MIN_POSITIVE) {
        return None;
    }
    Some((d1d2 * d2.dot(r) - d2d2 * d1.dot(r)) / den)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axis_drag_finds_the_closest_point_on_the_axis() {
        // Vertical mouse ray above x = 3 on the X axis.
        let s = axis_drag_param(Vec3::ZERO, Vec3::X, Vec3::new(3.0, 5.0, 0.0), Vec3::NEG_Y);
        assert!((s.unwrap() - 3.0).abs() < 1e-5);
    }

    #[test]
    fn axis_drag_handles_skew_lines_and_scaled_directions() {
        // Axis through (1, 2, 3) along (0, 1, 1); mouse ray along (1, 0, 0)
        // at y = 10, z = 0. Closest axis point: minimise over s.
        let p1 = Vec3::new(1.0, 2.0, 3.0);
        let d1 = Vec3::new(0.0, 1.0, 1.0);
        let p2 = Vec3::new(-4.0, 10.0, 0.0);
        let d2 = Vec3::new(2.0, 0.0, 0.0);
        let s = axis_drag_param(p1, d1, p2, d2).unwrap();
        let q = p1 + s * d1;
        // The closest point has a connecting segment perpendicular to both.
        let t = (q - p2).dot(d2) / d2.dot(d2);
        let gap = q - (p2 + t * d2);
        assert!(
            gap.dot(d1).abs() < 1e-4 && gap.dot(d2).abs() < 1e-4,
            "{gap}"
        );
        // Brute-force check of the minimum.
        let dist = |s: f32| {
            let q = p1 + s * d1;
            let t = (q - p2).dot(d2) / d2.dot(d2);
            (q - (p2 + t * d2)).length()
        };
        assert!(dist(s) <= dist(s + 0.01) + 1e-6 && dist(s) <= dist(s - 0.01) + 1e-6);
    }

    #[test]
    fn parallel_rays_have_no_solution() {
        let none = axis_drag_param(Vec3::ZERO, Vec3::X, Vec3::new(0.0, 4.0, 1.0), Vec3::X);
        assert_eq!(none, None);
        let anti = axis_drag_param(Vec3::ZERO, Vec3::X, Vec3::new(0.0, 4.0, 1.0), -Vec3::X);
        assert_eq!(anti, None);
        // Nearly parallel is still rejected, clearly skew is accepted.
        assert_eq!(
            axis_drag_param(
                Vec3::ZERO,
                Vec3::X,
                Vec3::new(0.0, 4.0, 1.0),
                Vec3::new(1.0, 1e-5, 0.0)
            ),
            None
        );
        assert!(axis_drag_param(Vec3::ZERO, Vec3::X, Vec3::new(0.0, 4.0, 1.0), Vec3::Z).is_some());
    }

    #[test]
    fn ray_box_hits_and_misses() {
        let b = Aabb::from_corners(Vec3::ZERO, Vec3::ONE);
        let t = ray_aabb(Vec3::new(0.5, 0.5, 5.0), Vec3::NEG_Z, &b).unwrap();
        assert!((t - 4.0).abs() < 1e-6);
        assert_eq!(ray_aabb(Vec3::new(2.0, 0.5, 5.0), Vec3::NEG_Z, &b), None);
        assert_eq!(ray_aabb(Vec3::new(0.5, 0.5, 5.0), Vec3::Z, &b), None);
        assert_eq!(ray_aabb(Vec3::splat(0.5), Vec3::X, &b), Some(0.0));
    }
}

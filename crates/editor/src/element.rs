//! Selectable map elements, their bounds, and exact grid transforms of brushes.

use faset_core::glam::{Quat, Vec3};
use faset_mapfmt::{grid_to_metres, Axis, Brush, Dir, GridPos, MapFile, Prop};

use crate::math::{ray_aabb, Aabb, Ray};

/// What is selected: an index into the document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Selection {
    Brush(usize),
    Prop(usize),
}

/// A copy of a selected element, used by drags and commands.
#[derive(Debug, Clone, PartialEq)]
pub enum Element {
    Brush(Brush),
    Prop(Prop),
}

impl Selection {
    /// The element in the document, if the index is still valid.
    #[must_use]
    pub fn get(&self, map: &MapFile) -> Option<Element> {
        match *self {
            Self::Brush(i) => map.brushes.get(i).cloned().map(Element::Brush),
            Self::Prop(i) => map.props.get(i).cloned().map(Element::Prop),
        }
    }

    /// Writes `element` over the selected one. Returns false (and writes
    /// nothing) when the index or the kind does not match.
    pub fn set(&self, map: &mut MapFile, element: Element) -> bool {
        match (*self, element) {
            (Self::Brush(i), Element::Brush(b)) if i < map.brushes.len() => {
                map.brushes[i] = b;
                true
            }
            (Self::Prop(i), Element::Prop(p)) if i < map.props.len() => {
                map.props[i] = p;
                true
            }
            _ => false,
        }
    }
}

/// Local bounds of a prop recipe, relative to the prop origin, before yaw and
/// scale. The asset library (lot 5.3) provides the real ones.
pub trait PropBounds {
    /// Local box of `recipe`.
    fn bounds(&self, recipe: &str) -> Aabb;
}

/// Decision: until a library is plugged in, every prop is a 1 m cube standing
/// on its origin.
#[derive(Debug, Clone, Copy, Default)]
pub struct UnitPropBounds;

impl PropBounds for UnitPropBounds {
    fn bounds(&self, _recipe: &str) -> Aabb {
        Aabb {
            min: Vec3::new(-0.5, 0.0, -0.5),
            max: Vec3::new(0.5, 1.0, 0.5),
        }
    }
}

/// World-space bounds of a brush in grid units: `(min, max)`.
#[must_use]
pub fn brush_grid_bounds(brush: &Brush) -> (GridPos, GridPos) {
    match brush {
        Brush::Box { min, max, .. }
        | Brush::Wedge { min, max, .. }
        | Brush::Cylinder { min, max, .. }
        | Brush::Stairs { min, max, .. } => (*min, *max),
        Brush::Prism {
            points,
            y_min,
            y_max,
            ..
        } => {
            let lo_x = points.iter().map(|p| p[0]).min().unwrap_or(0);
            let hi_x = points.iter().map(|p| p[0]).max().unwrap_or(0);
            let lo_z = points.iter().map(|p| p[1]).min().unwrap_or(0);
            let hi_z = points.iter().map(|p| p[1]).max().unwrap_or(0);
            ([lo_x, *y_min, lo_z], [hi_x, *y_max, hi_z])
        }
    }
}

/// Bounds of a brush in metres.
#[must_use]
pub fn brush_aabb(brush: &Brush) -> Aabb {
    let (min, max) = brush_grid_bounds(brush);
    Aabb {
        min: grid_to_metres(min),
        max: grid_to_metres(max),
    }
}

/// Rotation of a prop about +Y, degrees counter-clockwise seen from above.
#[must_use]
pub fn prop_rotation(prop: &Prop) -> Quat {
    Quat::from_rotation_y(prop.yaw.to_radians())
}

/// Distance along the ray to a prop's oriented bounds, `None` on a miss.
#[must_use]
pub fn ray_prop(ray: &Ray, prop: &Prop, bounds: &dyn PropBounds) -> Option<f32> {
    let at = Vec3::from_array(prop.at);
    let inv = prop_rotation(prop).inverse();
    let scale = prop.scale.max(1e-4);
    // Rotation and uniform scale keep the ray parameter unchanged.
    let origin = inv * (ray.origin - at) / scale;
    let dir = inv * ray.dir / scale;
    ray_aabb(origin, dir, &bounds.bounds(&prop.recipe))
}

/// World-space centre of the selection's bounds (the handles' origin).
#[must_use]
pub fn element_centre(element: &Element, bounds: &dyn PropBounds) -> Vec3 {
    match element {
        Element::Brush(b) => brush_aabb(b).centre(),
        Element::Prop(p) => {
            let local = bounds.bounds(&p.recipe).centre() * p.scale;
            Vec3::from_array(p.at) + prop_rotation(p) * local
        }
    }
}

/// Moves a brush by a grid offset.
#[must_use]
pub fn translate_brush(brush: &Brush, d: GridPos) -> Brush {
    let add = |p: GridPos| [p[0] + d[0], p[1] + d[1], p[2] + d[2]];
    let mut b = brush.clone();
    match &mut b {
        Brush::Box { min, max, .. }
        | Brush::Wedge { min, max, .. }
        | Brush::Cylinder { min, max, .. }
        | Brush::Stairs { min, max, .. } => {
            *min = add(*min);
            *max = add(*max);
        }
        Brush::Prism {
            points,
            y_min,
            y_max,
            ..
        } => {
            for p in points.iter_mut() {
                p[0] += d[0];
                p[1] += d[2];
            }
            *y_min += d[1];
            *y_max += d[1];
        }
    }
    b
}

fn rotate_dir(d: Dir) -> Dir {
    // A quarter turn about +Y, counter-clockwise seen from above:
    // (x, z) -> (z, -x).
    match d {
        Dir::PosX => Dir::NegZ,
        Dir::NegZ => Dir::NegX,
        Dir::NegX => Dir::PosZ,
        Dir::PosZ => Dir::PosX,
    }
}

/// Rotates a brush by `turns` quarter turns about +Y (counter-clockwise seen
/// from above) around the vertical line through grid point `pivot = (x, z)`.
/// Exact on the integer grid.
#[must_use]
pub fn rotate_brush(brush: &Brush, pivot: [i32; 2], turns: i32) -> Brush {
    let mut b = brush.clone();
    for _ in 0..turns.rem_euclid(4) {
        let rot = |x: i32, z: i32| [pivot[0] + (z - pivot[1]), pivot[1] - (x - pivot[0])];
        match &mut b {
            Brush::Box { min, max, .. } => rotate_box(min, max, rot),
            Brush::Wedge { min, max, rise, .. } | Brush::Stairs { min, max, rise, .. } => {
                rotate_box(min, max, rot);
                *rise = rotate_dir(*rise);
            }
            Brush::Cylinder { min, max, axis, .. } => {
                rotate_box(min, max, rot);
                *axis = match axis {
                    Axis::X => Axis::Z,
                    Axis::Z => Axis::X,
                    Axis::Y => Axis::Y,
                };
            }
            Brush::Prism { points, .. } => {
                for p in points.iter_mut() {
                    *p = rot(p[0], p[1]);
                }
            }
        }
    }
    b
}

fn rotate_box(min: &mut GridPos, max: &mut GridPos, rot: impl Fn(i32, i32) -> [i32; 2]) {
    let a = rot(min[0], min[2]);
    let c = rot(max[0], max[2]);
    *min = [a[0].min(c[0]), min[1], a[1].min(c[1])];
    *max = [a[0].max(c[0]), max[1], a[1].max(c[1])];
}

/// Sets the extent of a brush along `axis` (0 = x, 1 = y, 2 = z) to `units`
/// half-metres (at least 1), keeping the minimum face fixed.
#[must_use]
pub fn resize_brush(brush: &Brush, axis: usize, units: i32) -> Brush {
    let units = units.max(1);
    let mut b = brush.clone();
    match &mut b {
        Brush::Box { min, max, .. }
        | Brush::Wedge { min, max, .. }
        | Brush::Cylinder { min, max, .. }
        | Brush::Stairs { min, max, .. } => max[axis] = min[axis] + units,
        Brush::Prism {
            points,
            y_min,
            y_max,
            ..
        } => {
            if axis == 1 {
                *y_max = *y_min + units;
            } else {
                let k = usize::from(axis == 2);
                let lo = points.iter().map(|p| p[k]).min().unwrap_or(0);
                let hi = points.iter().map(|p| p[k]).max().unwrap_or(0);
                let old = (hi - lo).max(1);
                for p in points.iter_mut() {
                    // Round to nearest, ties away from zero.
                    let num = (p[k] - lo) * units;
                    p[k] = lo + (2 * num + old * num.signum()) / (2 * old);
                }
            }
        }
    }
    b
}

/// Extent of a brush along `axis`, in half-metres.
#[must_use]
pub fn brush_extent(brush: &Brush, axis: usize) -> i32 {
    let (min, max) = brush_grid_bounds(brush);
    max[axis] - min[axis]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wedge() -> Brush {
        Brush::Wedge {
            min: [0, 0, 0],
            max: [6, 4, 2],
            rise: Dir::PosX,
            material: "metal".into(),
        }
    }

    #[test]
    fn four_quarter_turns_are_the_identity() {
        let brushes = [
            wedge(),
            Brush::Cylinder {
                min: [1, 0, 2],
                max: [5, 8, 4],
                axis: Axis::X,
                sides: 8,
                material: "metal".into(),
            },
            Brush::Prism {
                points: vec![[0, 6], [8, 6], [4, 0]],
                y_min: 0,
                y_max: 4,
                material: "metal".into(),
            },
        ];
        for b in brushes {
            let once = rotate_brush(&b, [3, 1], 1);
            assert_ne!(once, b);
            assert_eq!(rotate_brush(&b, [3, 1], 4), b);
            assert_eq!(rotate_brush(&once, [3, 1], 3), b);
            assert_eq!(rotate_brush(&b, [3, 1], -1), rotate_brush(&b, [3, 1], 3));
        }
    }

    #[test]
    fn quarter_turn_swaps_extents_and_turns_the_rise() {
        let r = rotate_brush(&wedge(), [0, 0], 1);
        let Brush::Wedge { min, max, rise, .. } = r else {
            panic!("wedge expected");
        };
        // (x, z) -> (z, -x): the x extent 0..6 becomes z -6..0.
        assert_eq!((min, max), ([0, 0, -6], [2, 4, 0]));
        assert_eq!(rise, Dir::NegZ);
    }

    #[test]
    fn resize_keeps_the_minimum_face() {
        let b = resize_brush(&wedge(), 0, 10);
        assert_eq!(brush_grid_bounds(&b), ([0, 0, 0], [10, 4, 2]));
        assert_eq!(brush_extent(&resize_brush(&wedge(), 1, -3), 1), 1);
        let prism = Brush::Prism {
            points: vec![[0, 6], [8, 6], [4, 0]],
            y_min: 0,
            y_max: 4,
            material: "m".into(),
        };
        let wide = resize_brush(&prism, 0, 16);
        assert_eq!(brush_grid_bounds(&wide), ([0, 0, 0], [16, 4, 6]));
    }

    #[test]
    fn props_are_picked_through_their_oriented_bounds() {
        let prop = Prop {
            recipe: "crate".into(),
            at: [10.0, 0.0, 0.0],
            yaw: 90.0,
            scale: 2.0,
        };
        // Unit cube scaled by 2: spans x 9..11 and y 0..2 whatever the yaw.
        let ray = Ray::new(Vec3::new(10.0, 1.0, 10.0), Vec3::NEG_Z);
        let t = ray_prop(&ray, &prop, &UnitPropBounds).unwrap();
        assert!((t - 9.0).abs() < 1e-4, "{t}");
        let miss = Ray::new(Vec3::new(12.0, 1.0, 10.0), Vec3::NEG_Z);
        assert_eq!(ray_prop(&miss, &prop, &UnitPropBounds), None);
    }
}

//! Move, rotate and scale handles on the selection, and their drags.

use faset_core::glam::Vec3;
use faset_mapfmt::GRID_UNIT_M;

use crate::element::{
    brush_extent, brush_grid_bounds, element_centre, resize_brush, rotate_brush, translate_brush,
    Element, PropBounds, Selection,
};
use crate::math::{axis_drag_param, ray_horizontal_plane, Ray};
use crate::snap::SnapSettings;

/// What a handle drag does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HandleMode {
    #[default]
    Move,
    Rotate,
    Scale,
}

/// An axis of a handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandleAxis {
    X,
    Y,
    Z,
}

impl HandleAxis {
    /// Index 0, 1 or 2.
    #[must_use]
    pub fn index(self) -> usize {
        match self {
            Self::X => 0,
            Self::Y => 1,
            Self::Z => 2,
        }
    }

    /// Unit direction.
    #[must_use]
    pub fn dir(self) -> Vec3 {
        match self {
            Self::X => Vec3::X,
            Self::Y => Vec3::Y,
            Self::Z => Vec3::Z,
        }
    }
}

/// On-screen handle length as a fraction of the camera distance.
pub const HANDLE_SCREEN_FRACTION: f32 = 0.18;
/// Pick tolerance as a fraction of the handle length.
const PICK_RADIUS_FRACTION: f32 = 0.08;

/// Handle length in metres for a camera `distance` away: constant size on
/// screen, but never below 1 m.
#[must_use]
pub fn handle_length(distance: f32) -> f32 {
    (distance * HANDLE_SCREEN_FRACTION).max(1.0)
}

/// Finds the handle under the ray: three axis arrows for move and scale, a
/// horizontal ring (axis Y) for rotate. `length` is the arrow length or ring
/// radius.
#[must_use]
pub fn pick_handle(ray: &Ray, origin: Vec3, length: f32, mode: HandleMode) -> Option<HandleAxis> {
    let radius = length * PICK_RADIUS_FRACTION;
    if mode == HandleMode::Rotate {
        let t = ray_horizontal_plane(ray, origin.y)?;
        let d = (ray.at(t) - origin).length();
        return ((d - length).abs() <= radius).then_some(HandleAxis::Y);
    }
    let mut best: Option<(f32, HandleAxis)> = None;
    for axis in [HandleAxis::X, HandleAxis::Y, HandleAxis::Z] {
        let d1 = axis.dir();
        // Closest points of the arrow line and of the mouse ray.
        let Some(s) = axis_drag_param(origin, d1, ray.origin, ray.dir) else {
            continue;
        };
        if !(0.0..=length).contains(&s) {
            continue;
        }
        let q = origin + d1 * s;
        let t = (q - ray.origin).dot(ray.dir);
        if t < 0.0 {
            continue;
        }
        let gap = (q - ray.at(t)).length();
        if gap <= radius && best.is_none_or(|(bt, _)| t < bt) {
            best = Some((t, axis));
        }
    }
    best.map(|(_, a)| a)
}

/// A drag in progress on a handle.
#[derive(Debug, Clone, PartialEq)]
pub struct Drag {
    pub target: Selection,
    pub mode: HandleMode,
    pub axis: HandleAxis,
    /// The element when the drag started.
    pub original: Element,
    /// The result of the last update (the original before any update).
    pub current: Element,
    origin: Vec3,
    length: f32,
    /// Start parameter along the axis (move, scale) or start angle in
    /// radians (rotate).
    start: f32,
}

impl Drag {
    /// Starts a drag of `axis` on `target`. `length` is the handle length at
    /// the time of the click.
    #[must_use]
    pub fn begin(
        ray: &Ray,
        target: Selection,
        original: Element,
        axis: HandleAxis,
        mode: HandleMode,
        length: f32,
        bounds: &dyn PropBounds,
    ) -> Option<Self> {
        let origin = element_centre(&original, bounds);
        let start = if mode == HandleMode::Rotate {
            angle_at(ray, origin)?
        } else {
            axis_drag_param(origin, axis.dir(), ray.origin, ray.dir)?
        };
        Some(Self {
            target,
            mode,
            axis,
            current: original.clone(),
            original,
            origin,
            length,
            start,
        })
    }

    /// Updates the drag with the new mouse ray and returns the element to
    /// show. When the ray gives no solution (parallel to the axis) the
    /// previous result is kept.
    pub fn update(&mut self, ray: &Ray, snap: &SnapSettings) -> &Element {
        let next = match self.mode {
            HandleMode::Move => axis_drag_param(self.origin, self.axis.dir(), ray.origin, ray.dir)
                .map(|s| self.moved(s - self.start, snap)),
            HandleMode::Scale => axis_drag_param(self.origin, self.axis.dir(), ray.origin, ray.dir)
                .map(|s| self.scaled(s - self.start, snap)),
            HandleMode::Rotate => {
                angle_at(ray, self.origin).map(|a| self.rotated(a - self.start, snap))
            }
        };
        if let Some(e) = next {
            self.current = e;
        }
        &self.current
    }

    fn moved(&self, delta: f32, snap: &SnapSettings) -> Element {
        let i = self.axis.index();
        match &self.original {
            Element::Brush(b) => {
                let (min, _) = brush_grid_bounds(b);
                let target = snap.to_grid(min[i] as f32 * GRID_UNIT_M + delta);
                let mut d = [0; 3];
                d[i] = target - min[i];
                Element::Brush(translate_brush(b, d))
            }
            Element::Prop(p) => {
                let mut p = p.clone();
                p.at[i] = snap.snap_coord(p.at[i] + delta);
                Element::Prop(p)
            }
        }
    }

    fn scaled(&self, delta: f32, snap: &SnapSettings) -> Element {
        match &self.original {
            Element::Brush(b) => {
                let i = self.axis.index();
                let old = brush_extent(b, i);
                let want = old as f32 * GRID_UNIT_M + delta;
                // The new extent lands on the grid, whatever the snap state.
                let units = if snap.enabled {
                    snap.to_grid(want)
                } else {
                    (want / GRID_UNIT_M).round() as i32
                };
                Element::Brush(resize_brush(b, i, units))
            }
            Element::Prop(p) => {
                let mut p = p.clone();
                let factor = 1.0 + delta / self.length.max(1e-3);
                p.scale = snap.snap_scale(p.scale * factor).clamp(0.1, 10.0);
                Element::Prop(p)
            }
        }
    }

    fn rotated(&self, delta_rad: f32, snap: &SnapSettings) -> Element {
        let deg = delta_rad.to_degrees();
        match &self.original {
            Element::Prop(p) => {
                let mut p = p.clone();
                // Decision: the snapped angle is the new yaw delta, applied
                // to the starting yaw.
                p.yaw += snap.snap_angle_deg(deg);
                Element::Prop(p)
            }
            Element::Brush(b) => {
                // Decision: brushes live on the integer grid, so they turn by
                // whole quarter turns around the centre of their footprint
                // (rounded to the grid).
                let turns = (deg / 90.0).round() as i32;
                let (min, max) = brush_grid_bounds(b);
                let pivot = [
                    (min[0] + max[0] + 1).div_euclid(2),
                    (min[2] + max[2] + 1).div_euclid(2),
                ];
                Element::Brush(rotate_brush(b, pivot, turns))
            }
        }
    }
}

/// Angle of the point under the ray on the horizontal plane through `origin`,
/// about the vertical axis, counter-clockwise seen from above: `x`
/// is angle 0 and `-z` is +90 degrees.
fn angle_at(ray: &Ray, origin: Vec3) -> Option<f32> {
    let t = ray_horizontal_plane(ray, origin.y)?;
    let p = ray.at(t) - origin;
    Some((-p.z).atan2(p.x))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::element::UnitPropBounds;
    use faset_mapfmt::{Brush, Prop};

    fn prop() -> Prop {
        Prop {
            recipe: "crate".into(),
            at: [0.0, 0.0, 0.0],
            yaw: 0.0,
            scale: 1.0,
        }
    }

    fn side_ray(x: f32, y: f32) -> Ray {
        // A ray from +Z looking at (x, y) in the XY plane.
        Ray::new(Vec3::new(x, y, 20.0), Vec3::NEG_Z)
    }

    #[test]
    fn move_drag_follows_the_mouse_along_the_axis() {
        let snap = SnapSettings::default();
        let el = Element::Prop(prop());
        let origin = element_centre(&el, &UnitPropBounds);
        assert_eq!(origin, Vec3::new(0.0, 0.5, 0.0));
        let mut drag = Drag::begin(
            &side_ray(origin.x + 1.0, origin.y + 0.5),
            Selection::Prop(0),
            el,
            HandleAxis::X,
            HandleMode::Move,
            2.0,
            &UnitPropBounds,
        )
        .unwrap();
        // Mouse moved 3.3 m along x: the position snaps to 3.5.
        let Element::Prop(p) = drag.update(&side_ray(4.3, 1.0), &snap).clone() else {
            panic!()
        };
        assert_eq!(p.at, [3.5, 0.0, 0.0]);
    }

    #[test]
    fn brush_move_lands_on_the_half_metre_grid() {
        let snap = SnapSettings::default();
        let b = Brush::Box {
            min: [0, 0, 0],
            max: [4, 4, 4],
            material: "concrete".into(),
        };
        let el = Element::Brush(b);
        let o = element_centre(&el, &UnitPropBounds);
        let mut drag = Drag::begin(
            &side_ray(o.x, o.y),
            Selection::Brush(0),
            el,
            HandleAxis::X,
            HandleMode::Move,
            2.0,
            &UnitPropBounds,
        )
        .unwrap();
        let Element::Brush(Brush::Box { min, max, .. }) =
            drag.update(&side_ray(o.x + 2.8, o.y), &snap).clone()
        else {
            panic!()
        };
        assert_eq!((min, max), ([6, 0, 0], [10, 4, 4]));
    }

    #[test]
    fn parallel_mouse_ray_keeps_the_previous_position() {
        let snap = SnapSettings::default();
        let el = Element::Prop(prop());
        let o = element_centre(&el, &UnitPropBounds);
        let mut drag = Drag::begin(
            &side_ray(o.x, o.y),
            Selection::Prop(0),
            el,
            HandleAxis::X,
            HandleMode::Move,
            2.0,
            &UnitPropBounds,
        )
        .unwrap();
        let moved = drag.update(&side_ray(o.x + 2.0, o.y), &snap).clone();
        // A ray along +X is parallel to the X axis: nothing changes.
        let parallel = Ray::new(Vec3::new(-30.0, 3.0, 4.0), Vec3::X);
        assert_eq!(drag.update(&parallel, &snap), &moved);
    }

    #[test]
    fn rotation_snaps_to_fifteen_degrees() {
        let snap = SnapSettings::default();
        let el = Element::Prop(prop());
        let o = element_centre(&el, &UnitPropBounds);
        let down = |x: f32, z: f32| Ray::new(Vec3::new(o.x + x, 30.0, o.z + z), Vec3::NEG_Y);
        let mut drag = Drag::begin(
            &down(3.0, 0.0),
            Selection::Prop(0),
            el,
            HandleAxis::Y,
            HandleMode::Rotate,
            3.0,
            &UnitPropBounds,
        )
        .unwrap();
        // The mouse moves 50 degrees counter-clockwise seen from above
        // (towards -z): snaps to 45.
        let a = 50f32.to_radians();
        let Element::Prop(p) = drag
            .update(&down(3.0 * a.cos(), -3.0 * a.sin()), &snap)
            .clone()
        else {
            panic!()
        };
        assert_eq!(p.yaw, 45.0);
    }

    #[test]
    fn brush_rotation_is_by_quarter_turns() {
        let snap = SnapSettings::default();
        let b = Brush::Box {
            min: [0, 0, 0],
            max: [8, 4, 2],
            material: "concrete".into(),
        };
        let el = Element::Brush(b);
        let o = element_centre(&el, &UnitPropBounds);
        let down = |x: f32, z: f32| Ray::new(Vec3::new(o.x + x, 30.0, o.z + z), Vec3::NEG_Y);
        let mut drag = Drag::begin(
            &down(3.0, 0.0),
            Selection::Brush(0),
            el,
            HandleAxis::Y,
            HandleMode::Rotate,
            3.0,
            &UnitPropBounds,
        )
        .unwrap();
        let Element::Brush(Brush::Box { min, max, .. }) =
            drag.update(&down(0.0, -3.0), &snap).clone()
        else {
            panic!()
        };
        // Quarter turn: the 8 x 2 footprint becomes 2 x 8 around (4, 1).
        assert_eq!(max[0] - min[0], 2);
        assert_eq!(max[2] - min[2], 8);
    }

    #[test]
    fn scale_resizes_a_brush_and_scales_a_prop() {
        let snap = SnapSettings::default();
        let b = Brush::Box {
            min: [0, 0, 0],
            max: [4, 4, 4],
            material: "concrete".into(),
        };
        let el = Element::Brush(b);
        let o = element_centre(&el, &UnitPropBounds);
        let mut drag = Drag::begin(
            &side_ray(o.x, o.y),
            Selection::Brush(0),
            el,
            HandleAxis::X,
            HandleMode::Scale,
            2.0,
            &UnitPropBounds,
        )
        .unwrap();
        let Element::Brush(Brush::Box { min, max, .. }) =
            drag.update(&side_ray(o.x + 2.0, o.y), &snap).clone()
        else {
            panic!()
        };
        assert_eq!((min, max), ([0, 0, 0], [8, 4, 4]));

        let el = Element::Prop(prop());
        let o = element_centre(&el, &UnitPropBounds);
        let mut drag = Drag::begin(
            &side_ray(o.x, o.y),
            Selection::Prop(0),
            el,
            HandleAxis::X,
            HandleMode::Scale,
            2.0,
            &UnitPropBounds,
        )
        .unwrap();
        let Element::Prop(p) = drag.update(&side_ray(o.x + 1.0, o.y), &snap).clone() else {
            panic!()
        };
        assert!((p.scale - 1.5).abs() < 1e-5, "{}", p.scale);
    }

    #[test]
    fn handles_are_picked_on_their_axis_only() {
        let origin = Vec3::new(0.0, 1.0, 0.0);
        // Looking at the X arrow from +Z.
        let hit = pick_handle(&side_ray(1.0, 1.0), origin, 2.0, HandleMode::Move);
        assert_eq!(hit, Some(HandleAxis::X));
        let hit = pick_handle(&side_ray(0.0, 2.0), origin, 2.0, HandleMode::Scale);
        assert_eq!(hit, Some(HandleAxis::Y));
        assert_eq!(
            pick_handle(&side_ray(1.0, 2.5), origin, 2.0, HandleMode::Move),
            None
        );
        // Beyond the arrow tip.
        assert_eq!(
            pick_handle(&side_ray(3.0, 1.0), origin, 2.0, HandleMode::Move),
            None
        );
        // The rotate ring of radius 2 around the origin.
        let down = Ray::new(Vec3::new(2.0, 30.0, 0.0), Vec3::NEG_Y);
        assert_eq!(
            pick_handle(&down, origin, 2.0, HandleMode::Rotate),
            Some(HandleAxis::Y)
        );
        let inside = Ray::new(Vec3::new(0.5, 30.0, 0.0), Vec3::NEG_Y);
        assert_eq!(pick_handle(&inside, origin, 2.0, HandleMode::Rotate), None);
    }

    #[test]
    fn handle_length_has_a_floor() {
        assert_eq!(handle_length(2.0), 1.0);
        assert!((handle_length(50.0) - 9.0).abs() < 1e-5);
    }
}

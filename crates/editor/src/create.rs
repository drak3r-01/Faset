//! Volume creation: drag a rectangle on the ground, then pull the height.

use faset_core::glam::Vec3;
use faset_mapfmt::{Axis, Brush, Dir, GRID_UNIT_M};

use crate::math::{axis_drag_param, ray_horizontal_plane, Ray};
use crate::snap::SnapSettings;

/// The brush kinds the editor can create.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BrushKind {
    Box,
    Wedge,
    Cylinder,
    Prism,
    Stairs,
}

impl BrushKind {
    /// Every kind, in toolbar order.
    pub const ALL: [Self; 5] = [
        Self::Box,
        Self::Wedge,
        Self::Cylinder,
        Self::Prism,
        Self::Stairs,
    ];

    /// Label for the toolbar.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Box => "Box",
            Self::Wedge => "Wedge",
            Self::Cylinder => "Cylinder",
            Self::Prism => "Prism",
            Self::Stairs => "Stairs",
        }
    }
}

/// Parameters of the volume being created.
#[derive(Debug, Clone, PartialEq)]
pub struct CreationSettings {
    pub kind: BrushKind,
    pub material: String,
    /// Rising side of wedges and stairs.
    pub rise: Dir,
    /// Facets of a cylinder (decision: 12).
    pub cylinder_sides: u8,
    /// Riser height of stairs in metres (decision: 0.25 m); the step count is
    /// the height divided by it.
    pub stairs_riser_m: f32,
    /// Height used when the height cannot be pulled (top view), metres.
    pub default_height_m: f32,
    /// Elevation of the ground plane the rectangle is drawn on, metres.
    pub ground_y: f32,
}

impl Default for CreationSettings {
    fn default() -> Self {
        Self {
            kind: BrushKind::Box,
            material: "concrete".to_owned(),
            rise: Dir::PosX,
            cylinder_sides: 12,
            stairs_riser_m: 0.25,
            default_height_m: 3.0,
            ground_y: 0.0,
        }
    }
}

/// Progress of a creation gesture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CreateStage {
    Idle,
    /// Dragging the rectangle; corners in grid units `(x, z)`.
    Footprint {
        start: [i32; 2],
        current: [i32; 2],
    },
    /// Rectangle fixed, pulling the height; `height` in grid units, negative
    /// below the ground plane.
    Height {
        min: [i32; 2],
        max: [i32; 2],
        corner: [i32; 2],
        height: i32,
    },
}

/// Builds the brush of `kind` over the footprint `min..max` (grid units) and
/// the heights `y0..y1` (grid units, `y0 < y1`).
#[must_use]
pub fn make_brush(
    settings: &CreationSettings,
    min: [i32; 2],
    max: [i32; 2],
    y0: i32,
    y1: i32,
) -> Brush {
    let (lo, hi) = ([min[0], y0, min[1]], [max[0], y1, max[1]]);
    let material = settings.material.clone();
    match settings.kind {
        BrushKind::Box => Brush::Box {
            min: lo,
            max: hi,
            material,
        },
        BrushKind::Wedge => Brush::Wedge {
            min: lo,
            max: hi,
            rise: settings.rise,
            material,
        },
        BrushKind::Cylinder => Brush::Cylinder {
            min: lo,
            max: hi,
            axis: Axis::Y,
            sides: settings.cylinder_sides.max(3),
            material,
        },
        // Decision: the prism footprint is the triangle on the rectangle's
        // far edge with its apex on the near edge, as in the example map.
        BrushKind::Prism => Brush::Prism {
            points: vec![
                [min[0], max[1]],
                [max[0], max[1]],
                [min[0] + (max[0] - min[0]) / 2, min[1]],
            ],
            y_min: y0,
            y_max: y1,
            material,
        },
        BrushKind::Stairs => {
            let height_m = (y1 - y0) as f32 * GRID_UNIT_M;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let steps = (height_m / settings.stairs_riser_m.max(0.05))
                .round()
                .clamp(1.0, 255.0) as u8;
            Brush::Stairs {
                min: lo,
                max: hi,
                rise: settings.rise,
                steps,
                material,
            }
        }
    }
}

/// The state machine of the creation gesture. The editor feeds it pointer
/// events; it returns the finished brush.
#[derive(Debug, Clone, PartialEq)]
pub struct Creation {
    pub settings: CreationSettings,
    pub stage: CreateStage,
}

impl Default for Creation {
    fn default() -> Self {
        Self {
            settings: CreationSettings::default(),
            stage: CreateStage::Idle,
        }
    }
}

impl Creation {
    /// Abandons the gesture.
    pub fn cancel(&mut self) {
        self.stage = CreateStage::Idle;
    }

    fn ground_point(&self, ray: &Ray, snap: &SnapSettings) -> Option<[i32; 2]> {
        let t = ray_horizontal_plane(ray, self.settings.ground_y)?;
        let p = ray.at(t);
        Some([snap.to_grid(p.x), snap.to_grid(p.z)])
    }

    /// Pointer pressed. In the idle stage starts the rectangle; while pulling
    /// the height, finishes the brush.
    pub fn press(&mut self, ray: &Ray, snap: &SnapSettings) -> Option<Brush> {
        match self.stage {
            CreateStage::Idle => {
                if let Some(p) = self.ground_point(ray, snap) {
                    self.stage = CreateStage::Footprint {
                        start: p,
                        current: p,
                    };
                }
                None
            }
            CreateStage::Footprint { .. } => None,
            CreateStage::Height { .. } => self.finish(),
        }
    }

    /// Pointer moved: grows the rectangle or pulls the height.
    pub fn drag(&mut self, ray: &Ray, snap: &SnapSettings) {
        match self.stage {
            CreateStage::Footprint { start, .. } => {
                if let Some(p) = self.ground_point(ray, snap) {
                    self.stage = CreateStage::Footprint { start, current: p };
                }
            }
            CreateStage::Height {
                min,
                max,
                corner,
                height,
            } => {
                let base = Vec3::new(
                    corner[0] as f32 * GRID_UNIT_M,
                    self.settings.ground_y,
                    corner[1] as f32 * GRID_UNIT_M,
                );
                // Keep the previous height when the mouse ray is parallel to
                // the vertical axis.
                let height = axis_drag_param(base, Vec3::Y, ray.origin, ray.dir)
                    .map_or(height, |s| snap.to_grid(s));
                self.stage = CreateStage::Height {
                    min,
                    max,
                    corner,
                    height,
                };
            }
            CreateStage::Idle => {}
        }
    }

    /// Pointer released. Ends the rectangle: with `top_view` the brush is
    /// finished with the default height (the height cannot be pulled from
    /// above), otherwise the height stage starts. A degenerate rectangle is
    /// abandoned.
    pub fn release(&mut self, top_view: bool) -> Option<Brush> {
        let CreateStage::Footprint { start, current } = self.stage else {
            return None;
        };
        let min = [start[0].min(current[0]), start[1].min(current[1])];
        let max = [start[0].max(current[0]), start[1].max(current[1])];
        if max[0] == min[0] || max[1] == min[1] {
            self.stage = CreateStage::Idle;
            return None;
        }
        if top_view {
            self.stage = CreateStage::Height {
                min,
                max,
                corner: current,
                height: self.default_height_units(),
            };
            return self.finish();
        }
        self.stage = CreateStage::Height {
            min,
            max,
            corner: current,
            height: 0,
        };
        None
    }

    #[allow(clippy::cast_possible_truncation)]
    fn default_height_units(&self) -> i32 {
        ((self.settings.default_height_m / GRID_UNIT_M).round() as i32).max(1)
    }

    fn finish(&mut self) -> Option<Brush> {
        let brush = self.preview();
        self.stage = CreateStage::Idle;
        brush
    }

    /// The brush the gesture would create now, for drawing the ghost.
    #[must_use]
    #[allow(clippy::cast_possible_truncation)]
    pub fn preview(&self) -> Option<Brush> {
        let ground = (self.settings.ground_y / GRID_UNIT_M).round() as i32;
        match self.stage {
            CreateStage::Idle => None,
            CreateStage::Footprint { start, current } => {
                let min = [start[0].min(current[0]), start[1].min(current[1])];
                let max = [start[0].max(current[0]), start[1].max(current[1])];
                (max[0] > min[0] && max[1] > min[1])
                    .then(|| make_brush(&self.settings, min, max, ground, ground + 1))
            }
            CreateStage::Height {
                min, max, height, ..
            } => {
                // A height of zero would be degenerate: at least one unit.
                let (y0, y1) = if height >= 0 {
                    (ground, ground + height.max(1))
                } else {
                    (ground + height, ground)
                };
                Some(make_brush(&self.settings, min, max, y0, y1))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn down_ray(x: f32, z: f32) -> Ray {
        Ray::new(Vec3::new(x, 50.0, z), Vec3::NEG_Y)
    }

    #[test]
    fn rectangle_then_height_makes_a_box() {
        let snap = SnapSettings::default();
        let mut c = Creation::default();
        assert!(c.press(&down_ray(1.1, 2.1), &snap).is_none());
        c.drag(&down_ray(4.9, 6.2), &snap);
        assert!(c.release(false).is_none());
        // Pull the height with a ray from the side: at height 3 m.
        c.drag(&Ray::new(Vec3::new(2.5, 3.2, 30.0), Vec3::NEG_Z), &snap);
        let brush = c.press(&down_ray(0.0, 0.0), &snap).expect("brush");
        assert_eq!(
            brush,
            Brush::Box {
                min: [2, 0, 4],
                max: [10, 6, 12],
                material: "concrete".into()
            }
        );
        assert_eq!(c.stage, CreateStage::Idle);
    }

    #[test]
    fn degenerate_rectangle_is_abandoned() {
        let snap = SnapSettings::default();
        let mut c = Creation::default();
        c.press(&down_ray(1.0, 1.0), &snap);
        c.drag(&down_ray(1.1, 5.0), &snap);
        assert!(c.release(false).is_none());
        assert_eq!(c.stage, CreateStage::Idle);
    }

    #[test]
    fn top_view_uses_the_default_height() {
        let snap = SnapSettings::default();
        let mut c = Creation::default();
        c.press(&down_ray(0.0, 0.0), &snap);
        c.drag(&down_ray(4.0, 4.0), &snap);
        let b = c.release(true).expect("brush");
        assert_eq!(
            b,
            Brush::Box {
                min: [0, 0, 0],
                max: [8, 6, 8],
                material: "concrete".into()
            }
        );
    }

    #[test]
    fn parallel_height_ray_keeps_the_previous_height() {
        let snap = SnapSettings::default();
        let mut c = Creation::default();
        c.press(&down_ray(0.0, 0.0), &snap);
        c.drag(&down_ray(4.0, 4.0), &snap);
        c.release(false);
        c.drag(&Ray::new(Vec3::new(2.0, 2.0, 30.0), Vec3::NEG_Z), &snap);
        let before = c.stage;
        // A vertical ray is parallel to the height axis.
        c.drag(&down_ray(3.0, 3.0), &snap);
        assert_eq!(c.stage, before);
    }

    #[test]
    fn every_kind_makes_a_valid_brush() {
        for kind in BrushKind::ALL {
            let settings = CreationSettings {
                kind,
                ..CreationSettings::default()
            };
            let b = make_brush(&settings, [0, 0], [8, 6], 0, 6);
            let mut map = crate::new_map("t");
            map.brushes.push(b);
            let text = faset_mapfmt::to_ron(&map);
            assert!(faset_mapfmt::parse(text.as_bytes()).is_ok(), "{kind:?}");
        }
        let stairs = make_brush(
            &CreationSettings {
                kind: BrushKind::Stairs,
                ..CreationSettings::default()
            },
            [0, 0],
            [8, 6],
            0,
            8,
        );
        // 4 m of height at 0.25 m risers.
        assert!(matches!(stairs, Brush::Stairs { steps: 16, .. }));
    }

    #[test]
    fn negative_height_extrudes_below_the_ground() {
        let c = Creation {
            stage: CreateStage::Height {
                min: [0, 0],
                max: [4, 4],
                corner: [4, 4],
                height: -3,
            },
            ..Creation::default()
        };
        assert_eq!(
            c.preview().unwrap(),
            Brush::Box {
                min: [0, -3, 0],
                max: [4, 0, 4],
                material: "concrete".into()
            }
        );
    }
}

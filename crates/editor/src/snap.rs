//! Snapping of positions, angles and scales.

use faset_core::glam::Vec3;
use faset_mapfmt::{GridPos, GRID_UNIT_M};

/// Snap settings of the editor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SnapSettings {
    /// When off, props and spawns move freely; brushes still land on the
    /// 0.5 m grid because the format stores integer half-metres.
    pub enabled: bool,
    /// Grid step in half-metre units (1 = 0.5 m, 2 = 1 m, ...), at least 1.
    pub grid_units: u32,
    /// Angle step in degrees.
    pub angle_deg: f32,
    /// Prop scale step.
    pub scale_step: f32,
}

impl Default for SnapSettings {
    /// On, 0.5 m grid, 15 degree angles, 0.05 scale step.
    fn default() -> Self {
        Self {
            enabled: true,
            grid_units: 1,
            angle_deg: 15.0,
            scale_step: 0.05,
        }
    }
}

impl SnapSettings {
    /// Grid step in metres.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn grid_m(&self) -> f32 {
        self.grid_units.max(1) as f32 * GRID_UNIT_M
    }

    /// A coordinate in metres snapped to the grid (unchanged when off).
    #[must_use]
    pub fn snap_coord(&self, v: f32) -> f32 {
        if self.enabled {
            let g = self.grid_m();
            (v / g).round() * g
        } else {
            v
        }
    }

    /// A position snapped to the grid on every axis.
    #[must_use]
    pub fn snap_position(&self, p: Vec3) -> Vec3 {
        Vec3::new(
            self.snap_coord(p.x),
            self.snap_coord(p.y),
            self.snap_coord(p.z),
        )
    }

    /// An angle in degrees snapped to the angle step (unchanged when off).
    #[must_use]
    pub fn snap_angle_deg(&self, a: f32) -> f32 {
        if self.enabled && self.angle_deg > 0.0 {
            (a / self.angle_deg).round() * self.angle_deg
        } else {
            a
        }
    }

    /// A scale snapped to the scale step (unchanged when off).
    #[must_use]
    pub fn snap_scale(&self, s: f32) -> f32 {
        if self.enabled && self.scale_step > 0.0 {
            (s / self.scale_step).round() * self.scale_step
        } else {
            s
        }
    }

    /// A coordinate in metres as integer half-metres, on the snap grid when
    /// snapping is on and on the half-metre grid otherwise.
    #[must_use]
    #[allow(clippy::cast_possible_truncation)]
    pub fn to_grid(&self, v: f32) -> i32 {
        if self.enabled {
            (v / self.grid_m()).round() as i32 * self.grid_units.max(1) as i32
        } else {
            (v / GRID_UNIT_M).round() as i32
        }
    }

    /// A position as an integer half-metre grid position.
    #[must_use]
    pub fn to_grid_pos(&self, p: Vec3) -> GridPos {
        [self.to_grid(p.x), self.to_grid(p.y), self.to_grid(p.z)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_snap_to_half_metres() {
        let s = SnapSettings::default();
        assert_eq!(s.snap_coord(0.2), 0.0);
        assert_eq!(s.snap_coord(0.3), 0.5);
        assert_eq!(s.snap_coord(-0.76), -1.0);
        assert_eq!(s.snap_coord(7.74), 7.5);
        let p = s.snap_position(Vec3::new(1.26, -3.1, 9.9));
        assert_eq!(p, Vec3::new(1.5, -3.0, 10.0));
        assert_eq!(s.to_grid_pos(Vec3::new(1.26, -3.1, 9.9)), [3, -6, 20]);
    }

    #[test]
    fn angles_snap_to_fifteen_degrees() {
        let s = SnapSettings::default();
        assert_eq!(s.snap_angle_deg(7.0), 0.0);
        assert_eq!(s.snap_angle_deg(8.0), 15.0);
        assert_eq!(s.snap_angle_deg(-22.0), -15.0);
        assert_eq!(s.snap_angle_deg(-23.0), -30.0);
        assert_eq!(s.snap_angle_deg(91.0), 90.0);
    }

    #[test]
    fn coarser_grid_and_disabled_snapping() {
        let mut s = SnapSettings {
            grid_units: 4,
            ..SnapSettings::default()
        };
        assert_eq!(s.snap_coord(1.2), 2.0);
        assert_eq!(s.to_grid(2.9), 4);
        s.enabled = false;
        assert_eq!(s.snap_coord(0.3), 0.3);
        assert_eq!(s.snap_angle_deg(7.0), 7.0);
        // Brushes still land on the half-metre grid.
        assert_eq!(s.to_grid(0.3), 1);
    }
}

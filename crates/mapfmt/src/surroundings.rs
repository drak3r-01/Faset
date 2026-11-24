//! Surroundings: the landscape beyond the playable area (lot 6.12).
//!
//! A map ends at the edge of its playable rectangle; the optional
//! [`Surroundings`] block says what the world looks like past that edge so
//! the renderer can draw far terrain, a horizon silhouette and distance haze.
//! It is scenery only: the simulation's limits (walls, kill volumes) are the
//! brushes and zones of the map and are not changed by this block.
//!
//! Each of the four sides of the rectangle has a [`Border`] (what the ground
//! does past the edge: a forest, a cliff, the sea, a steep slope...), and the
//! whole map has a [`Horizon`] (the far silhouette). Sides are named by the
//! direction they face: north is -Z, east is +X, south is +Z, west is -X.
//!
//! Decisions of lot 6.12:
//! - the rectangle is stored in metres as `(min_x, min_z, max_x, max_z)`;
//!   `ground` is the height of the playable floor at its edge, `sea_level`
//!   the water height of coastal borders (used for the sea when the map has
//!   no water zone);
//! - far terrain is generated at load time from this data and `seed` (it is
//!   not stored as a height field), so the block stays a few dozen bytes;
//! - `haze` scales the fog density of the map's ambience profile: above 1
//!   the far landscape fades sooner, below 1 it stays crisp.

use serde::{Deserialize, Serialize};

/// What the ground does past one side of the playable rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Border {
    /// Rolling wooded hills, trees right up to the edge.
    Forest,
    /// A cliff face down to a beach (when the map has a sea) or a low plain.
    Cliff,
    /// Open sea: the ground drops away under the water.
    Sea,
    /// A steep slope climbing to mountains.
    Slope,
    /// Sand dunes.
    Dunes,
    /// Broken rock ridges.
    Rocks,
}

impl Border {
    /// Every border, in declaration order.
    pub const ALL: [Self; 6] = [
        Self::Forest,
        Self::Cliff,
        Self::Sea,
        Self::Slope,
        Self::Dunes,
        Self::Rocks,
    ];
}

/// The far silhouette, drawn beyond the borders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum Horizon {
    /// Nothing: the terrain stays at the level of its border.
    #[default]
    Flat,
    /// Tall distant mountains with caps of snow.
    Mountains,
    /// A low ridge of distant forest.
    Treeline,
    /// Distant dunes.
    Dunes,
}

/// The landscape block of a map. Serialised as `surroundings: (...)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Surroundings {
    /// Playable rectangle in metres: `(min_x, min_z, max_x, max_z)`.
    pub area: [f32; 4],
    /// Height of the playable floor at the edge of the rectangle, metres.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub ground: f32,
    /// Sea height in metres, for the coastal borders; `None` for no sea.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sea_level: Option<f32>,
    /// Side facing -Z.
    pub north: Border,
    /// Side facing +X.
    pub east: Border,
    /// Side facing +Z.
    pub south: Border,
    /// Side facing -X.
    pub west: Border,
    /// Far silhouette.
    #[serde(default, skip_serializing_if = "is_flat")]
    pub horizon: Horizon,
    /// Multiplier of the ambience's fog density, `0..=MAX_HAZE`; 1 keeps it.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub haze: f32,
    /// Seed of the generated hills and plants.
    #[serde(default, skip_serializing_if = "is_zero_seed")]
    pub seed: u32,
}

/// Largest `haze`.
pub const MAX_HAZE: f32 = 8.0;

fn one() -> f32 {
    1.0
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_one(v: &f32) -> bool {
    (*v - 1.0).abs() < f32::EPSILON
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero(v: &f32) -> bool {
    *v == 0.0
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero_seed(v: &u32) -> bool {
    *v == 0
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_flat(h: &Horizon) -> bool {
    *h == Horizon::Flat
}

impl Surroundings {
    /// The four borders in the order north, east, south, west.
    #[must_use]
    pub fn borders(&self) -> [Border; 4] {
        [self.north, self.east, self.south, self.west]
    }

    /// Whether `(x, z)` (metres) lies inside the playable rectangle.
    #[must_use]
    pub fn contains(&self, x: f32, z: f32) -> bool {
        let [x0, z0, x1, z1] = self.area;
        (x0..=x1).contains(&x) && (z0..=z1).contains(&z)
    }

    /// Checks the numbers.
    ///
    /// # Errors
    /// A description of the first problem found.
    pub fn validate(&self) -> Result<(), String> {
        let [x0, z0, x1, z1] = self.area;
        if !self.area.iter().all(|v| v.is_finite())
            || !self.ground.is_finite()
            || !self.haze.is_finite()
        {
            return Err("surroundings: a number is NaN or infinite".to_owned());
        }
        if self.sea_level.is_some_and(|s| !s.is_finite()) {
            return Err("surroundings: sea_level is NaN or infinite".to_owned());
        }
        if x1 - x0 < 8.0 || z1 - z0 < 8.0 {
            return Err(format!(
                "surroundings: area {:?} is smaller than 8 m on an axis",
                self.area
            ));
        }
        if !(0.0..=MAX_HAZE).contains(&self.haze) {
            return Err(format!(
                "surroundings: haze {} outside 0..={MAX_HAZE}",
                self.haze
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block() -> Surroundings {
        Surroundings {
            area: [-10.0, -8.0, 10.0, 8.0],
            ground: 0.0,
            sea_level: None,
            north: Border::Forest,
            east: Border::Cliff,
            south: Border::Sea,
            west: Border::Slope,
            horizon: Horizon::Mountains,
            haze: 1.0,
            seed: 0,
        }
    }

    #[test]
    fn a_good_block_validates_and_contains_its_area() {
        let s = block();
        assert!(s.validate().is_ok());
        assert!(s.contains(0.0, 0.0) && s.contains(10.0, -8.0));
        assert!(!s.contains(10.1, 0.0) && !s.contains(0.0, -8.1));
        assert_eq!(s.borders()[1], Border::Cliff);
    }

    #[test]
    fn bad_numbers_are_rejected() {
        let mut s = block();
        s.area = [0.0, 0.0, 4.0, 40.0];
        assert!(s.validate().is_err(), "too narrow");
        let mut s = block();
        s.haze = 9.0;
        assert!(s.validate().is_err());
        let mut s = block();
        s.ground = f32::NAN;
        assert!(s.validate().is_err());
        let mut s = block();
        s.sea_level = Some(f32::INFINITY);
        assert!(s.validate().is_err());
    }
}

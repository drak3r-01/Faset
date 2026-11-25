//! The map data model, serialised to and from RON with serde.
//!
//! Field order in these structs is the canonical field order of the file.
//! Optional blocks and fields are omitted from the file when empty or at
//! their default value, so a parsed file re-serialises to the same text.

use faset_core::glam::Vec3;
use serde::{Deserialize, Serialize};

use crate::surroundings::Surroundings;
use crate::terrain::Terrain;
use crate::vegetation::Vegetation;
use crate::world::World;

/// Size of one brush grid unit in metres (brush coordinates are integers).
pub const GRID_UNIT_M: f32 = 0.5;

/// Integer grid position in half-metres (`x`, `y`, `z`).
pub type GridPos = [i32; 3];

/// World position in metres (`x`, `y`, `z`).
pub type Pos = [f32; 3];

/// One map file. Serialised as `Map(...)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename = "Map", deny_unknown_fields)]
pub struct MapFile {
    /// Version of the file format; see [`crate::FORMAT_VERSION`].
    pub format_version: u32,
    pub meta: Meta,
    pub theme: Theme,
    /// Optional local physics overrides.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "implicit")]
    pub physics: Option<Physics>,
    #[serde(default)]
    pub symmetry: Symmetry,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prefabs: Vec<Prefab>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub brushes: Vec<Brush>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub props: Vec<Prop>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spawns: Vec<Spawn>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pickups: Vec<Pickup>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub zones: Vec<Zone>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lights: Vec<Light>,
    /// Optional height field (format version 2); see [`crate::terrain`].
    #[serde(default, skip_serializing_if = "Option::is_none", with = "implicit")]
    pub terrain: Option<Terrain>,
    /// Optional density rules and hand-placed plants (format version 3); see
    /// [`crate::vegetation`].
    #[serde(default, skip_serializing_if = "Option::is_none", with = "implicit")]
    pub vegetation: Option<Vegetation>,
    /// Optional landscape beyond the playable area (format version 4); see
    /// [`crate::surroundings`].
    #[serde(default, skip_serializing_if = "Option::is_none", with = "implicit")]
    pub surroundings: Option<Surroundings>,
    /// Optional large world around the playable zone (format version 5); see
    /// [`crate::world`].
    #[serde(default, skip_serializing_if = "Option::is_none", with = "implicit")]
    pub world: Option<World>,
}

/// Descriptive data shown in menus and used by the rotation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Meta {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub author: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// Labels of the compatible game modes, e.g. `"fps"`.
    pub modes: Vec<String>,
    /// Recommended player count, `(min, max)`.
    pub players: (u8, u8),
}

/// Look of the map: names resolved by the renderer and asset crates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Theme {
    /// Mood (ambience) profile name.
    pub ambience: String,
    /// Sky preset name; empty means the ambience default.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sky: String,
    /// Weather calendar, ordered by start time.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub weather: Vec<WeatherSlot>,
    /// Colour palette name.
    pub palette: String,
}

/// One entry of the weather calendar: `weather` starts at `from_s` seconds
/// after the start of the match and lasts until the next entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WeatherSlot {
    pub from_s: f32,
    pub weather: String,
}

/// Local physics overrides; an absent field keeps the game default.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Physics {
    /// Gravity magnitude in m/s^2 (positive, pulls toward -Y).
    #[serde(default, skip_serializing_if = "Option::is_none", with = "implicit")]
    pub gravity: Option<f32>,
    /// Multiplier applied to movement speeds.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "implicit")]
    pub speed_scale: Option<f32>,
}

/// Declared symmetry: the file describes one half, loading adds the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum Symmetry {
    #[default]
    None,
    /// Mirror across the plane x = 0.
    MirrorX,
    /// Mirror across the plane z = 0.
    MirrorZ,
    /// Half turn about the vertical axis through the origin.
    Rotate180,
}

/// Horizontal direction, used for the rising side of wedges and stairs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Dir {
    PosX,
    NegX,
    PosZ,
    NegZ,
}

/// Axis of a cylinder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Axis {
    X,
    Y,
    Z,
}

/// A simple solid volume. Coordinates are integers in half-metres; `min` is
/// strictly below `max` on every axis.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Brush {
    /// Axis-aligned box.
    Box {
        min: GridPos,
        max: GridPos,
        material: String,
    },
    /// Ramp filling its box: full height on the `rise` side, zero on the
    /// opposite side.
    Wedge {
        min: GridPos,
        max: GridPos,
        rise: Dir,
        material: String,
    },
    /// Cylinder inscribed in its box, along `axis`, with `sides` facets.
    Cylinder {
        min: GridPos,
        max: GridPos,
        axis: Axis,
        sides: u8,
        material: String,
    },
    /// Vertical extrusion of a convex polygon footprint `(x, z)` given
    /// counter-clockwise seen from above, between heights `y_min` and `y_max`.
    Prism {
        points: Vec<[i32; 2]>,
        y_min: i32,
        y_max: i32,
        material: String,
    },
    /// Staircase filling its box with `steps` steps, climbing toward `rise`.
    Stairs {
        min: GridPos,
        max: GridPos,
        rise: Dir,
        steps: u8,
        material: String,
    },
}

/// A prop recipe placed in the map.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Prop {
    /// Recipe id in the asset library.
    pub recipe: String,
    pub at: Pos,
    /// Rotation about +Y in degrees (counter-clockwise seen from above).
    pub yaw: f32,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub scale: f32,
}

/// Team a spawn point belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Team {
    A,
    B,
    /// Usable by every player (free-for-all modes).
    Any,
}

impl Team {
    /// The opposing team; `Any` stays `Any`.
    #[must_use]
    pub fn other(self) -> Self {
        match self {
            Self::A => Self::B,
            Self::B => Self::A,
            Self::Any => Self::Any,
        }
    }
}

/// A player spawn point.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spawn {
    pub at: Pos,
    /// Facing about +Y in degrees; 0 faces -Z.
    pub yaw: f32,
    pub team: Team,
}

/// What a pickup gives.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PickupKind {
    Health,
    Armor,
    Ammo,
    /// A weapon, by weapon id.
    Weapon(String),
}

/// A pickup location.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pickup {
    pub kind: PickupKind,
    pub at: Pos,
    /// Respawn delay in seconds.
    pub respawn_s: u32,
}

/// A gameplay volume.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Zone {
    /// Axis-aligned box zone, integer half-metre grid like brushes.
    Box {
        min: GridPos,
        max: GridPos,
        kind: ZoneKind,
    },
}

/// Effect of a zone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum ZoneKind {
    /// Kills whoever enters.
    KillVolume,
    /// Swimmable water.
    Water,
    /// Sets the velocity (m/s) of whoever enters.
    JumpPad { velocity: Pos },
    /// Moves whoever enters to `target`, facing `yaw` degrees.
    Teleporter { target: Pos, yaw: f32 },
    /// Multiplies gravity by `scale` inside the zone.
    LowGravity { scale: f32 },
    /// Mode objective, identified by `id`.
    Objective { id: String },
}

/// A point light.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Light {
    pub at: Pos,
    /// Linear RGB colour, 0..1.
    pub color: [f32; 3],
    /// Relative intensity multiplier.
    pub intensity: f32,
    /// Range in metres.
    pub range: f32,
}

/// A reusable group of brushes and props, placed several times.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Prefab {
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub brushes: Vec<Brush>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub props: Vec<Prop>,
    /// Where the group is instantiated.
    pub place: Vec<Placement>,
}

/// One instance of a prefab: quarter turns about +Y (counter-clockwise seen
/// from above) applied around the prefab origin, then a grid offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Placement {
    /// Offset in half-metres.
    pub at: GridPos,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub turns: u8,
}

/// Converts a half-metre grid position to metres.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn grid_to_metres(p: GridPos) -> Vec3 {
    Vec3::new(p[0] as f32, p[1] as f32, p[2] as f32) * GRID_UNIT_M
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_one(v: &f32) -> bool {
    *v == 1.0
}

fn one() -> f32 {
    1.0
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero(v: &u8) -> bool {
    *v == 0
}

/// Serde adapter writing `Option<T>` fields as the bare value. Only used with
/// `skip_serializing_if = "Option::is_none"` and `default`, so `None` is
/// never written and an absent field reads back as `None`.
mod implicit {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[allow(clippy::ref_option)]
    pub fn serialize<T: Serialize, S: Serializer>(v: &Option<T>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(v) => v.serialize(s),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, T: Deserialize<'de>, D: Deserializer<'de>>(
        d: D,
    ) -> Result<Option<T>, D::Error> {
        T::deserialize(d).map(Some)
    }
}

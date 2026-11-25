//! Versioned RON map format.
//!
//! A map is one RON file made of simple volumes on a 0.5 m grid (brush
//! coordinates are integers in half-metres), plus props, spawns, pickups,
//! zones and lights in metres. Angles in the file are degrees about +Y,
//! counter-clockwise seen from above, 0 facing -Z. Right-handed, Y up.
//!
//! [`parse`] reads and migrates a file, [`to_ron`] writes its canonical form
//! and [`fingerprint`] hashes that form: the map id used on the network.
//! Symmetry and prefabs are declared in the file and expanded on load with
//! [`expand_symmetry`] and [`expand_prefabs`].
//!
//! Format version 2 (lot 6.10) adds the optional [`Terrain`] height field;
//! version 1 files are migrated by bumping their version (they have no
//! terrain).
//!
//! Format version 3 (lot 6.11) adds the optional [`Vegetation`] block
//! (density rules per ground material and hand-placed plants, scattered by
//! [`vegetation::scatter`]); version 2 files are migrated by bumping their
//! version (they have no vegetation).
//!
//! Format version 4 (lot 6.12) adds the optional [`Surroundings`] block (the
//! landscape beyond the playable area); version 3 files are migrated by
//! bumping their version (they have no surroundings).
//!
//! Format version 5 (lot 8.1) adds the optional [`World`] block (the world
//! extent, the playable rectangle and a coarse far height field around the
//! playable [`Terrain`]); version 4 files are migrated by bumping their
//! version (they have no world).
#![forbid(unsafe_code)]

mod expand;
pub mod geometry;
mod migrate;
mod schema;
pub mod surroundings;
pub mod terrain;
pub mod vegetation;
pub mod world;

pub use expand::{expand_prefabs, expand_symmetry};
pub use schema::{
    grid_to_metres, Axis, Brush, Dir, GridPos, Light, MapFile, Meta, Physics, Pickup, PickupKind,
    Placement, Pos, Prefab, Prop, Spawn, Symmetry, Team, Theme, WeatherSlot, Zone, ZoneKind,
    GRID_UNIT_M,
};
pub use surroundings::{Border, Horizon, Surroundings};
pub use terrain::Terrain;
pub use vegetation::{DensityRule, Plant, Species, Vegetation};
pub use world::{FarField, World};

/// Expands prefabs then symmetry: the map as built and played.
#[must_use]
pub fn expand(map: &MapFile) -> MapFile {
    expand_symmetry(&expand_prefabs(map))
}

/// Current version of the map format; [`parse`] migrates older files to it.
pub const FORMAT_VERSION: u32 = 5;

/// Largest accepted map file, in bytes (256 KiB).
pub const MAX_MAP_BYTES: usize = 262_144;

/// Why a map file was rejected.
#[derive(Debug, thiserror::Error)]
pub enum MapError {
    /// Not valid RON, or not matching the schema (including unknown fields).
    #[error("invalid map RON: {0}")]
    Parse(#[from] ron::error::SpannedError),
    /// `format_version` is 0 or newer than [`FORMAT_VERSION`].
    #[error("unknown map format version {found} (this build reads 1..={FORMAT_VERSION})")]
    UnknownVersion { found: u32 },
    /// The file is larger than [`MAX_MAP_BYTES`].
    #[error("map file is {size} bytes, limit is {MAX_MAP_BYTES}")]
    TooLarge { size: usize },
    /// Well-formed but structurally wrong (bad encoding, degenerate brush...).
    #[error("invalid map: {0}")]
    Invalid(String),
}

/// Parses a map file, migrating it to [`FORMAT_VERSION`].
///
/// # Errors
/// [`MapError::TooLarge`] above [`MAX_MAP_BYTES`], [`MapError::UnknownVersion`]
/// for a version this build cannot read, [`MapError::Parse`] for malformed RON
/// or unknown fields, [`MapError::Invalid`] for non-UTF-8 text or a
/// structurally wrong map.
pub fn parse(bytes: &[u8]) -> Result<MapFile, MapError> {
    if bytes.len() > MAX_MAP_BYTES {
        return Err(MapError::TooLarge { size: bytes.len() });
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|e| MapError::Invalid(format!("map file is not UTF-8: {e}")))?;
    let map = migrate::migrate(text)?;
    check(&map)?;
    Ok(map)
}

/// Writes the canonical form of `map`: fixed field order, empty blocks and
/// default values omitted, 4-space indentation, LF newlines, no comments,
/// trailing newline. Deterministic for a given `map`.
#[must_use]
pub fn to_ron(map: &MapFile) -> String {
    let config = ron::ser::PrettyConfig::new()
        .new_line("\n".to_owned())
        .indentor("    ".to_owned())
        .depth_limit(2)
        .struct_names(false)
        .separate_tuple_members(false)
        .enumerate_arrays(false);
    // Serialising plain data (no maps with non-string keys, no unsupported
    // types) cannot fail.
    let body = ron::ser::to_string_pretty(map, config).expect("map serialises to RON");
    // Only the top level carries its struct name, like the plan's example.
    format!("Map{body}\n")
}

/// Map id: BLAKE3 hash of the canonical form ([`to_ron`]).
#[must_use]
pub fn fingerprint(map: &MapFile) -> [u8; 32] {
    *blake3::hash(to_ron(map).as_bytes()).as_bytes()
}

/// Structural checks the schema cannot express. Gameplay and layout rules
/// belong to the map validator (lot 3.3).
fn check(map: &MapFile) -> Result<(), MapError> {
    let bad = |what: String| Err(MapError::Invalid(what));
    let (lo, hi) = map.meta.players;
    if lo == 0 || lo > hi {
        return bad(format!("player count ({lo}, {hi}) is not a valid range"));
    }
    let boxed = |what: &str, min: &GridPos, max: &GridPos| -> Result<(), MapError> {
        if (0..3).all(|i| min[i] < max[i]) {
            Ok(())
        } else {
            Err(MapError::Invalid(format!(
                "{what} min {min:?} is not below max {max:?} on every axis"
            )))
        }
    };
    let brushes = map
        .brushes
        .iter()
        .chain(map.prefabs.iter().flat_map(|p| &p.brushes));
    for b in brushes {
        match b {
            Brush::Box { min, max, .. } | Brush::Wedge { min, max, .. } => {
                boxed("brush", min, max)?
            }
            Brush::Cylinder {
                min, max, sides, ..
            } => {
                boxed("cylinder", min, max)?;
                if *sides < 3 {
                    return bad(format!("cylinder has {sides} sides, at least 3 needed"));
                }
            }
            Brush::Prism {
                points,
                y_min,
                y_max,
                ..
            } => {
                if points.len() < 3 || y_min >= y_max {
                    return bad("prism needs 3+ points and y_min < y_max".to_owned());
                }
            }
            Brush::Stairs {
                min, max, steps, ..
            } => {
                boxed("stairs", min, max)?;
                if *steps == 0 {
                    return bad("stairs need at least one step".to_owned());
                }
            }
        }
    }
    for z in &map.zones {
        let Zone::Box { min, max, .. } = z;
        boxed("zone", min, max)?;
    }
    let mut names = std::collections::HashSet::new();
    for p in &map.prefabs {
        if !names.insert(p.name.as_str()) {
            return bad(format!("prefab `{}` is defined twice", p.name));
        }
    }
    if !finite(map) {
        return bad("a number is NaN or infinite".to_owned());
    }
    if let Some(t) = &map.terrain {
        t.validate().map_err(MapError::Invalid)?;
    }
    if let Some(s) = &map.surroundings {
        s.validate().map_err(MapError::Invalid)?;
    }
    if let Some(v) = &map.vegetation {
        v.validate().map_err(MapError::Invalid)?;
    }
    if let Some(w) = &map.world {
        w.validate(map.terrain.as_ref())
            .map_err(MapError::Invalid)?;
        if let Some(s) = &map.surroundings {
            if w.playable
                .iter()
                .zip(&s.area)
                .any(|(a, b)| (a - b).abs() > 1e-3)
            {
                return bad(format!(
                    "world playable {:?} differs from the surroundings area {:?}",
                    w.playable, s.area
                ));
            }
        }
    }
    Ok(())
}

fn finite(map: &MapFile) -> bool {
    let pos = |p: &Pos| p.iter().all(|v| v.is_finite());
    let prop = |p: &Prop| pos(&p.at) && p.yaw.is_finite() && p.scale.is_finite();
    map.props
        .iter()
        .chain(map.prefabs.iter().flat_map(|p| &p.props))
        .all(prop)
        && map.spawns.iter().all(|s| pos(&s.at) && s.yaw.is_finite())
        && map.pickups.iter().all(|p| pos(&p.at))
        && map
            .lights
            .iter()
            .all(|l| pos(&l.at) && pos(&l.color) && l.intensity.is_finite() && l.range.is_finite())
        && map.zones.iter().all(|Zone::Box { kind, .. }| match kind {
            ZoneKind::JumpPad { velocity } => pos(velocity),
            ZoneKind::Teleporter { target, yaw } => pos(target) && yaw.is_finite(),
            ZoneKind::LowGravity { scale } => scale.is_finite(),
            _ => true,
        })
        && map.theme.weather.iter().all(|w| w.from_s.is_finite())
        && map.physics.as_ref().is_none_or(|p| {
            p.gravity.is_none_or(f32::is_finite) && p.speed_scale.is_none_or(f32::is_finite)
        })
}

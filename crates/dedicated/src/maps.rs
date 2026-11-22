//! Map catalog: maps embedded in the binary plus a `maps/` folder.
//!
//! Lot 4.11: map files (the `mapfmt` RON format of lots 3.1 to 3.3) load:
//! collision, spawns, zones, objective zones and pickups. The embedded maps
//! are the sim's test arena (no file) and the example map `example`
//! (`assets/maps/example.ron`).

use std::path::{Path, PathBuf};

use faset_core::glam::Vec3;
use faset_mapfmt::{MapFile, PickupKind, Zone, ZoneKind};
use faset_minigame_api::{MapPickup, ObjectiveZone};
use faset_server::{map_fingerprint, MapConfig};
use faset_sim::{MapCollision, Zones};

/// Name of the embedded test arena.
pub const TEST_ARENA: &str = "test-arena";
/// Name of the embedded example map.
pub const EXAMPLE: &str = "example";

const EXAMPLE_RON: &str = include_str!("../../../assets/maps/example.ron");

/// Height of a spawn's capsule centre above the spawn's feet position:
/// half the standing capsule plus 1 cm, so the first tick lands.
const SPAWN_LIFT: f32 = 0.91;

/// Where a map comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MapSource {
    /// Compiled into the binary.
    Embedded,
    /// A file in the maps folder.
    File(PathBuf),
}

/// A map ready for the server and its match.
#[derive(Debug, Clone)]
pub struct LoadedMap {
    /// What the server runs.
    pub config: MapConfig,
    /// The expanded map file (`None` for the test arena).
    pub file: Option<MapFile>,
    /// Objective zones, for the match.
    pub objectives: Vec<ObjectiveZone>,
    /// Pickups placed in the file, for the match.
    pub pickups: Vec<MapPickup>,
}

/// Known maps.
#[derive(Debug, Clone)]
pub struct Catalog {
    maps: Vec<(String, MapSource)>,
}

impl Default for Catalog {
    /// Embedded maps only.
    fn default() -> Self {
        Self {
            maps: vec![
                (TEST_ARENA.to_owned(), MapSource::Embedded),
                (EXAMPLE.to_owned(), MapSource::Embedded),
            ],
        }
    }
}

impl Catalog {
    /// Embedded maps plus every file in `dir` (name = file stem; embedded
    /// names win). A missing folder is not an error.
    #[must_use]
    pub fn scan(dir: &Path) -> Self {
        let mut maps = Self::default().maps;
        if let Ok(entries) = std::fs::read_dir(dir) {
            let mut files: Vec<_> = entries
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.is_file())
                .collect();
            files.sort();
            for p in files {
                if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                    if !maps.iter().any(|(n, _)| n == stem) {
                        maps.push((stem.to_owned(), MapSource::File(p)));
                    }
                }
            }
        }
        Self { maps }
    }

    /// All map names.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.maps.iter().map(|(n, _)| n.as_str())
    }

    /// Loads a map.
    ///
    /// # Errors
    /// Unknown name, an unreadable file or an invalid map file.
    pub fn load(&self, name: &str) -> Result<LoadedMap, String> {
        match self.maps.iter().find(|(n, _)| n == name) {
            None => Err(format!("unknown map `{name}`")),
            Some((_, MapSource::Embedded)) if name == TEST_ARENA => Ok(LoadedMap {
                config: MapConfig::default(),
                file: None,
                objectives: Vec::new(),
                pickups: Vec::new(),
            }),
            Some((_, MapSource::Embedded)) => from_bytes(name, EXAMPLE_RON.as_bytes()),
            Some((_, MapSource::File(p))) => {
                let bytes =
                    std::fs::read(p).map_err(|e| format!("map `{name}` ({}): {e}", p.display()))?;
                from_bytes(name, &bytes)
            }
        }
    }
}

/// Builds a map from its file bytes.
fn from_bytes(name: &str, bytes: &[u8]) -> Result<LoadedMap, String> {
    let parsed = faset_mapfmt::parse(bytes).map_err(|e| format!("map `{name}`: {e}"))?;
    let map = faset_mapfmt::expand(&parsed);
    let spawn_points: Vec<Vec3> = map
        .spawns
        .iter()
        .map(|s| Vec3::from_array(s.at) + Vec3::Y * SPAWN_LIFT)
        .collect();
    if spawn_points.is_empty() {
        return Err(format!("map `{name}` has no spawn point"));
    }
    let zones = Zones::from_map(&map);
    let objectives = map
        .zones
        .iter()
        .zip(zones.volumes())
        .filter_map(|(z, v)| match z {
            Zone::Box {
                kind: ZoneKind::Objective { id },
                ..
            } => Some(ObjectiveZone {
                id: id.clone(),
                min: v.min,
                max: v.max,
            }),
            _ => None,
        })
        .collect();
    let pickups = map
        .pickups
        .iter()
        .map(|p| MapPickup {
            kind: match &p.kind {
                PickupKind::Health => "health".to_owned(),
                PickupKind::Armor => "armor".to_owned(),
                PickupKind::Ammo => "ammo".to_owned(),
                PickupKind::Weapon(w) => format!("weapon:{w}"),
            },
            position: Vec3::from_array(p.at),
        })
        .collect();
    let config = MapConfig {
        statics: MapCollision::build(&map).world().clone(),
        fingerprint: map_fingerprint(bytes),
        spawn_points,
        zones,
    };
    Ok(LoadedMap {
        config,
        file: Some(map),
        objectives,
        pickups,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_and_folder_maps_are_listed() {
        let dir = std::env::temp_dir().join(format!("faset-maps-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("custom.ron"), "()").unwrap();
        let c = Catalog::scan(&dir);
        assert_eq!(
            c.names().collect::<Vec<_>>(),
            [TEST_ARENA, EXAMPLE, "custom"]
        );
        assert!(c.load(TEST_ARENA).is_ok());
        assert!(c.load("custom").is_err(), "not a map file");
        assert!(c.load("nope").is_err());
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(Catalog::scan(&dir).names().count(), 2);
    }

    #[test]
    fn the_example_map_loads_with_its_gameplay_data() {
        let m = Catalog::default().load(EXAMPLE).unwrap();
        assert!(m.file.is_some());
        assert!(m.config.spawn_points.len() >= 6);
        assert!(m.config.zones.volumes().len() >= 6);
        assert!(m.objectives.iter().any(|o| o.id == "flag_a"));
        assert!(m.pickups.iter().any(|p| p.kind == "health"));
        assert_ne!(m.config.fingerprint, MapConfig::default().fingerprint);
    }
}

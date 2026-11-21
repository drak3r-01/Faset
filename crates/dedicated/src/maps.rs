//! Map catalog: maps embedded in the binary plus a `maps/` folder.
//!
//! Decision: no map file format exists yet (a later lot defines it), so only
//! the embedded test arena can be loaded; files found in the folder are listed
//! and refused with a clear error until a loader lands.

use std::path::{Path, PathBuf};

use faset_server::MapConfig;

/// Name of the embedded test arena.
pub const TEST_ARENA: &str = "test-arena";

/// Where a map comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MapSource {
    /// Compiled into the binary.
    Embedded,
    /// A file in the maps folder.
    File(PathBuf),
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
            maps: vec![(TEST_ARENA.to_owned(), MapSource::Embedded)],
        }
    }
}

impl Catalog {
    /// Embedded maps plus every file in `dir` (name = file stem; embedded
    /// names win). A missing folder is not an error.
    #[must_use]
    pub fn scan(dir: &Path) -> Self {
        let mut maps = vec![(TEST_ARENA.to_owned(), MapSource::Embedded)];
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
    /// Unknown name, or a file map (no loader yet).
    pub fn load(&self, name: &str) -> Result<MapConfig, String> {
        match self.maps.iter().find(|(n, _)| n == name) {
            None => Err(format!("unknown map `{name}`")),
            Some((_, MapSource::Embedded)) => Ok(MapConfig::default()),
            Some((_, MapSource::File(p))) => Err(format!(
                "map `{name}` ({}) cannot be loaded: no map format loader yet",
                p.display()
            )),
        }
    }
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
        assert_eq!(c.names().collect::<Vec<_>>(), [TEST_ARENA, "custom"]);
        assert!(c.load(TEST_ARENA).is_ok());
        assert!(c.load("custom").is_err());
        assert!(c.load("nope").is_err());
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(Catalog::scan(&dir).names().count(), 1);
    }
}

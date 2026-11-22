//! Favourite servers and join history (lot 6.2), stored as RON.
//!
//! The file is [`FILE_NAME`] in the data folder ([`data_dir`]). A missing
//! file is an empty list; an unreadable one is an error the caller logs and
//! replaces by an empty list.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Name of the favourites file in the data folder (decision: the plan does
/// not fix it).
pub const FILE_NAME: &str = "favorites.ron";
/// Entries kept in the history.
pub const HISTORY_LEN: usize = 8;

/// Errors from reading or writing the favourites file.
#[derive(Debug, thiserror::Error)]
pub enum FavoritesError {
    /// The file cannot be read or written.
    #[error("favourites file: {0}")]
    Io(#[from] std::io::Error),
    /// The file is not valid RON.
    #[error("favourites file is invalid: {0}")]
    Parse(#[from] ron::error::SpannedError),
    /// The list cannot be serialised.
    #[error("favourites cannot be saved: {0}")]
    Encode(#[from] ron::Error),
}

/// Favourite and recent server addresses (`host:port`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Favorites {
    /// Servers the player starred, in the order added.
    pub favourites: Vec<String>,
    /// Servers joined recently, newest first.
    pub history: Vec<String>,
}

/// The per-user data folder: `%APPDATA%\Faset` on Windows,
/// `$XDG_DATA_HOME/faset` or `~/.local/share/faset` elsewhere.
#[must_use]
pub fn data_dir() -> Option<PathBuf> {
    let var = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    if cfg!(windows) {
        var("APPDATA").map(|p| p.join("Faset"))
    } else {
        var("XDG_DATA_HOME")
            .or_else(|| var("HOME").map(|h| h.join(".local").join("share")))
            .map(|p| p.join("faset"))
    }
}

impl Favorites {
    /// Reads `path`; a missing file gives an empty list.
    ///
    /// # Errors
    /// The file exists but cannot be read or parsed.
    pub fn load(path: &Path) -> Result<Self, FavoritesError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(ron::from_str(&text)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// Writes the list to `path`, creating its folder.
    ///
    /// # Errors
    /// The file cannot be written.
    pub fn save(&self, path: &Path) -> Result<(), FavoritesError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())?;
        std::fs::write(path, text)?;
        Ok(())
    }

    /// Stars `address`, or removes the star if it is there. Returns whether
    /// it is a favourite afterwards.
    pub fn toggle(&mut self, address: &str) -> bool {
        if let Some(i) = self.favourites.iter().position(|a| a == address) {
            self.favourites.remove(i);
            false
        } else {
            self.favourites.push(address.to_owned());
            true
        }
    }

    /// Whether `address` is a favourite.
    #[must_use]
    pub fn is_favourite(&self, address: &str) -> bool {
        self.favourites.iter().any(|a| a == address)
    }

    /// Records a join: `address` moves to the front of the history, which
    /// keeps [`HISTORY_LEN`] entries.
    pub fn record_join(&mut self, address: &str) {
        self.history.retain(|a| a != address);
        self.history.insert(0, address.to_owned());
        self.history.truncate(HISTORY_LEN);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("faset-fav-{}-{name}", std::process::id()))
    }

    #[test]
    fn favourites_survive_save_and_reload() {
        let dir = temp("roundtrip");
        let path = dir.join(FILE_NAME);
        let mut f = Favorites::default();
        assert!(f.toggle("192.168.1.5:7777"));
        f.toggle("10.0.0.2:7777");
        f.record_join("10.0.0.2:7777");
        f.save(&path).unwrap();
        let loaded = Favorites::load(&path).unwrap();
        assert_eq!(loaded, f);
        assert!(loaded.is_favourite("10.0.0.2:7777"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_file_is_empty_and_a_bad_one_is_an_error() {
        let dir = temp("missing");
        assert_eq!(
            Favorites::load(&dir.join(FILE_NAME)).unwrap(),
            Favorites::default()
        );
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(FILE_NAME), "not ron (").unwrap();
        assert!(Favorites::load(&dir.join(FILE_NAME)).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn toggle_and_history_rules() {
        let mut f = Favorites::default();
        assert!(f.toggle("a:1"));
        assert!(!f.toggle("a:1"));
        for i in 0..12 {
            f.record_join(&format!("h:{i}"));
        }
        f.record_join("h:5");
        assert_eq!(f.history.len(), HISTORY_LEN);
        assert_eq!(f.history[0], "h:5");
        assert_eq!(f.history.iter().filter(|a| *a == "h:5").count(), 1);
    }
}

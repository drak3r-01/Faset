//! Files: save to the user's `maps/` folder, export, import and autosave.
//!
//! Everything is written as the canonical RON of [`faset_mapfmt::to_ron`],
//! through a temporary file renamed over the target so a crash never leaves a
//! half-written map. The map identity is [`faset_mapfmt::fingerprint`], the
//! BLAKE3 hash of that canonical form (decision: the lot leaves the
//! canonicalisation open, `mapfmt` already defines it and the editor reuses it
//! as it is, so an exported map reloads with the same id).

use std::path::{Path, PathBuf};

use faset_mapfmt::{fingerprint, MapError, MapFile, MAX_MAP_BYTES};

/// Seconds between two autosaves.
pub const AUTOSAVE_INTERVAL_S: f32 = 60.0;

/// Why a file operation failed.
#[derive(Debug, thiserror::Error)]
pub enum FileError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// Not a valid map (rule 1), or larger than 256 KiB (rule 2).
    #[error("not a valid map: {0}")]
    Map(#[from] MapError),
    /// The saved form would exceed 256 KiB (rule 2).
    #[error("map is {size} bytes, limit is {MAX_MAP_BYTES}")]
    TooLarge { size: usize },
}

impl FileError {
    fn io(path: &Path) -> impl FnOnce(std::io::Error) -> Self + '_ {
        move |source| Self::Io {
            path: path.to_owned(),
            source,
        }
    }

    /// The validator rule this failure corresponds to: 2 for a file over
    /// the size limit, 1 for a schema or version problem, `None` for I/O.
    #[must_use]
    pub fn rule(&self) -> Option<u8> {
        match self {
            Self::Io { .. } => None,
            Self::TooLarge { .. } | Self::Map(MapError::TooLarge { .. }) => Some(2),
            Self::Map(_) => Some(1),
        }
    }
}

/// A map brought in with [`FileService::import`].
#[derive(Debug, Clone, PartialEq)]
pub struct Imported {
    pub map: MapFile,
    /// Where the copy in the `maps/` folder lives.
    pub path: PathBuf,
}

/// File operations of the editor, rooted at the game data directory.
#[derive(Debug, Clone)]
pub struct FileService {
    root: PathBuf,
    interval_s: f32,
    elapsed_s: f32,
    last_autosaved: Option<[u8; 32]>,
}

impl FileService {
    /// A service over the game data directory `root`; `maps/` and `autosave/`
    /// are created on first use.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            interval_s: AUTOSAVE_INTERVAL_S,
            elapsed_s: 0.0,
            last_autosaved: None,
        }
    }

    /// The per-user game data directory of this platform: `%APPDATA%\Faset`,
    /// `~/Library/Application Support/Faset` or `$XDG_DATA_HOME/faset`
    /// (decision: no path crate for three lines of logic).
    #[must_use]
    pub fn default_root() -> Option<PathBuf> {
        let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
        if cfg!(windows) {
            env("APPDATA").map(|p| p.join("Faset"))
        } else if cfg!(target_os = "macos") {
            env("HOME").map(|p| p.join("Library/Application Support/Faset"))
        } else {
            env("XDG_DATA_HOME")
                .or_else(|| env("HOME").map(|h| h.join(".local/share")))
                .map(|p| p.join("faset"))
        }
    }

    /// Changes the autosave period (tests use a short one).
    pub fn set_interval(&mut self, seconds: f32) {
        self.interval_s = seconds;
    }

    /// The user's `maps/` folder.
    #[must_use]
    pub fn maps_dir(&self) -> PathBuf {
        self.root.join("maps")
    }

    /// The folder of autosave files, separate from `maps/` so the map list
    /// never offers an autosave and a save is never overwritten by one.
    #[must_use]
    pub fn autosave_dir(&self) -> PathBuf {
        self.root.join("autosave")
    }

    /// File stem of a map: its name in lower case, letters, digits, `-` and
    /// `_` kept, anything else turned into `-`.
    #[must_use]
    pub fn slug(name: &str) -> String {
        let s: String = name
            .trim()
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '_' {
                    c.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .collect();
        let s = s.trim_matches('-');
        if s.is_empty() {
            "untitled".to_owned()
        } else {
            s.to_owned()
        }
    }

    /// Where `map` is saved: `maps/<slug>.ron`.
    #[must_use]
    pub fn save_path(&self, map: &MapFile) -> PathBuf {
        self.maps_dir()
            .join(format!("{}.ron", Self::slug(&map.meta.name)))
    }

    /// Where `map` is autosaved: `autosave/<slug>.autosave.ron`.
    #[must_use]
    pub fn autosave_path(&self, map: &MapFile) -> PathBuf {
        self.autosave_dir()
            .join(format!("{}.autosave.ron", Self::slug(&map.meta.name)))
    }

    /// Saves `map` to the `maps/` folder.
    ///
    /// # Errors
    /// [`FileError::TooLarge`] above 256 KiB, or an I/O failure.
    pub fn save(&self, map: &MapFile) -> Result<PathBuf, FileError> {
        let path = self.save_path(map);
        write_map(&path, map)?;
        tracing::info!(path = %path.display(), "map saved");
        Ok(path)
    }

    /// Writes a copy of `map` to `dest`, wherever the user chose.
    ///
    /// # Errors
    /// [`FileError::TooLarge`] above 256 KiB, or an I/O failure.
    pub fn export(&self, map: &MapFile, dest: &Path) -> Result<(), FileError> {
        write_map(dest, map)
    }

    /// Reads a map file without touching the library.
    ///
    /// # Errors
    /// An I/O failure, or [`FileError::Map`] if it is not a valid map.
    pub fn load(&self, path: &Path) -> Result<MapFile, FileError> {
        read_map(path)
    }

    /// Brings the map file `src` in: it is checked (rules 1 and 2), then its
    /// canonical form is copied into `maps/` under a name that does not
    /// replace an existing map (`name-2.ron`, `name-3.ron`...).
    ///
    /// # Errors
    /// An I/O failure, or [`FileError::Map`] if the file is not a valid map.
    pub fn import(&self, src: &Path) -> Result<Imported, FileError> {
        let map = read_map(src)?;
        let stem = Self::slug(&map.meta.name);
        let dir = self.maps_dir();
        let mut path = dir.join(format!("{stem}.ron"));
        let mut n = 2;
        while path.exists() {
            path = dir.join(format!("{stem}-{n}.ron"));
            n += 1;
        }
        write_map(&path, &map)?;
        Ok(Imported { map, path })
    }

    /// Advances the autosave clock by `dt` seconds. After a full period, and
    /// only when the map changed since the last autosave, writes the autosave
    /// file and returns its path. The saved map file is never touched.
    ///
    /// # Errors
    /// An I/O failure; the period restarts either way.
    pub fn tick(&mut self, dt: f32, map: &MapFile) -> Result<Option<PathBuf>, FileError> {
        self.elapsed_s += dt;
        if self.elapsed_s < self.interval_s {
            return Ok(None);
        }
        self.elapsed_s = 0.0;
        let fp = fingerprint(map);
        if self.last_autosaved == Some(fp) {
            return Ok(None);
        }
        let path = self.autosave_now(map)?;
        Ok(Some(path))
    }

    /// Writes the autosave file right away.
    ///
    /// # Errors
    /// [`FileError::TooLarge`] above 256 KiB, or an I/O failure.
    pub fn autosave_now(&mut self, map: &MapFile) -> Result<PathBuf, FileError> {
        let path = self.autosave_path(map);
        debug_assert_ne!(path, self.save_path(map));
        write_map(&path, map)?;
        self.last_autosaved = Some(fingerprint(map));
        tracing::debug!(path = %path.display(), "autosaved");
        Ok(path)
    }

    /// Reads back the autosave of the map called `name`, if there is one.
    ///
    /// # Errors
    /// An I/O failure or an invalid file.
    pub fn load_autosave(&self, name: &str) -> Result<MapFile, FileError> {
        read_map(
            &self
                .autosave_dir()
                .join(format!("{}.autosave.ron", Self::slug(name))),
        )
    }
}

fn read_map(path: &Path) -> Result<MapFile, FileError> {
    let io = FileError::io(path);
    let len = std::fs::metadata(path).map_err(FileError::io(path))?.len();
    if len > MAX_MAP_BYTES as u64 {
        return Err(MapError::TooLarge {
            size: usize::try_from(len).unwrap_or(usize::MAX),
        }
        .into());
    }
    let bytes = std::fs::read(path).map_err(io)?;
    Ok(faset_mapfmt::parse(&bytes)?)
}

fn write_map(path: &Path, map: &MapFile) -> Result<(), FileError> {
    let text = faset_mapfmt::to_ron(map);
    if text.len() > MAX_MAP_BYTES {
        return Err(FileError::TooLarge { size: text.len() });
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(FileError::io(dir))?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, text).map_err(FileError::io(&tmp))?;
    std::fs::rename(&tmp, path).map_err(FileError::io(path))
}

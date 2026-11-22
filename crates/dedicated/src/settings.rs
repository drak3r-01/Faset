//! `server.toml` schema and command-line arguments (arguments win).

use std::path::{Path, PathBuf};

use clap::Parser;
use serde::Deserialize;

/// Configuration problem.
#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    /// The config file could not be read.
    #[error("cannot read {0}: {1}")]
    Read(PathBuf, std::io::Error),
    /// The config file is not valid `server.toml`.
    #[error("invalid {0}: {1}")]
    Parse(PathBuf, toml::de::Error),
    /// A value is out of range.
    #[error("{0}")]
    Invalid(String),
}

/// Command-line arguments. Every option overrides the same key of the file.
#[derive(Debug, Default, Clone, Parser)]
#[command(name = "dedicated", version, about = "Faset dedicated server")]
pub struct Cli {
    /// Path of the configuration file (read if present).
    #[arg(long, default_value = "server.toml")]
    pub config: PathBuf,
    /// Server name.
    #[arg(long)]
    pub name: Option<String>,
    /// UDP port.
    #[arg(long)]
    pub port: Option<u16>,
    /// Maximum players (humans and bots).
    #[arg(long)]
    pub max_players: Option<usize>,
    /// Game mode id.
    #[arg(long)]
    pub mode: Option<u8>,
    /// Hosted game mode (`fps.ffa`, `fps.team`); empty for none.
    #[arg(long)]
    pub game_mode: Option<String>,
    /// World seed.
    #[arg(long)]
    pub seed: Option<u64>,
    /// Map rotation, comma separated.
    #[arg(long, value_delimiter = ',')]
    pub rotation: Option<Vec<String>>,
    /// Seconds per map.
    #[arg(long)]
    pub rotation_seconds: Option<u32>,
    /// Number of bots.
    #[arg(long)]
    pub bots: Option<usize>,
    /// Bot skill, 0 to 10.
    #[arg(long)]
    pub bot_level: Option<u8>,
    /// Join password.
    #[arg(long)]
    pub password: Option<String>,
    /// Folder of extra maps (default: `maps/` next to the executable).
    #[arg(long)]
    pub maps_dir: Option<PathBuf>,
}

/// Effective settings; the `server.toml` schema (all keys optional).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    /// Server name.
    pub name: String,
    /// UDP port.
    pub port: u16,
    /// Maximum players, humans and bots.
    pub max_players: usize,
    /// Game mode id sent in the handshake when no `game_mode` is hosted.
    pub mode: u8,
    /// Hosted game mode, by descriptor id (`fps.ffa`, `fps.team`; lot
    /// 4.11). Empty (the default): no match, players move freely.
    pub game_mode: String,
    /// World seed (spread, respawn draws).
    pub seed: u64,
    /// Map names, played in order.
    pub rotation: Vec<String>,
    /// Seconds per map.
    pub rotation_seconds: u32,
    /// Number of bots.
    pub bots: usize,
    /// Bot skill, 0 to 10.
    pub bot_level: u8,
    /// Join password, empty for none.
    pub password: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            name: "Faset server".into(),
            port: 7777,
            max_players: 16,
            mode: 0,
            game_mode: String::new(),
            seed: 0,
            rotation: vec![crate::maps::TEST_ARENA.into()],
            rotation_seconds: 300,
            bots: 0,
            bot_level: 5,
            password: String::new(),
        }
    }
}

impl Settings {
    /// Parses `server.toml` text.
    ///
    /// # Errors
    /// [`SettingsError::Parse`] on bad TOML or unknown keys.
    pub fn from_toml(text: &str) -> Result<Self, SettingsError> {
        toml::from_str(text).map_err(|e| SettingsError::Parse(PathBuf::from("server.toml"), e))
    }

    /// Reads `path` if it exists (defaults otherwise), then applies `cli`.
    ///
    /// # Errors
    /// Unreadable or invalid file, or an out-of-range value.
    pub fn load(path: &Path, cli: &Cli) -> Result<Self, SettingsError> {
        let mut s = match std::fs::read_to_string(path) {
            Ok(text) => {
                toml::from_str(&text).map_err(|e| SettingsError::Parse(path.to_path_buf(), e))?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => return Err(SettingsError::Read(path.to_path_buf(), e)),
        };
        s.apply(cli);
        s.validate()?;
        Ok(s)
    }

    /// Overrides settings with the arguments that were given.
    pub fn apply(&mut self, cli: &Cli) {
        macro_rules! over {
            ($($f:ident),*) => {$(if let Some(v) = &cli.$f { self.$f = v.clone(); })*};
        }
        over!(
            name,
            port,
            max_players,
            mode,
            game_mode,
            seed,
            rotation,
            rotation_seconds,
            bots,
            bot_level,
            password
        );
    }

    /// Range checks the server crate does not do.
    ///
    /// # Errors
    /// [`SettingsError::Invalid`].
    pub fn validate(&self) -> Result<(), SettingsError> {
        let bad = |m: &str| Err(SettingsError::Invalid(m.into()));
        if self.rotation.is_empty() {
            return bad("rotation must list at least one map");
        }
        if self.rotation_seconds == 0 {
            return bad("rotation_seconds must be positive");
        }
        if self.bot_level > 10 {
            return bad("bot_level must be 0 to 10");
        }
        if self.bots > self.max_players {
            return bad("bots exceeds max_players");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_spec() {
        let s = Settings::default();
        assert_eq!((s.port, s.rotation_seconds), (7777, 300));
        s.validate().unwrap();
    }

    #[test]
    fn toml_parses_and_rejects_unknown_keys() {
        let s = Settings::from_toml("port = 9000\nbots = 2\nrotation = [\"a\", \"b\"]").unwrap();
        assert_eq!((s.port, s.bots, s.rotation.len()), (9000, 2, 2));
        assert_eq!(s.max_players, 16);
        assert!(Settings::from_toml("prot = 1").is_err());
    }

    #[test]
    fn arguments_override_the_file() {
        let mut s = Settings::from_toml("port = 9000\nname = \"file\"").unwrap();
        let cli = Cli::parse_from(["dedicated", "--port", "8000", "--rotation", "x,y"]);
        s.apply(&cli);
        assert_eq!(s.port, 8000);
        assert_eq!(s.name, "file");
        assert_eq!(s.rotation, ["x", "y"]);
    }

    #[test]
    fn example_file_is_valid_and_equals_defaults() {
        let s = Settings::from_toml(include_str!("../server.toml")).unwrap();
        assert_eq!(s, Settings::default());
        s.validate().unwrap();
    }

    #[test]
    fn invalid_values_are_rejected() {
        let s = Settings {
            bots: 99,
            ..Settings::default()
        };
        assert!(s.validate().is_err());
    }
}

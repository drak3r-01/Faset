//! The player's profile (lot 6.3): the custom classes they built, stored as
//! versioned RON in `profile.ron` of the data folder ([`data_dir`](crate::data_dir)).
//!
//! A missing file is the default profile; an unreadable one, or one written by
//! a newer version, is an error the caller logs before using the default.
//! [`Profile::sanitised`] replaces what the server would refuse (an unknown
//! weapon, a loadout over budget) with the default class, so a hand-edited
//! file never makes the client send a loadout it knows is invalid.

use std::path::Path;

use faset_mode_fps::{validate_loadout, Loadout};
use faset_ui::PALETTE;
use serde::{Deserialize, Serialize};

/// Name of the profile file in the data folder.
pub const FILE_NAME: &str = "profile.ron";
/// Version this build reads and writes.
pub const PROFILE_VERSION: u32 = 1;

/// Errors from reading or writing the profile.
#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    /// The file cannot be read or written.
    #[error("profile file: {0}")]
    Io(#[from] std::io::Error),
    /// The file is not valid RON.
    #[error("profile file is invalid: {0}")]
    Parse(#[from] ron::error::SpannedError),
    /// The profile cannot be serialised.
    #[error("profile cannot be saved: {0}")]
    Encode(#[from] ron::Error),
    /// The file was written by a newer version.
    #[error("profile version {found} is not supported (this build reads {PROFILE_VERSION})")]
    Version { found: u32 },
}

/// The colours chosen for a character: indices into [`PALETTE`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Appearance {
    pub body: u8,
    pub accent: u8,
}

impl Default for Appearance {
    fn default() -> Self {
        Self { body: 0, accent: 1 }
    }
}

/// A class the player built.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedClass {
    pub name: String,
    pub loadout: Loadout,
    pub appearance: Appearance,
}

impl Default for SavedClass {
    fn default() -> Self {
        Self {
            name: "Trooper".to_owned(),
            loadout: Loadout::trooper(),
            appearance: Appearance::default(),
        }
    }
}

/// What the client remembers about the player.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    pub version: u32,
    /// The player's classes; never empty after [`sanitised`](Self::sanitised).
    pub classes: Vec<SavedClass>,
    /// Index of the class taken into matches.
    pub selected: usize,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            version: PROFILE_VERSION,
            classes: vec![SavedClass::default()],
            selected: 0,
        }
    }
}

impl Profile {
    /// Reads `path`; a missing file gives the default profile. The result is
    /// [`sanitised`](Self::sanitised).
    ///
    /// # Errors
    /// The file exists but cannot be read, parsed, or is of a newer version.
    pub fn load(path: &Path) -> Result<Self, ProfileError> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e.into()),
        };
        let profile: Self = ron::from_str(&text)?;
        if profile.version != PROFILE_VERSION {
            return Err(ProfileError::Version {
                found: profile.version,
            });
        }
        Ok(profile.sanitised())
    }

    /// Writes the profile to `path`, creating its folder.
    ///
    /// # Errors
    /// The file cannot be written.
    pub fn save(&self, path: &Path) -> Result<(), ProfileError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())?;
        std::fs::write(path, text)?;
        Ok(())
    }

    /// The class taken into matches.
    #[must_use]
    pub fn current(&self) -> &SavedClass {
        &self.classes[self.selected.min(self.classes.len().saturating_sub(1))]
    }

    /// Replaces the selected class.
    pub fn set_current(&mut self, class: SavedClass) {
        let i = self.selected.min(self.classes.len().saturating_sub(1));
        match self.classes.get_mut(i) {
            Some(slot) => *slot = class,
            None => self.classes.push(class),
        }
        self.selected = i;
    }

    /// The profile with every invalid class replaced by the default one,
    /// colours kept inside the palette and the selection inside the list.
    #[must_use]
    pub fn sanitised(mut self) -> Self {
        let colours = u8::try_from(PALETTE.len()).unwrap_or(u8::MAX);
        for class in &mut self.classes {
            if let Err(e) = validate_loadout(&class.loadout) {
                tracing::warn!(class = %class.name, error = %e, "saved class replaced by the default");
                class.loadout = Loadout::trooper();
            }
            class.appearance.body = class.appearance.body.min(colours - 1);
            class.appearance.accent = class.appearance.accent.min(colours - 1);
        }
        if self.classes.is_empty() {
            self.classes.push(SavedClass::default());
        }
        self.selected = self.selected.min(self.classes.len() - 1);
        self.version = PROFILE_VERSION;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("faset-profile-{}-{name}", std::process::id()))
    }

    fn custom() -> Profile {
        let mut p = Profile::default();
        p.classes.push(SavedClass {
            name: "Chaos".into(),
            loadout: Loadout {
                class: "prankster".into(),
                primary: "smg".into(),
                secondary: Some("sidearm".into()),
                melee: Some("baguette".into()),
                gadget: Some("bubble_gun".into()),
            },
            appearance: Appearance { body: 4, accent: 2 },
        });
        p.selected = 1;
        p
    }

    #[test]
    fn profile_round_trips_with_the_same_content_and_version() {
        let path = temp("roundtrip").join("profile.ron");
        let p = custom();
        p.save(&path).unwrap();
        let back = Profile::load(&path).unwrap();
        assert_eq!(back, p);
        assert_eq!(back.version, PROFILE_VERSION);
        assert_eq!(back.current().name, "Chaos");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn missing_file_is_the_default_and_bad_files_are_errors() {
        let dir = temp("errors");
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(
            Profile::load(&dir.join("absent.ron")).unwrap(),
            Profile::default()
        );
        let bad = dir.join("bad.ron");
        std::fs::write(&bad, "(not ron").unwrap();
        assert!(matches!(Profile::load(&bad), Err(ProfileError::Parse(_))));
        let newer = Profile {
            version: PROFILE_VERSION + 1,
            ..Profile::default()
        };
        let path = dir.join("newer.ron");
        newer.save(&path).unwrap();
        assert!(matches!(
            Profile::load(&path),
            Err(ProfileError::Version { .. })
        ));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn invalid_classes_fall_back_to_the_trooper() {
        let mut p = custom();
        p.classes[1].loadout.gadget = Some("toaster".into());
        p.classes[1].loadout.primary = "precision_rifle".into();
        p.classes[1].loadout.melee = Some("baguette".into());
        p.classes[1].appearance = Appearance {
            body: 200,
            accent: 9,
        };
        p.selected = 7;
        let s = p.sanitised();
        assert_eq!(s.classes[1].loadout, Loadout::trooper());
        assert_eq!(s.selected, 1);
        assert!(usize::from(s.classes[1].appearance.body) < PALETTE.len());
        let empty = Profile {
            classes: Vec::new(),
            ..Profile::default()
        };
        assert_eq!(empty.sanitised().classes.len(), 1);
    }
}

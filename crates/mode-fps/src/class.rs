//! Classes, loadouts and the catalog that ties them to weapons (lot 4.6).
//!
//! the proposed path of the lot file). [`Catalog::builtin`] embeds them;
//! [`Catalog::load_dir`] reads the same layout from disk. Loading checks
//! every weapon reference, so a class can never name a missing weapon.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::weapon::{WeaponDef, WeaponError};

/// Base run speed in m/s; a class multiplies it by its speed factor.
pub const BASE_RUN_SPEED: f32 = 7.0;

/// The trait that sets a class apart. The mode applies it; the data names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClassTrait {
    AllRound,
    DoubleJump,
    Resistant,
    Scope,
    ExplosiveJump,
    QuirkyArsenal,
}

/// A starting class, as loaded from its RON file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClassDef {
    /// Unique key.
    pub id: String,
    pub health: f32,
    /// Multiplies [`BASE_RUN_SPEED`].
    pub speed_factor: f32,
    pub class_trait: ClassTrait,
    /// Weapon id of the main weapon.
    pub main_weapon: String,
    /// Weapon ids a loadout may pick as its secondary.
    #[serde(default)]
    pub secondaries: Vec<String>,
}

impl ClassDef {
    /// Run speed in m/s.
    #[must_use]
    pub fn run_speed(&self) -> f32 {
        BASE_RUN_SPEED * self.speed_factor
    }
}

/// What a player brings: a class and at most one secondary from its list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoadoutDef {
    pub class: String,
    #[serde(default)]
    pub secondary: Option<String>,
}

/// Data that cannot be used.
#[derive(Debug, thiserror::Error)]
pub enum ClassError {
    /// A weapon file is invalid.
    #[error(transparent)]
    Weapon(#[from] WeaponError),
    /// A class file is not valid RON for [`ClassDef`] (unknown fields included).
    #[error("class file: {0}")]
    Parse(#[from] ron::error::SpannedError),
    /// A value out of its range.
    #[error("class `{id}`: {reason}")]
    Invalid { id: String, reason: &'static str },
    /// Two files share an id.
    #[error("duplicate id `{0}`")]
    Duplicate(String),
    /// A class or loadout names a weapon that does not exist.
    #[error("`{owner}` references unknown weapon `{weapon}`")]
    UnknownWeapon { owner: String, weapon: String },
    /// A loadout names a class that does not exist.
    #[error("loadout references unknown class `{0}`")]
    UnknownClass(String),
    /// A loadout secondary that its class does not allow.
    #[error("class `{class}` does not allow secondary `{weapon}`")]
    SecondaryNotAllowed { class: String, weapon: String },
    /// A data directory could not be read.
    #[error("reading {path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
}

/// Every weapon and class, validated against each other.
#[derive(Debug, Clone, PartialEq)]
pub struct Catalog {
    weapons: BTreeMap<String, WeaponDef>,
    classes: Vec<ClassDef>,
}

impl Catalog {
    /// Builds a catalog from RON texts.
    ///
    /// # Errors
    /// Any invalid file, duplicate id or dangling weapon reference.
    pub fn from_ron<'a>(
        weapons: impl IntoIterator<Item = &'a str>,
        classes: impl IntoIterator<Item = &'a str>,
    ) -> Result<Self, ClassError> {
        let mut map = BTreeMap::new();
        for text in weapons {
            let w = WeaponDef::from_ron(text)?;
            let id = w.id.clone();
            if map.insert(id.clone(), w).is_some() {
                return Err(ClassError::Duplicate(id));
            }
        }
        let mut list: Vec<ClassDef> = Vec::new();
        for text in classes {
            let c: ClassDef = ron::from_str(text)?;
            if list.iter().any(|o| o.id == c.id) {
                return Err(ClassError::Duplicate(c.id));
            }
            list.push(c);
        }
        let catalog = Self {
            weapons: map,
            classes: list,
        };
        catalog.validate()?;
        Ok(catalog)
    }

    fn validate(&self) -> Result<(), ClassError> {
        for c in &self.classes {
            let bad = |reason| {
                Err(ClassError::Invalid {
                    id: c.id.clone(),
                    reason,
                })
            };
            if !(c.health.is_finite() && c.health > 0.0) {
                return bad("health must be positive");
            }
            if !(c.speed_factor.is_finite() && c.speed_factor > 0.0) {
                return bad("speed_factor must be positive");
            }
            for w in std::iter::once(&c.main_weapon).chain(&c.secondaries) {
                if !self.weapons.contains_key(w) {
                    return Err(ClassError::UnknownWeapon {
                        owner: c.id.clone(),
                        weapon: w.clone(),
                    });
                }
            }
        }
        Ok(())
    }

    /// The catalog shipped with the crate: six classes, their main weapons,
    /// the sidearm and the quirky gadgets (classes in file-name order, like
    /// [`load_dir`](Self::load_dir)).
    ///
    /// # Panics
    /// Never: the embedded files are covered by tests.
    #[must_use]
    pub fn builtin() -> Self {
        let weapons = [
            include_str!("../data/weapons/assault_rifle.ron"),
            include_str!("../data/weapons/smg.ron"),
            include_str!("../data/weapons/pump_shotgun.ron"),
            include_str!("../data/weapons/precision_rifle.ron"),
            include_str!("../data/weapons/boomer_rocket_launcher.ron"),
            include_str!("../data/weapons/rubber_chicken_launcher.ron"),
            include_str!("../data/weapons/sidearm.ron"),
            include_str!("../data/weapons/banana_peel.ron"),
            include_str!("../data/weapons/confetti_cannon.ron"),
            include_str!("../data/weapons/baguette.ron"),
            include_str!("../data/weapons/toaster.ron"),
            include_str!("../data/weapons/bubble_gun.ron"),
        ];
        let classes = [
            include_str!("../data/classes/boomer.ron"),
            include_str!("../data/classes/bruiser.ron"),
            include_str!("../data/classes/marksman.ron"),
            include_str!("../data/classes/prankster.ron"),
            include_str!("../data/classes/sprinter.ron"),
            include_str!("../data/classes/trooper.ron"),
        ];
        Self::from_ron(weapons, classes).expect("embedded data is valid")
    }

    /// Loads `<dir>/weapons/*.ron` and `<dir>/classes/*.ron`, in file-name order.
    ///
    /// # Errors
    /// Unreadable directories plus everything [`from_ron`](Self::from_ron) reports.
    pub fn load_dir(dir: &Path) -> Result<Self, ClassError> {
        let read = |sub: &str| -> Result<Vec<String>, ClassError> {
            let path = dir.join(sub);
            let mut files: Vec<_> = std::fs::read_dir(&path)
                .map_err(|source| ClassError::Io {
                    path: path.display().to_string(),
                    source,
                })?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|x| x == "ron"))
                .collect();
            files.sort();
            files
                .iter()
                .map(|p| {
                    std::fs::read_to_string(p).map_err(|source| ClassError::Io {
                        path: p.display().to_string(),
                        source,
                    })
                })
                .collect()
        };
        let (w, c) = (read("weapons")?, read("classes")?);
        Self::from_ron(w.iter().map(String::as_str), c.iter().map(String::as_str))
    }

    #[must_use]
    pub fn classes(&self) -> &[ClassDef] {
        &self.classes
    }

    /// Weapons in id order.
    pub fn weapons(&self) -> impl Iterator<Item = &WeaponDef> {
        self.weapons.values()
    }

    #[must_use]
    pub fn weapon(&self, id: &str) -> Option<&WeaponDef> {
        self.weapons.get(id)
    }

    #[must_use]
    pub fn class(&self, id: &str) -> Option<&ClassDef> {
        self.classes.iter().find(|c| c.id == id)
    }

    /// Checks a loadout against the catalog.
    ///
    /// # Errors
    /// Unknown class or weapon, or a secondary the class does not allow.
    pub fn check_loadout(&self, loadout: &LoadoutDef) -> Result<(), ClassError> {
        let class = self
            .class(&loadout.class)
            .ok_or_else(|| ClassError::UnknownClass(loadout.class.clone()))?;
        if let Some(s) = &loadout.secondary {
            if !self.weapons.contains_key(s) {
                return Err(ClassError::UnknownWeapon {
                    owner: format!("loadout of {}", class.id),
                    weapon: s.clone(),
                });
            }
            if !class.secondaries.contains(s) {
                return Err(ClassError::SecondaryNotAllowed {
                    class: class.id.clone(),
                    weapon: s.clone(),
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIDEARM: &str = include_str!("../data/weapons/sidearm.ron");

    #[test]
    fn builtin_catalog_has_six_classes_with_valid_weapons() {
        let c = Catalog::builtin();
        assert_eq!(c.classes().len(), 6);
        assert_eq!(c.weapons().count(), 12);
        for class in c.classes() {
            assert!(c.weapon(&class.main_weapon).is_some());
        }
        assert!((c.class("sprinter").unwrap().run_speed() - 7.84).abs() < 1e-5);
    }

    #[test]
    fn unknown_weapon_reference_is_rejected() {
        let class = r#"(id: "x", health: 100.0, speed_factor: 1.0, class_trait: AllRound, main_weapon: "ghost")"#;
        let err = Catalog::from_ron([SIDEARM], [class]).unwrap_err();
        assert!(matches!(err, ClassError::UnknownWeapon { .. }), "{err}");
        let class = r#"(id: "x", health: 100.0, speed_factor: 1.0, class_trait: AllRound, main_weapon: "sidearm", secondaries: ["ghost"])"#;
        assert!(Catalog::from_ron([SIDEARM], [class]).is_err());
    }

    #[test]
    fn unknown_fields_and_duplicates_are_rejected() {
        let class = r#"(id: "x", health: 100.0, speed_factor: 1.0, class_trait: AllRound, main_weapon: "sidearm", bogus: 1)"#;
        assert!(matches!(
            Catalog::from_ron([SIDEARM], [class]),
            Err(ClassError::Parse(_))
        ));
        let weapon = SIDEARM.replace("range: 200.0,", "range: 200.0,\n    bogus: 1,");
        assert!(Catalog::from_ron([weapon.as_str()], []).is_err());
        assert!(matches!(
            Catalog::from_ron([SIDEARM, SIDEARM], []),
            Err(ClassError::Duplicate(_))
        ));
    }

    #[test]
    fn loadouts_are_checked() {
        let c = Catalog::builtin();
        let ok = LoadoutDef {
            class: "prankster".into(),
            secondary: Some("banana_peel".into()),
        };
        assert!(c.check_loadout(&ok).is_ok());
        let not_allowed = LoadoutDef {
            class: "marksman".into(),
            secondary: Some("toaster".into()),
        };
        assert!(matches!(
            c.check_loadout(&not_allowed),
            Err(ClassError::SecondaryNotAllowed { .. })
        ));
        let ghost = LoadoutDef {
            class: "marksman".into(),
            secondary: Some("ghost".into()),
        };
        assert!(matches!(
            c.check_loadout(&ghost),
            Err(ClassError::UnknownWeapon { .. })
        ));
        let no_class = LoadoutDef {
            class: "nobody".into(),
            secondary: None,
        };
        assert!(matches!(
            c.check_loadout(&no_class),
            Err(ClassError::UnknownClass(_))
        ));
    }

    #[test]
    fn load_dir_reads_the_shipped_data() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("data");
        assert_eq!(Catalog::load_dir(&dir).unwrap(), Catalog::builtin());
        assert!(matches!(
            Catalog::load_dir(Path::new("no/such/dir")),
            Err(ClassError::Io { .. })
        ));
    }
}

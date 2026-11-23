//! Player loadouts and their budget (lot 6.3).
//!
//! A [`Loadout`] is a class plus a primary weapon, an optional secondary, an
//! optional melee weapon and an optional gadget. The weapon pools and the
//! point costs live in `data/loadout.ron` ([`LoadoutRules`]); the sum of the
//! costs must stay within the budget. [`validate_loadout`] is the check the
//! server runs on every loadout it receives; [`LoadoutMsg`] is the wire form.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::class::{Catalog, ClassDef};
use crate::rules::DEFAULT_CLASS;

/// The shipped loadout data.
const BUILTIN: &str = include_str!("../data/loadout.ron");

/// Byte meaning "no weapon" in [`LoadoutMsg`].
pub const NONE_BYTE: u8 = u8::MAX;

/// What a player brings, by id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Loadout {
    pub class: String,
    pub primary: String,
    #[serde(default)]
    pub secondary: Option<String>,
    #[serde(default)]
    pub melee: Option<String>,
    #[serde(default)]
    pub gadget: Option<String>,
}

/// Why a loadout is refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LoadoutError {
    #[error("unknown class `{0}`")]
    UnknownClass(String),
    #[error("unknown weapon `{0}`")]
    UnknownWeapon(String),
    /// The weapon exists but does not belong in this slot.
    #[error("`{weapon}` cannot go in the {slot} slot")]
    WrongSlot { slot: &'static str, weapon: String },
    /// The class does not allow the weapon.
    #[error("class `{class}` does not allow `{weapon}`")]
    NotAllowed { class: String, weapon: String },
    #[error("loadout costs {cost} points, the budget is {budget}")]
    OverBudget { cost: u32, budget: u32 },
    /// A message that does not decode to a loadout.
    #[error("malformed loadout message")]
    Malformed,
}

impl LoadoutError {
    /// Stable code for the wire ([`crate::ServerMsg::LoadoutReplaced`]).
    #[must_use]
    pub fn code(&self) -> u8 {
        match self {
            Self::UnknownClass(_) => 1,
            Self::UnknownWeapon(_) => 2,
            Self::WrongSlot { .. } => 3,
            Self::NotAllowed { .. } => 4,
            Self::OverBudget { .. } => 5,
            Self::Malformed => 6,
        }
    }
}

/// Budget, costs and the weapon pool of each slot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoadoutRules {
    /// Points a loadout may spend.
    pub budget: u32,
    /// Cost of each weapon id.
    pub costs: BTreeMap<String, u32>,
    pub primaries: Vec<String>,
    pub secondaries: Vec<String>,
    pub melee: Vec<String>,
    pub gadgets: Vec<String>,
}

impl LoadoutRules {
    /// The rules shipped with the crate.
    ///
    /// # Panics
    /// Never: the embedded file is covered by tests.
    #[must_use]
    pub fn builtin() -> &'static Self {
        static RULES: OnceLock<LoadoutRules> = OnceLock::new();
        RULES.get_or_init(|| ron::from_str(BUILTIN).expect("embedded loadout data is valid"))
    }

    /// Points a weapon costs (0 for a weapon without a price).
    #[must_use]
    pub fn cost(&self, weapon: &str) -> u32 {
        self.costs.get(weapon).copied().unwrap_or(0)
    }

    /// Checks `loadout` against `classes`. Weapon ids are known through the
    /// cost table, which a test ties to the weapon list.
    ///
    /// # Errors
    /// The first rule the loadout breaks.
    pub fn validate(&self, classes: &[ClassDef], loadout: &Loadout) -> Result<(), LoadoutError> {
        let class = classes
            .iter()
            .find(|c| c.id == loadout.class)
            .ok_or_else(|| LoadoutError::UnknownClass(loadout.class.clone()))?;
        let in_slot = |slot: &'static str, w: &str, pool: &[String]| {
            if !self.costs.contains_key(w) {
                Err(LoadoutError::UnknownWeapon(w.to_owned()))
            } else if !pool.iter().any(|p| p == w) {
                Err(LoadoutError::WrongSlot {
                    slot,
                    weapon: w.to_owned(),
                })
            } else {
                Ok(())
            }
        };
        in_slot("primary", &loadout.primary, &self.primaries)?;
        for (slot, w, pool) in [
            ("secondary", &loadout.secondary, &self.secondaries),
            ("melee", &loadout.melee, &self.melee),
            ("gadget", &loadout.gadget, &self.gadgets),
        ] {
            if let Some(w) = w {
                in_slot(slot, w, pool)?;
                if !class.secondaries.iter().any(|s| s == w) {
                    return Err(LoadoutError::NotAllowed {
                        class: class.id.clone(),
                        weapon: w.clone(),
                    });
                }
            }
        }
        let cost = loadout.budget_cost_in(self);
        if cost > self.budget {
            return Err(LoadoutError::OverBudget {
                cost,
                budget: self.budget,
            });
        }
        Ok(())
    }
}

impl Loadout {
    /// The loadout a class starts with: its main weapon and first secondary.
    #[must_use]
    pub fn for_class(class: &ClassDef) -> Self {
        Self {
            class: class.id.clone(),
            primary: class.main_weapon.clone(),
            secondary: class.secondaries.first().cloned(),
            melee: None,
            gadget: None,
        }
    }

    /// The default class's loadout (Trooper).
    ///
    /// # Panics
    /// Never: the built-in catalog has the default class.
    #[must_use]
    pub fn trooper() -> Self {
        Self::for_class(
            Catalog::builtin_ref()
                .class(DEFAULT_CLASS)
                .expect("the built-in catalog has the default class"),
        )
    }

    /// Points spent, with the shipped costs.
    #[must_use]
    pub fn budget_cost(&self) -> u32 {
        self.budget_cost_in(LoadoutRules::builtin())
    }

    /// Points spent under `rules`.
    #[must_use]
    pub fn budget_cost_in(&self, rules: &LoadoutRules) -> u32 {
        std::iter::once(&self.primary)
            .chain(self.secondary.iter())
            .chain(self.melee.iter())
            .chain(self.gadget.iter())
            .map(|w| rules.cost(w))
            .sum()
    }

    /// The wire form, or `None` if a name is not in `catalog`.
    #[must_use]
    pub fn to_msg(&self, catalog: &Catalog) -> Option<LoadoutMsg> {
        let weapon = |id: &str| {
            catalog
                .weapons()
                .position(|w| w.id == id)
                .and_then(|i| u8::try_from(i).ok())
        };
        let opt = |w: &Option<String>| match w {
            None => Some(NONE_BYTE),
            Some(w) => weapon(w),
        };
        let class = catalog.classes().iter().position(|c| c.id == self.class)?;
        Some(LoadoutMsg {
            class: u8::try_from(class).ok()?,
            primary: weapon(&self.primary)?,
            secondary: opt(&self.secondary)?,
            melee: opt(&self.melee)?,
            gadget: opt(&self.gadget)?,
        })
    }
}

/// Validates `loadout` against the shipped catalog and rules.
///
/// # Errors
/// [`LoadoutError`] for an unknown class or weapon, a weapon in the wrong
/// slot or not allowed by the class, or a cost over the budget.
pub fn validate_loadout(loadout: &Loadout) -> Result<(), LoadoutError> {
    LoadoutRules::builtin().validate(Catalog::builtin_ref().classes(), loadout)
}

/// Client to server: the full loadout.
///
/// Layout: tag 3 (1), class index into the catalog's classes (1), then
/// primary, secondary, melee and gadget (1 each): an index into the weapon
/// list (catalog id order), 255 for none. It widens [`crate::ClassMsg`]
/// (tag 2), which stays valid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadoutMsg {
    pub class: u8,
    pub primary: u8,
    pub secondary: u8,
    pub melee: u8,
    pub gadget: u8,
}

impl LoadoutMsg {
    /// First byte of the message.
    pub const TAG: u8 = 3;

    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        vec![
            Self::TAG,
            self.class,
            self.primary,
            self.secondary,
            self.melee,
            self.gadget,
        ]
    }

    /// Inverse of [`encode`](Self::encode).
    #[must_use]
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        match *bytes {
            [Self::TAG, class, primary, secondary, melee, gadget] => Some(Self {
                class,
                primary,
                secondary,
                melee,
                gadget,
            }),
            _ => None,
        }
    }

    /// Names the indices (`classes` and weapon ids in the order the server
    /// uses); `None` if one is out of range.
    #[must_use]
    pub fn to_loadout(&self, classes: &[ClassDef], weapons: &[String]) -> Option<Loadout> {
        let weapon = |i: u8| weapons.get(usize::from(i)).cloned();
        let opt = |i: u8| match i {
            NONE_BYTE => Some(None),
            i => weapon(i).map(Some),
        };
        Some(Loadout {
            class: classes.get(usize::from(self.class))?.id.clone(),
            primary: weapon(self.primary)?,
            secondary: opt(self.secondary)?,
            melee: opt(self.melee)?,
            gadget: opt(self.gadget)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loadout(class: &str, primary: &str) -> Loadout {
        Loadout {
            class: class.into(),
            primary: primary.into(),
            secondary: None,
            melee: None,
            gadget: None,
        }
    }

    #[test]
    fn every_base_class_validates() {
        for class in Catalog::builtin().classes() {
            let l = Loadout::for_class(class);
            assert_eq!(validate_loadout(&l), Ok(()), "{}", class.id);
        }
        assert_eq!(Loadout::trooper().class, "trooper");
    }

    #[test]
    fn builtin_rules_reference_real_weapons() {
        let rules = LoadoutRules::builtin();
        let catalog = Catalog::builtin();
        for w in rules
            .primaries
            .iter()
            .chain(&rules.secondaries)
            .chain(&rules.melee)
            .chain(&rules.gadgets)
            .chain(rules.costs.keys())
        {
            assert!(catalog.weapon(w).is_some(), "{w}");
            assert!(rules.costs.contains_key(w), "{w} has no cost");
        }
    }

    #[test]
    fn over_budget_is_refused() {
        let mut l = loadout("prankster", "precision_rifle");
        l.secondary = Some("sidearm".into());
        l.melee = Some("baguette".into());
        l.gadget = Some("toaster".into());
        assert_eq!(l.budget_cost(), 11);
        assert_eq!(
            validate_loadout(&l),
            Err(LoadoutError::OverBudget {
                cost: 11,
                budget: 10
            })
        );
        l.gadget = Some("bubble_gun".into());
        assert_eq!(validate_loadout(&l), Ok(()));
    }

    #[test]
    fn slot_and_class_rules() {
        // Any class may take any primary.
        assert_eq!(validate_loadout(&loadout("trooper", "smg")), Ok(()));
        assert!(matches!(
            validate_loadout(&loadout("nobody", "smg")),
            Err(LoadoutError::UnknownClass(_))
        ));
        assert!(matches!(
            validate_loadout(&loadout("trooper", "ghost")),
            Err(LoadoutError::UnknownWeapon(_))
        ));
        assert!(matches!(
            validate_loadout(&loadout("trooper", "sidearm")),
            Err(LoadoutError::WrongSlot { .. })
        ));
        let mut l = loadout("trooper", "smg");
        l.gadget = Some("toaster".into());
        assert!(matches!(
            validate_loadout(&l),
            Err(LoadoutError::NotAllowed { .. })
        ));
        l.gadget = None;
        l.melee = Some("baguette".into());
        assert_eq!(validate_loadout(&l), Ok(()));
        let mut m = loadout("marksman", "precision_rifle");
        m.melee = Some("baguette".into());
        assert!(matches!(
            validate_loadout(&m),
            Err(LoadoutError::NotAllowed { .. })
        ));
    }

    #[test]
    fn message_round_trips_and_names_indices() {
        let catalog = Catalog::builtin();
        let mut l = loadout("prankster", "smg");
        l.secondary = Some("sidearm".into());
        l.gadget = Some("banana_peel".into());
        let msg = l.to_msg(&catalog).unwrap();
        assert_eq!(LoadoutMsg::decode(&msg.encode()), Some(msg));
        assert_eq!(msg.melee, NONE_BYTE);
        let ids: Vec<String> = catalog.weapons().map(|w| w.id.clone()).collect();
        assert_eq!(msg.to_loadout(catalog.classes(), &ids), Some(l));
        assert_eq!(LoadoutMsg::decode(&[3, 0, 0]), None);
        let wild = LoadoutMsg {
            class: 0,
            primary: 200,
            secondary: 255,
            melee: 255,
            gadget: 255,
        };
        assert_eq!(wild.to_loadout(catalog.classes(), &ids), None);
        assert_eq!(loadout("ghost", "smg").to_msg(&catalog), None);
    }
}

//! Glue between the loadout screen (`faset-ui`, which knows no weapon data),
//! the class catalogue and budget of `faset-mode-fps`, and the
//! [`Profile`](crate::profile::Profile) (lot 6.3).

use faset_mode_fps::class::Catalog;
use faset_mode_fps::{Loadout, LoadoutMsg, LoadoutRules};
use faset_ui::{ClassRow, LoadoutChoices, LoadoutSelection, WeaponRow};

use crate::profile::{Appearance, SavedClass};

/// What the loadout screen offers: the catalogue's classes, the weapon pool
/// of each slot with its cost, and the budget.
#[must_use]
pub fn choices(catalog: &Catalog, rules: &LoadoutRules) -> LoadoutChoices {
    let rows = |ids: &[String]| -> Vec<WeaponRow> {
        ids.iter()
            .map(|id| WeaponRow {
                id: id.clone(),
                cost: rules.cost(id),
            })
            .collect()
    };
    LoadoutChoices {
        budget: rules.budget,
        classes: catalog
            .classes()
            .iter()
            .map(|c| ClassRow {
                id: c.id.clone(),
                health: c.health,
                speed: c.speed_factor,
                main_weapon: c.main_weapon.clone(),
                allowed: c.secondaries.clone(),
            })
            .collect(),
        primaries: rows(&rules.primaries),
        secondaries: rows(&rules.secondaries),
        melee: rows(&rules.melee),
        gadgets: rows(&rules.gadgets),
    }
}

/// The screen's selection for a saved class.
#[must_use]
pub fn selection_of(class: &SavedClass) -> LoadoutSelection {
    let l = &class.loadout;
    LoadoutSelection {
        class: l.class.clone(),
        primary: l.primary.clone(),
        secondary: l.secondary.clone(),
        melee: l.melee.clone(),
        gadget: l.gadget.clone(),
        body_colour: class.appearance.body,
        accent_colour: class.appearance.accent,
    }
}

/// The saved class for the screen's selection; `name` is kept from the class
/// it replaces.
#[must_use]
pub fn saved_class(selection: &LoadoutSelection, name: &str) -> SavedClass {
    SavedClass {
        name: name.to_owned(),
        loadout: Loadout {
            class: selection.class.clone(),
            primary: selection.primary.clone(),
            secondary: selection.secondary.clone(),
            melee: selection.melee.clone(),
            gadget: selection.gadget.clone(),
        },
        appearance: Appearance {
            body: selection.body_colour,
            accent: selection.accent_colour,
        },
    }
}

/// The message that sends `loadout` to the server, `None` if it names
/// something the catalogue does not have.
#[must_use]
pub fn message(catalog: &Catalog, loadout: &Loadout) -> Option<LoadoutMsg> {
    loadout.to_msg(catalog)
}

#[cfg(test)]
mod tests {
    use super::*;
    use faset_mode_fps::validate_loadout;

    #[test]
    fn choices_cover_the_catalogue_and_every_class_default_fits_the_budget() {
        let catalog = Catalog::builtin();
        let rules = LoadoutRules::builtin();
        let c = choices(&catalog, rules);
        assert_eq!(c.classes.len(), catalog.classes().len());
        assert_eq!(c.budget, rules.budget);
        assert!(c.primaries.iter().all(|w| w.cost > 0));
        for class in catalog.classes() {
            assert_eq!(validate_loadout(&Loadout::for_class(class)), Ok(()));
        }
    }

    #[test]
    fn selection_and_saved_class_convert_both_ways() {
        let class = SavedClass {
            name: "Mine".into(),
            loadout: Loadout {
                class: "prankster".into(),
                primary: "smg".into(),
                secondary: Some("sidearm".into()),
                melee: None,
                gadget: Some("bubble_gun".into()),
            },
            appearance: Appearance { body: 3, accent: 5 },
        };
        assert_eq!(saved_class(&selection_of(&class), "Mine"), class);
        let msg = message(&Catalog::builtin(), &class.loadout).unwrap();
        assert_eq!(msg.melee, u8::MAX);
    }
}

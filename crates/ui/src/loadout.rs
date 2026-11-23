//! The loadout screen (lot 6.3): pick a base class, then a primary weapon, a
//! secondary, a melee weapon and a gadget among those the class allows, within
//! a point budget, and colour the character from a palette.
//!
//! The crate knows no weapon data: the application fills a
//! [`LoadoutChoices`] (classes, weapon pools and costs, budget) and the screen
//! edits a [`LoadoutSelection`] in place. [`LoadoutModel::preview_request`]
//! tells the application what to draw as the 3D preview behind the screen.
//! "Apply" is refused while the selection is over budget and otherwise
//! issues [`UiCommand::SaveLoadout`].

use crate::screens::{back, panel_at};
use crate::{text, Theme, UiCommand};

/// One colour of the palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaletteColour {
    pub name: &'static str,
    /// sRGB.
    pub rgb: [u8; 3],
}

/// The colours a character can wear (decision: eight, shared by the body and
/// the accent).
pub const PALETTE: [PaletteColour; 8] = [
    PaletteColour {
        name: "Slate",
        rgb: [96, 108, 128],
    },
    PaletteColour {
        name: "Crimson",
        rgb: [200, 52, 60],
    },
    PaletteColour {
        name: "Amber",
        rgb: [240, 168, 40],
    },
    PaletteColour {
        name: "Lime",
        rgb: [124, 200, 60],
    },
    PaletteColour {
        name: "Teal",
        rgb: [40, 170, 160],
    },
    PaletteColour {
        name: "Azure",
        rgb: [64, 120, 220],
    },
    PaletteColour {
        name: "Violet",
        rgb: [140, 84, 200],
    },
    PaletteColour {
        name: "Bone",
        rgb: [226, 220, 204],
    },
];

/// A base class on offer.
#[derive(Debug, Clone, PartialEq)]
pub struct ClassRow {
    pub id: String,
    pub health: f32,
    /// Run speed factor.
    pub speed: f32,
    /// The class's own primary weapon.
    pub main_weapon: String,
    /// Weapons the class allows besides its primary (secondary, melee and
    /// gadget slots).
    pub allowed: Vec<String>,
}

/// A weapon on offer in one slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WeaponRow {
    pub id: String,
    pub cost: u32,
}

/// What the screen offers, prepared by the application.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LoadoutChoices {
    /// Points a loadout may spend.
    pub budget: u32,
    pub classes: Vec<ClassRow>,
    pub primaries: Vec<WeaponRow>,
    pub secondaries: Vec<WeaponRow>,
    pub melee: Vec<WeaponRow>,
    pub gadgets: Vec<WeaponRow>,
}

impl LoadoutChoices {
    fn cost(&self, id: &str) -> u32 {
        self.primaries
            .iter()
            .chain(&self.secondaries)
            .chain(&self.melee)
            .chain(&self.gadgets)
            .find(|w| w.id == id)
            .map_or(0, |w| w.cost)
    }
}

/// What the player picked.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoadoutSelection {
    pub class: String,
    pub primary: String,
    pub secondary: Option<String>,
    pub melee: Option<String>,
    pub gadget: Option<String>,
    /// Index into [`PALETTE`].
    pub body_colour: u8,
    /// Index into [`PALETTE`].
    pub accent_colour: u8,
}

/// What the application draws as the 3D preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewRequest {
    pub selection: LoadoutSelection,
}

/// Data of the loadout screen.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LoadoutModel {
    pub choices: LoadoutChoices,
    pub selection: LoadoutSelection,
}

impl LoadoutModel {
    /// Points the selection spends.
    #[must_use]
    pub fn cost(&self) -> u32 {
        let s = &self.selection;
        std::iter::once(&s.primary)
            .chain(s.secondary.iter())
            .chain(s.melee.iter())
            .chain(s.gadget.iter())
            .map(|w| self.choices.cost(w))
            .sum()
    }

    /// Whether the selection spends more than the budget.
    #[must_use]
    pub fn over_budget(&self) -> bool {
        self.cost() > self.choices.budget
    }

    /// Whether the model has anything to show (the application has filled
    /// it).
    #[must_use]
    pub fn is_ready(&self) -> bool {
        !self.choices.classes.is_empty()
    }

    /// The preview to draw.
    #[must_use]
    pub fn preview_request(&self) -> PreviewRequest {
        PreviewRequest {
            selection: self.selection.clone(),
        }
    }

    /// Picks `class` and resets the weapons to its defaults (its main weapon
    /// and first allowed secondary, nothing else). Unknown ids are ignored.
    pub fn select_class(&mut self, class: &str) {
        let Some(row) = self.choices.classes.iter().find(|c| c.id == class) else {
            return;
        };
        let secondary = self
            .choices
            .secondaries
            .iter()
            .find(|w| row.allowed.contains(&w.id))
            .map(|w| w.id.clone());
        self.selection.class.clone_from(&row.id);
        self.selection.primary.clone_from(&row.main_weapon);
        self.selection.secondary = secondary;
        self.selection.melee = None;
        self.selection.gadget = None;
    }

    fn allowed(&self, weapon: &str) -> bool {
        self.choices
            .classes
            .iter()
            .find(|c| c.id == self.selection.class)
            .is_some_and(|c| c.allowed.iter().any(|a| a == weapon))
    }
}

/// Display name of an id: `rubber_chicken_launcher` is "Rubber chicken
/// launcher".
#[must_use]
pub fn display_name(id: &str) -> String {
    let mut name = id.replace('_', " ");
    if let Some(first) = name.get(..1) {
        let upper = first.to_uppercase();
        name.replace_range(..1, &upper);
    }
    name
}

fn colour32(rgb: [u8; 3]) -> egui::Color32 {
    egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2])
}

fn weapon_combo(
    ui: &mut egui::Ui,
    label: &str,
    options: &[&WeaponRow],
    current: &mut Option<String>,
    optional: bool,
) {
    ui.horizontal(|ui| {
        ui.label(label);
        let shown = current.as_deref().map_or_else(
            || text("loadout.none").to_owned(),
            |id| {
                let cost = options.iter().find(|w| w.id == id).map_or(0, |w| w.cost);
                format!("{} ({cost})", display_name(id))
            },
        );
        egui::ComboBox::from_id_salt(label)
            .selected_text(shown)
            .show_ui(ui, |ui| {
                if optional
                    && ui
                        .selectable_label(current.is_none(), text("loadout.none"))
                        .clicked()
                {
                    *current = None;
                }
                for w in options {
                    let name = format!("{} ({})", display_name(&w.id), w.cost);
                    let on = current.as_deref() == Some(w.id.as_str());
                    if ui.selectable_label(on, name).clicked() {
                        *current = Some(w.id.clone());
                    }
                }
            });
    });
}

fn palette_row(ui: &mut egui::Ui, theme: &Theme, label: &str, current: &mut u8) {
    ui.horizontal(|ui| {
        ui.label(label);
        for (i, colour) in PALETTE.iter().enumerate() {
            let chosen = usize::from(*current) == i;
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(24.0, 24.0), egui::Sense::click());
            ui.painter().rect_filled(rect, 3.0, colour32(colour.rgb));
            if chosen {
                ui.painter().rect_stroke(
                    rect.expand(2.0),
                    3.0,
                    egui::Stroke::new(2.0, theme.accent),
                    egui::StrokeKind::Outside,
                );
            }
            if response.on_hover_text(colour.name).clicked() {
                *current = u8::try_from(i).unwrap_or(0);
            }
        }
    });
}

/// The loadout screen. It sits on the left, leaving the middle of the
/// display to the 3D preview the application draws.
pub fn loadout_screen(
    ctx: &egui::Context,
    theme: &Theme,
    model: &mut LoadoutModel,
    cmds: &mut Vec<UiCommand>,
) {
    panel_at(
        ctx,
        theme,
        text("loadout.title"),
        egui::Align2::LEFT_CENTER,
        |ui| {
            if !model.is_ready() {
                ui.colored_label(theme.text_muted, text("loadout.unavailable"));
                back(ui, cmds);
                return;
            }
            ui.label(text("loadout.class"));
            ui.horizontal_wrapped(|ui| {
                let ids: Vec<String> = model.choices.classes.iter().map(|c| c.id.clone()).collect();
                for id in ids {
                    let on = model.selection.class == id;
                    if ui.selectable_label(on, display_name(&id)).clicked() && !on {
                        model.select_class(&id);
                    }
                }
            });
            if let Some(c) = model
                .choices
                .classes
                .iter()
                .find(|c| c.id == model.selection.class)
            {
                ui.colored_label(
                    theme.text_muted,
                    format!(
                        "{} {:.0}   {} {:.2}",
                        text("loadout.health"),
                        c.health,
                        text("loadout.speed"),
                        c.speed
                    ),
                );
            }
            ui.add_space(8.0);

            let primaries: Vec<&WeaponRow> = model.choices.primaries.iter().collect();
            let mut primary = Some(model.selection.primary.clone());
            weapon_combo(ui, text("loadout.primary"), &primaries, &mut primary, false);
            if let Some(p) = primary {
                model.selection.primary = p;
            }
            let allowed = |rows: &[WeaponRow], model: &LoadoutModel| {
                rows.iter()
                    .filter(|w| model.allowed(&w.id))
                    .cloned()
                    .collect::<Vec<_>>()
            };
            let secondaries = allowed(&model.choices.secondaries, model);
            let melee = allowed(&model.choices.melee, model);
            let gadgets = allowed(&model.choices.gadgets, model);
            let slots = [
                ("loadout.secondary", &secondaries),
                ("loadout.melee", &melee),
                ("loadout.gadget", &gadgets),
            ];
            let mut picks = [
                model.selection.secondary.clone(),
                model.selection.melee.clone(),
                model.selection.gadget.clone(),
            ];
            for ((key, rows), pick) in slots.into_iter().zip(&mut picks) {
                let rows: Vec<&WeaponRow> = rows.iter().collect();
                weapon_combo(ui, text(key), &rows, pick, true);
            }
            let [secondary, melee, gadget] = picks;
            model.selection.secondary = secondary;
            model.selection.melee = melee;
            model.selection.gadget = gadget;
            ui.add_space(8.0);

            let (cost, budget) = (model.cost(), model.choices.budget);
            let over = cost > budget;
            let fraction = if budget == 0 {
                1.0
            } else {
                (cost as f32 / budget as f32).min(1.0)
            };
            ui.label(text("loadout.budget"));
            let mut bar = egui::ProgressBar::new(fraction)
                .text(format!("{cost} / {budget}"))
                .fill(if over { theme.danger } else { theme.accent });
            bar = bar.desired_width(ui.available_width());
            ui.add(bar);
            if over {
                ui.colored_label(theme.danger, text("loadout.over_budget"));
            }
            ui.add_space(8.0);

            palette_row(
                ui,
                theme,
                text("loadout.body"),
                &mut model.selection.body_colour,
            );
            palette_row(
                ui,
                theme,
                text("loadout.accent"),
                &mut model.selection.accent_colour,
            );
            ui.add_space(8.0);

            let apply = ui.add_enabled(
                !over,
                egui::Button::new(text("loadout.apply"))
                    .min_size(egui::vec2(ui.available_width(), 36.0)),
            );
            if apply.clicked() {
                cmds.push(UiCommand::SaveLoadout(model.selection.clone()));
            }
            back(ui, cmds);
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(items: &[(&str, u32)]) -> Vec<WeaponRow> {
        items
            .iter()
            .map(|(id, cost)| WeaponRow {
                id: (*id).into(),
                cost: *cost,
            })
            .collect()
    }

    fn model() -> LoadoutModel {
        let choices = LoadoutChoices {
            budget: 10,
            classes: vec![
                ClassRow {
                    id: "trooper".into(),
                    health: 100.0,
                    speed: 1.0,
                    main_weapon: "assault_rifle".into(),
                    allowed: vec!["sidearm".into(), "baguette".into()],
                },
                ClassRow {
                    id: "marksman".into(),
                    health: 80.0,
                    speed: 1.0,
                    main_weapon: "precision_rifle".into(),
                    allowed: vec!["sidearm".into()],
                },
            ],
            primaries: rows(&[("assault_rifle", 4), ("precision_rifle", 6)]),
            secondaries: rows(&[("sidearm", 1)]),
            melee: rows(&[("baguette", 1)]),
            gadgets: rows(&[("toaster", 3)]),
        };
        let mut m = LoadoutModel {
            choices,
            selection: LoadoutSelection::default(),
        };
        m.select_class("trooper");
        m
    }

    #[test]
    fn selecting_a_class_resets_to_its_defaults_and_costs_add_up() {
        let mut m = model();
        assert_eq!(m.selection.primary, "assault_rifle");
        assert_eq!(m.selection.secondary.as_deref(), Some("sidearm"));
        assert_eq!(m.cost(), 5);
        assert!(!m.over_budget());
        m.selection.primary = "precision_rifle".into();
        m.selection.melee = Some("baguette".into());
        m.selection.gadget = Some("toaster".into());
        assert_eq!(m.cost(), 11);
        assert!(m.over_budget());
        m.select_class("marksman");
        assert_eq!(m.selection.melee, None);
        assert_eq!(m.cost(), 7);
        m.select_class("ghost");
        assert_eq!(m.selection.class, "marksman");
    }

    #[test]
    fn preview_request_mirrors_the_selection_and_names_read_well() {
        let m = model();
        assert_eq!(m.preview_request().selection, m.selection);
        assert_eq!(
            display_name("rubber_chicken_launcher"),
            "Rubber chicken launcher"
        );
        assert!(!LoadoutModel::default().is_ready());
    }

    #[test]
    fn screen_draws_ready_and_empty_models_without_commands() {
        let ctx = egui::Context::default();
        let theme = Theme::default();
        let mut cmds = Vec::new();
        for mut m in [model(), LoadoutModel::default()] {
            ctx.begin_pass(egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 720.0),
                )),
                ..egui::RawInput::default()
            });
            loadout_screen(&ctx, &theme, &mut m, &mut cmds);
            let mut output = ctx.end_pass();
            output.textures_delta.clear();
        }
        assert!(cmds.is_empty());
    }
}

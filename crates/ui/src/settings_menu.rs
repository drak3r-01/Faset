//! The settings menu, generated from the metadata of `faset_core::settings`.
//!
//! [`build_settings_menu`] turns a [`Settings`] into a [`Menu`] (categories of
//! entries, one per metadata row); [`SettingsScreen`] draws it with egui and
//! reports what the player changed as [`SettingsEvent`]s. Labels are
//! translation keys of the shared table (`faset_core::translations`).

use faset_core::settings::{
    Category, ConfirmState, GameSettings, ReticleStyle, SettingMeta, SettingType, Settings,
    UpscalerChoice, Value, MAX_PSEUDONYM_CHARS, META,
};

use crate::i18n::{fill, set_language, text, try_text};
use crate::rebinding::RebindView;
use crate::{Screen, Theme};

/// One row of the menu: a setting and its current value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MenuEntry {
    /// The setting's metadata.
    pub meta: &'static SettingMeta,
    /// Its value now.
    pub value: Value,
}

/// A tab of the menu.
#[derive(Debug, Clone, PartialEq)]
pub struct MenuCategory {
    /// The category.
    pub category: Category,
    /// Its entries, in metadata order.
    pub entries: Vec<MenuEntry>,
}

/// The generated settings menu.
#[derive(Debug, Clone, PartialEq)]
pub struct Menu {
    /// Categories in menu order; empty ones are left out.
    pub categories: Vec<MenuCategory>,
}

impl Menu {
    /// The entry of setting `id`.
    pub fn entry(&self, id: &str) -> Option<&MenuEntry> {
        self.categories
            .iter()
            .flat_map(|c| &c.entries)
            .find(|e| e.meta.id == id)
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.categories.iter().map(|c| c.entries.len()).sum()
    }

    /// True without entries.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Builds the menu from the metadata: one entry per setting, grouped by
/// category, holding the value `settings` has now.
pub fn build_settings_menu(settings: &Settings) -> Menu {
    let categories = Category::ALL
        .iter()
        .map(|&category| MenuCategory {
            category,
            entries: META
                .iter()
                .filter(|m| m.category == category)
                .map(|m| MenuEntry {
                    meta: m,
                    value: settings.get(m.id).unwrap_or(m.default),
                })
                .collect(),
        })
        .filter(|c| !c.entries.is_empty())
        .collect();
    Menu { categories }
}

/// Text of a label key in the current language (a missing key shows the
/// key).
pub fn label(key: &str) -> &str {
    text(key)
}

/// Display text of an option name (`ultra_performance` becomes
/// `Ultra performance`); the fallback when the table has no text.
pub fn option_label(option: &str) -> String {
    let mut text = option.replace('_', " ");
    if let Some(first) = text.get_mut(..1) {
        first.make_ascii_uppercase();
    }
    text
}

/// Display text of `option` of setting `id`: the table entry
/// `option.<id>.<option>` when it exists, else [`option_label`].
pub fn option_text(id: &str, option: &str) -> String {
    try_text(&format!("option.{id}.{option}")).map_or_else(|| option_label(option), str::to_owned)
}

/// What the settings screen asks of its host.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SettingsEvent {
    /// The player changed this setting; apply it.
    Changed(&'static str),
    /// The player kept the new resolution.
    KeepResolution,
    /// The player asked to go back to the previous resolution.
    RevertResolution,
    /// The player changed a key binding.
    BindingsChanged,
    /// The player edited the pseudonym.
    PseudonymChanged,
    /// The player left the screen.
    Back,
}

/// The settings screen: edits `settings` in place and collects events.
pub struct SettingsScreen<'a> {
    /// Settings being edited.
    pub settings: &'a mut Settings,
    /// Upscalers this GPU can run; others are not offered.
    pub available_upscalers: &'a [UpscalerChoice],
    /// Pending resolution confirmation, if any.
    pub confirm: Option<ConfirmState>,
    /// Selected category tab.
    pub tab: usize,
    /// State of the rebinding view (kept by the host between frames).
    pub rebind: RebindView,
    /// Smoothed network jitter in seconds, to show the automatic
    /// interpolation delay (0 when unknown).
    pub jitter: f64,
    /// Events of this frame.
    pub events: Vec<SettingsEvent>,
}

impl<'a> SettingsScreen<'a> {
    /// A screen over `settings`.
    pub fn new(settings: &'a mut Settings, available_upscalers: &'a [UpscalerChoice]) -> Self {
        Self {
            settings,
            available_upscalers,
            confirm: None,
            tab: 0,
            rebind: RebindView::default(),
            jitter: 0.0,
            events: Vec::new(),
        }
    }

    fn offered(&self, meta: &SettingMeta, index: usize) -> bool {
        meta.id != "upscaler"
            || UpscalerChoice::from_index(index).is_some_and(|u| {
                self.available_upscalers.is_empty() || self.available_upscalers.contains(&u)
            })
    }

    fn row(&mut self, ui: &mut egui::Ui, meta: &'static SettingMeta) {
        let current = self.settings.get(meta.id).unwrap_or(meta.default);
        let free_scale = meta.id != "render_scale"
            || (matches!(
                self.settings.quality.upscaler,
                UpscalerChoice::Native | UpscalerChoice::Fsr1
            ) && self.settings.quality.upscale_level.factor().is_none());
        let mut new = current;
        ui.add_enabled_ui(free_scale, |ui| {
            ui.horizontal(|ui| {
                ui.label(text(meta.label_key));
                match (meta.ty, &mut new) {
                    (SettingType::Bool, Value::Bool(v)) => {
                        ui.checkbox(v, "");
                    }
                    (SettingType::Int { min, max }, Value::Int(v)) => {
                        ui.add(egui::Slider::new(v, min..=max));
                    }
                    (SettingType::Float { min, max }, Value::Float(v)) => {
                        ui.add(egui::Slider::new(v, min..=max));
                    }
                    (SettingType::Choice(options), Value::Choice(i)) => {
                        let shown = option_text(meta.id, options[(*i).min(options.len() - 1)]);
                        egui::ComboBox::from_id_salt(meta.id)
                            .selected_text(shown)
                            .show_ui(ui, |ui| {
                                for (n, name) in options.iter().enumerate() {
                                    if self.offered(meta, n) {
                                        ui.selectable_value(i, n, option_text(meta.id, name));
                                    }
                                }
                            });
                    }
                    _ => {}
                }
            });
        });
        if new != current && self.settings.set(meta.id, new).is_ok() {
            self.events.push(SettingsEvent::Changed(meta.id));
        }
    }
}

impl SettingsScreen<'_> {
    fn pseudonym_row(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(text("settings.pseudonym"));
            let mut name = self.settings.network.pseudonym.clone();
            let edit =
                ui.add(egui::TextEdit::singleline(&mut name).char_limit(MAX_PSEUDONYM_CHARS));
            if edit.changed() {
                self.settings.network.pseudonym = name;
                self.events.push(SettingsEvent::PseudonymChanged);
            }
            if edit.lost_focus() {
                self.settings.clean_pseudonym();
            }
        });
    }

    /// Extra information under some rows.
    fn after_row(&self, ui: &mut egui::Ui, id: &str) {
        let note = match id {
            "mouse_dpi" => Some(fill(
                text("settings.cm_per_360"),
                &[("value", &format!("{:.1}", self.settings.cm_per_360()))],
            )),
            "interpolation_delay_ms" => Some(fill(
                text("settings.interpolation_effective"),
                &[(
                    "value",
                    &format!(
                        "{:.0}",
                        self.settings.interpolation_delay(self.jitter) * 1000.0
                    ),
                )],
            )),
            _ => None,
        };
        if let Some(note) = note {
            ui.weak(note);
        }
    }
}

/// Draws the reticle as set in `game` on a dark swatch.
fn reticle_preview(ui: &mut egui::Ui, game: &GameSettings) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(96.0, 96.0), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, egui::Color32::from_gray(30));
    let alpha = (game.reticle_opacity.clamp(0.0, 1.0) * 255.0) as u8;
    let colour = egui::Color32::from_white_alpha(alpha);
    let c = rect.center();
    let (size, gap) = (game.reticle_size as f32 / 2.0, game.reticle_gap as f32);
    let stroke = egui::Stroke::new(game.reticle_thickness as f32, colour);
    let dot = |p: &egui::Painter| p.circle_filled(c, game.reticle_thickness as f32, colour);
    let cross = |p: &egui::Painter| {
        for d in [egui::vec2(1.0, 0.0), egui::vec2(0.0, 1.0)] {
            p.line_segment([c + d * gap, c + d * (gap + size)], stroke);
            p.line_segment([c - d * gap, c - d * (gap + size)], stroke);
        }
    };
    match game.reticle_style {
        ReticleStyle::Cross => cross(&painter),
        ReticleStyle::Dot => {
            dot(&painter);
        }
        ReticleStyle::Circle => {
            painter.circle_stroke(c, (gap + size).min(46.0), stroke);
        }
        ReticleStyle::CrossDot => {
            cross(&painter);
            dot(&painter);
        }
    }
}

impl Screen for SettingsScreen<'_> {
    fn show(&mut self, ctx: &egui::Context, theme: &Theme) {
        // The language of the setting is the language of this very frame.
        set_language(self.settings.game.language);
        let menu = build_settings_menu(self.settings);
        egui::Window::new("settings")
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .collapsible(false)
            .resizable(false)
            .title_bar(false)
            .frame(
                egui::Frame::new()
                    .fill(theme.background)
                    .stroke(egui::Stroke::new(1.0, theme.border))
                    .corner_radius(f32::from(theme.radius))
                    .inner_margin(24.0),
            )
            .show(ctx, |ui| {
                ui.set_min_width(480.0);
                ui.horizontal(|ui| {
                    for (n, c) in menu.categories.iter().enumerate() {
                        ui.selectable_value(&mut self.tab, n, text(c.category.label_key()));
                    }
                });
                ui.separator();
                if let Some(category) = menu.categories.get(self.tab) {
                    egui::ScrollArea::vertical()
                        .max_height(420.0)
                        .show(ui, |ui| {
                            if category.category == Category::Network {
                                self.pseudonym_row(ui);
                            }
                            for entry in &category.entries {
                                self.row(ui, entry.meta);
                                self.after_row(ui, entry.meta.id);
                            }
                            match category.category {
                                Category::Controls => {
                                    ui.separator();
                                    if self.rebind.show(ui, &mut self.settings.bindings) {
                                        self.events.push(SettingsEvent::BindingsChanged);
                                    }
                                }
                                Category::Game => {
                                    ui.separator();
                                    ui.label(text("settings.reticle_preview"));
                                    reticle_preview(ui, &self.settings.game);
                                }
                                _ => {}
                            }
                        });
                }
                if let Some(state) = self.confirm {
                    ui.separator();
                    let left = match state {
                        ConfirmState::Waiting(s) => s.ceil() as u32,
                        ConfirmState::Expired => 0,
                    };
                    ui.colored_label(
                        theme.text_muted,
                        fill(
                            text("settings.confirm_resolution"),
                            &[("seconds", &left.to_string())],
                        ),
                    );
                    ui.horizontal(|ui| {
                        if ui.button(text("settings.keep")).clicked() {
                            self.events.push(SettingsEvent::KeepResolution);
                        }
                        if ui.button(text("settings.revert")).clicked() {
                            self.events.push(SettingsEvent::RevertResolution);
                        }
                    });
                }
                ui.add_space(8.0);
                if ui
                    .add_sized(
                        [ui.available_width(), 36.0],
                        egui::Button::new(text("nav.back")),
                    )
                    .clicked()
                {
                    self.events.push(SettingsEvent::Back);
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_setting_appears_in_the_menu() {
        let settings = Settings::default();
        let menu = build_settings_menu(&settings);
        assert_eq!(menu.len(), META.len());
        for m in META {
            let entry = menu
                .entry(m.id)
                .unwrap_or_else(|| panic!("{} missing", m.id));
            assert_eq!(entry.value, m.default);
            assert_ne!(
                label(m.label_key),
                m.label_key,
                "no text for {}",
                m.label_key
            );
        }
        for c in &menu.categories {
            assert_ne!(label(c.category.label_key()), c.category.label_key());
        }
    }

    #[test]
    fn menu_shows_current_values() {
        let mut settings = Settings::default();
        settings.set("fov", Value::Int(100)).unwrap();
        let menu = build_settings_menu(&settings);
        assert_eq!(menu.entry("fov").unwrap().value, Value::Int(100));
    }

    #[test]
    fn screen_runs_headless() {
        let ctx = egui::Context::default();
        let mut settings = Settings::default();
        let mut screen = SettingsScreen::new(&mut settings, &[]);
        screen.confirm = Some(ConfirmState::Waiting(9.2));
        ctx.begin_pass(egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 720.0),
            )),
            ..egui::RawInput::default()
        });
        screen.show(&ctx, &Theme::default());
        let mut output = ctx.end_pass();
        output.textures_delta.clear();
        assert!(screen.events.is_empty());
    }

    #[test]
    fn option_labels_are_readable() {
        assert_eq!(option_label("ultra_performance"), "Ultra performance");
        assert_eq!(option_label("1280x720"), "1280x720");
        assert_eq!(option_text("window_mode", "windowed"), "Windowed");
        assert_eq!(option_text("resolution", "1280x720"), "1280x720");
    }
}

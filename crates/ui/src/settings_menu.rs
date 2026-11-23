//! The settings menu, generated from the metadata of `faset_core::settings`.
//!
//! [`build_settings_menu`] turns a [`Settings`] into a [`Menu`] (categories of
//! entries, one per metadata row); [`SettingsScreen`] draws it with egui and
//! reports what the player changed as [`SettingsEvent`]s. Labels are
//! translation keys: until lot 6.5 moves them to the shared table, the English
//! texts live in [`label`].

use faset_core::settings::{
    Category, ConfirmState, SettingMeta, SettingType, Settings, UpscalerChoice, Value, META,
};

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

/// English text of a label key (a missing key shows the key).
pub fn label(key: &str) -> &str {
    const EN: &[(&str, &str)] = &[
        ("settings.anti_aliasing", "Anti-aliasing"),
        ("settings.bloom", "Bloom"),
        ("settings.brightness", "Brightness"),
        ("settings.category.display", "Display"),
        ("settings.category.quality", "Quality"),
        ("settings.draw_distance", "Draw distance (m)"),
        ("settings.fov", "Field of view"),
        ("settings.fps_cap", "FPS cap (0 = unlimited)"),
        ("settings.monitor", "Monitor"),
        ("settings.occlusion", "Ambient occlusion"),
        ("settings.outlines", "Outlines"),
        ("settings.render_scale", "Render scale (%)"),
        ("settings.resolution", "Resolution"),
        ("settings.screen_effects", "Screen effects"),
        ("settings.shadows", "Shadows"),
        ("settings.sharpness", "Sharpness"),
        ("settings.upscale_level", "Upscale level"),
        ("settings.upscaler", "Upscaler"),
        ("settings.vsync", "Vertical sync"),
        ("settings.weather_density", "Weather density"),
        ("settings.window_mode", "Window mode"),
    ];
    EN.iter().find(|(k, _)| *k == key).map_or(key, |(_, v)| v)
}

/// Display text of an option name (`ultra_performance` becomes
/// `Ultra performance`).
pub fn option_label(option: &str) -> String {
    let mut text = option.replace('_', " ");
    if let Some(first) = text.get_mut(..1) {
        first.make_ascii_uppercase();
    }
    text
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
                ui.label(label(meta.label_key));
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
                        let shown = option_label(options[(*i).min(options.len() - 1)]);
                        egui::ComboBox::from_id_salt(meta.id)
                            .selected_text(shown)
                            .show_ui(ui, |ui| {
                                for (n, name) in options.iter().enumerate() {
                                    if self.offered(meta, n) {
                                        ui.selectable_value(i, n, option_label(name));
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

impl Screen for SettingsScreen<'_> {
    fn show(&mut self, ctx: &egui::Context, theme: &Theme) {
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
                        ui.selectable_value(&mut self.tab, n, label(c.category.label_key()));
                    }
                });
                ui.separator();
                if let Some(category) = menu.categories.get(self.tab) {
                    for entry in &category.entries {
                        self.row(ui, entry.meta);
                    }
                }
                if let Some(state) = self.confirm {
                    ui.separator();
                    let left = match state {
                        ConfirmState::Waiting(s) => s.ceil() as u32,
                        ConfirmState::Expired => 0,
                    };
                    ui.colored_label(
                        theme.text_muted,
                        format!("Keep this resolution? Reverting in {left} s"),
                    );
                    ui.horizontal(|ui| {
                        if ui.button("Keep").clicked() {
                            self.events.push(SettingsEvent::KeepResolution);
                        }
                        if ui.button("Revert").clicked() {
                            self.events.push(SettingsEvent::RevertResolution);
                        }
                    });
                }
                ui.add_space(8.0);
                if ui
                    .add_sized([ui.available_width(), 36.0], egui::Button::new("Back"))
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
    }
}

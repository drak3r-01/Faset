//! The main screens: one function per application state.
//!
//! Screens never touch the world or the state stack. They read a
//! [`MenuModel`] (data the application prepares) and append [`UiCommand`]s the
//! client applies to its `StateStack`. Nothing here is specific to one
//! mini-game: the play screen is generated from the registry's descriptors.

use faset_minigame_api::{
    ModeDescriptor, ModeEntry, Registry, SettingKind, SettingSpec, SettingValue, Settings,
};

use crate::loadout::{loadout_screen, LoadoutModel, LoadoutSelection};
use crate::{text, Screen, Theme};

/// Application states. The stack of states lives in `faset-client`; the enum
/// is defined here so screens can name the state a command targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AppState {
    /// Start-up: assets and settings load, then the main menu opens.
    Boot,
    /// The home screen.
    MainMenu,
    /// Join screen: LAN servers, address, favourites and history.
    ServerBrowser,
    /// Play screen: mini-game, map, bots and server options.
    HostSetup,
    /// Class creation and customisation.
    Loadout,
    /// Settings; also pushed on top of the match.
    Settings,
    /// Waiting for a remote server to accept the connection.
    Connecting,
    /// A map is loading.
    Loading,
    /// A match runs.
    InGame,
    /// The map editor.
    Editor,
    /// The in-game pause menu, pushed on top of [`AppState::InGame`]
    /// (decision: a state of its own, so the stack shows it).
    Pause,
}

impl AppState {
    /// Every state, in declaration order.
    pub const ALL: [Self; 11] = [
        Self::Boot,
        Self::MainMenu,
        Self::ServerBrowser,
        Self::HostSetup,
        Self::Loadout,
        Self::Settings,
        Self::Connecting,
        Self::Loading,
        Self::InGame,
        Self::Editor,
        Self::Pause,
    ];
}

/// What a screen asks the application to do.
#[derive(Debug, Clone, PartialEq)]
pub enum UiCommand {
    /// Push a state on the stack.
    Push(AppState),
    /// Pop the top state.
    Pop,
    /// Replace the top state.
    Replace(AppState),
    /// Close the application.
    Quit,
    /// Start a match from the play screen.
    StartMatch(PlayRequest),
    /// Connect to the server at this address.
    Connect(String),
    /// Add the address to the favourites, or remove it (lot 6.2).
    ToggleFavourite(String),
    /// Leave the running match and return to the main menu.
    LeaveMatch,
    /// Keep the loadout built on the loadout screen (lot 6.3).
    SaveLoadout(LoadoutSelection),
}

/// A mini-game offered by the play screen, copied from the registry.
#[derive(Debug, Clone)]
pub struct ModeChoice {
    /// Descriptor key.
    pub id: &'static str,
    /// Name shown to players.
    pub name: &'static str,
    /// Whether bots can play it.
    pub bots: bool,
    /// Settings schema, drawn as the mode settings.
    pub settings: Vec<SettingSpec>,
}

impl ModeChoice {
    fn from_descriptor(d: &ModeDescriptor) -> Self {
        Self {
            id: d.id,
            name: d.name,
            bots: d.bots,
            settings: d.settings.clone(),
        }
    }

    /// One choice per registered mode, in registration order, and nothing else.
    #[must_use]
    pub fn from_registry(registry: &Registry) -> Vec<Self> {
        registry
            .iter()
            .map(|e: &ModeEntry| Self::from_descriptor(&e.descriptor))
            .collect()
    }
}

/// The play screen's form, and what [`UiCommand::StartMatch`] carries.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayRequest {
    /// Descriptor key of the chosen mode (empty: no mode).
    pub mode: String,
    /// Index of the chosen map in [`MenuModel::maps`].
    pub map: usize,
    /// Play every map in turn instead of only `map`.
    pub rotation: bool,
    /// Number of bots.
    pub bots: u8,
    /// Bot level, 0 (easy) to 4 (hard).
    pub bot_level: u8,
    /// Maximum players, bots included.
    pub max_players: u8,
    /// Open the game to the local network.
    pub lan: bool,
    /// Join password (empty: none).
    pub password: String,
    /// Mode settings, by key.
    pub settings: Settings,
}

impl Default for PlayRequest {
    fn default() -> Self {
        Self {
            mode: String::new(),
            map: 0,
            rotation: false,
            bots: 0,
            bot_level: 2,
            max_players: 16,
            lan: false,
            password: String::new(),
            settings: Settings::new(),
        }
    }
}

/// A server found on the local network, as the join screen lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerRow {
    /// Server name.
    pub name: String,
    /// Address to connect to.
    pub address: String,
    /// Players present and capacity.
    pub players: (u16, u16),
    /// Map in play.
    pub map: String,
}

/// Data the screens read and edit.
#[derive(Debug, Clone, Default)]
pub struct MenuModel {
    /// Mini-games of the registry.
    pub modes: Vec<ModeChoice>,
    /// Map names.
    pub maps: Vec<String>,
    /// The play screen's form.
    pub form: PlayRequest,
    /// Servers seen on the LAN (filled by the application).
    pub lan_servers: Vec<ServerRow>,
    /// Favourite server addresses (content: lot 6.2).
    pub favourites: Vec<String>,
    /// Recent server addresses (content: lot 6.2).
    pub history: Vec<String>,
    /// Address field of the join screen.
    pub address: String,
    /// Loading progress in `0..=1`.
    pub loading: f32,
    /// Name of the map being loaded.
    pub loading_map: String,
    /// The loadout screen's data (content: lot 6.3).
    pub loadout: LoadoutModel,
}

impl MenuModel {
    /// A model listing the modes of `registry` and the given maps; the form
    /// starts on the first mode with its default settings.
    #[must_use]
    pub fn new(registry: &Registry, maps: Vec<String>) -> Self {
        let modes = ModeChoice::from_registry(registry);
        let mut model = Self {
            modes,
            maps,
            ..Self::default()
        };
        model.select_mode(0);
        model
    }

    /// Selects mode number `index` and resets the form's settings to its
    /// defaults.
    pub fn select_mode(&mut self, index: usize) {
        let Some(mode) = self.modes.get(index) else {
            self.form.mode.clear();
            self.form.settings = Settings::new();
            return;
        };
        self.form.mode = mode.id.to_owned();
        let mut settings = Settings::new();
        for spec in &mode.settings {
            settings.set(spec.key, spec.kind.default_value());
        }
        self.form.settings = settings;
        if !mode.bots {
            self.form.bots = 0;
        }
    }

    fn mode_index(&self) -> Option<usize> {
        self.modes.iter().position(|m| m.id == self.form.mode)
    }
}

pub(crate) fn panel(
    ctx: &egui::Context,
    theme: &Theme,
    title: &str,
    add: impl FnOnce(&mut egui::Ui),
) {
    panel_at(ctx, theme, title, egui::Align2::CENTER_CENTER, add);
}

/// [`panel`] anchored elsewhere (the loadout screen leaves the middle to the
/// 3D preview).
pub(crate) fn panel_at(
    ctx: &egui::Context,
    theme: &Theme,
    title: &str,
    anchor: egui::Align2,
    add: impl FnOnce(&mut egui::Ui),
) {
    egui::Window::new(title)
        .anchor(anchor, egui::vec2(24.0 * anchor_sign(anchor), 0.0))
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
            ui.set_min_width(360.0);
            ui.heading(title);
            ui.add_space(12.0);
            add(ui);
        });
}

/// Margin direction of an anchor: right of the left edge, left of the right
/// edge, none when centred.
fn anchor_sign(anchor: egui::Align2) -> f32 {
    match anchor.x() {
        egui::Align::Min => 1.0,
        egui::Align::Center => 0.0,
        egui::Align::Max => -1.0,
    }
}

fn wide_button(ui: &mut egui::Ui, key: &str) -> bool {
    ui.add_sized([ui.available_width(), 36.0], egui::Button::new(text(key)))
        .clicked()
}

pub(crate) fn back(ui: &mut egui::Ui, cmds: &mut Vec<UiCommand>) {
    ui.add_space(8.0);
    if wide_button(ui, "nav.back") {
        cmds.push(UiCommand::Pop);
    }
}

/// Start-up screen.
pub fn boot(ctx: &egui::Context, theme: &Theme, _cmds: &mut Vec<UiCommand>) {
    panel(ctx, theme, text("boot.title"), |ui| {
        ui.spinner();
    });
}

/// The home screen, over the (application-drawn) rotating map background.
pub fn main_menu(ctx: &egui::Context, theme: &Theme, cmds: &mut Vec<UiCommand>) {
    panel(ctx, theme, text("menu.title"), |ui| {
        let entries = [
            ("menu.play", UiCommand::Push(AppState::HostSetup)),
            ("menu.join", UiCommand::Push(AppState::ServerBrowser)),
            ("menu.classes", UiCommand::Push(AppState::Loadout)),
            ("menu.editor", UiCommand::Push(AppState::Editor)),
            ("menu.settings", UiCommand::Push(AppState::Settings)),
            ("menu.quit", UiCommand::Quit),
        ];
        for (key, command) in entries {
            if wide_button(ui, key) {
                cmds.push(command);
            }
            ui.add_space(4.0);
        }
    });
}

fn setting_widget(ui: &mut egui::Ui, spec: &SettingSpec, settings: &mut Settings) {
    let current = settings
        .get(spec.key)
        .unwrap_or_else(|| spec.kind.default_value());
    ui.horizontal(|ui| {
        ui.label(spec.label);
        match (spec.kind, current) {
            (SettingKind::Int { min, max, .. }, SettingValue::Int(mut v)) => {
                if ui.add(egui::Slider::new(&mut v, min..=max)).changed() {
                    settings.set(spec.key, SettingValue::Int(v));
                }
            }
            (SettingKind::Bool { .. }, SettingValue::Bool(mut v)) => {
                if ui.checkbox(&mut v, "").changed() {
                    settings.set(spec.key, SettingValue::Bool(v));
                }
            }
            (SettingKind::Choice { options, .. }, SettingValue::Choice(i)) => {
                let mut picked = i;
                egui::ComboBox::from_id_salt(spec.key)
                    .selected_text(options.get(i).copied().unwrap_or(""))
                    .show_ui(ui, |ui| {
                        for (n, name) in options.iter().enumerate() {
                            ui.selectable_value(&mut picked, n, *name);
                        }
                    });
                if picked != i {
                    settings.set(spec.key, SettingValue::Choice(picked));
                }
            }
            _ => {}
        }
    });
}

/// Play screen: mini-game from the registry, maps or rotation, bots, player
/// limit, LAN opening, password and the mode's own settings.
pub fn host_setup(
    ctx: &egui::Context,
    theme: &Theme,
    model: &mut MenuModel,
    cmds: &mut Vec<UiCommand>,
) {
    panel(ctx, theme, text("host.title"), |ui| {
        if model.modes.is_empty() {
            ui.colored_label(theme.warning, text("host.no_modes"));
            back(ui, cmds);
            return;
        }
        ui.horizontal(|ui| {
            ui.label(text("host.mode"));
            let current = model.mode_index();
            let mut picked = current.unwrap_or(0);
            egui::ComboBox::from_id_salt("mode")
                .selected_text(model.modes[picked].name)
                .show_ui(ui, |ui| {
                    for (i, m) in model.modes.iter().enumerate() {
                        ui.selectable_value(&mut picked, i, m.name);
                    }
                });
            if current != Some(picked) {
                model.select_mode(picked);
            }
        });
        let mode = model.mode_index().map(|i| model.modes[i].clone());
        ui.horizontal(|ui| {
            ui.label(text("host.map"));
            let selected = model.maps.get(model.form.map).cloned().unwrap_or_default();
            ui.add_enabled_ui(!model.form.rotation, |ui| {
                egui::ComboBox::from_id_salt("map")
                    .selected_text(selected)
                    .show_ui(ui, |ui| {
                        for (i, name) in model.maps.iter().enumerate() {
                            ui.selectable_value(&mut model.form.map, i, name);
                        }
                    });
            });
        });
        ui.checkbox(&mut model.form.rotation, text("host.rotation"));
        if mode.as_ref().is_some_and(|m| m.bots) {
            ui.horizontal(|ui| {
                ui.label(text("host.bots"));
                ui.add(egui::Slider::new(&mut model.form.bots, 0..=15));
            });
            ui.horizontal(|ui| {
                ui.label(text("host.bot_level"));
                ui.add(egui::Slider::new(&mut model.form.bot_level, 0..=4));
            });
        }
        ui.horizontal(|ui| {
            ui.label(text("host.max_players"));
            ui.add(egui::Slider::new(&mut model.form.max_players, 1..=32));
        });
        ui.checkbox(&mut model.form.lan, text("host.lan"));
        ui.horizontal(|ui| {
            ui.label(text("host.password"));
            ui.add(egui::TextEdit::singleline(&mut model.form.password).password(true));
        });
        if let Some(mode) = mode.filter(|m| !m.settings.is_empty()) {
            ui.add_space(8.0);
            ui.label(text("host.mode_settings"));
            for spec in &mode.settings {
                setting_widget(ui, spec, &mut model.form.settings);
            }
        }
        ui.add_space(8.0);
        if wide_button(ui, "host.start") {
            let mut request = model.form.clone();
            request.bots = request.bots.min(request.max_players.saturating_sub(1));
            cmds.push(UiCommand::StartMatch(request));
        }
        back(ui, cmds);
    });
}

fn placeholder(
    ctx: &egui::Context,
    theme: &Theme,
    title: &str,
    body: &str,
    cmds: &mut Vec<UiCommand>,
) {
    panel(ctx, theme, title, |ui| {
        ui.colored_label(theme.text_muted, body);
        back(ui, cmds);
    });
}

/// Settings screen (content: lots 6.4 and 6.5).
pub fn settings(ctx: &egui::Context, theme: &Theme, cmds: &mut Vec<UiCommand>) {
    placeholder(
        ctx,
        theme,
        text("settings.title"),
        text("settings.placeholder"),
        cmds,
    );
}

/// Editor entry screen; the editor itself is `faset-editor`.
pub fn editor(ctx: &egui::Context, theme: &Theme, cmds: &mut Vec<UiCommand>) {
    placeholder(
        ctx,
        theme,
        text("editor.title"),
        text("editor.placeholder"),
        cmds,
    );
}

/// Waiting for a remote server.
pub fn connecting(ctx: &egui::Context, theme: &Theme, cmds: &mut Vec<UiCommand>) {
    panel(ctx, theme, text("connecting.title"), |ui| {
        ui.spinner();
        if wide_button(ui, "connecting.cancel") {
            cmds.push(UiCommand::Pop);
        }
    });
}

/// Map loading.
pub fn loading(ctx: &egui::Context, theme: &Theme, model: &MenuModel) {
    panel(ctx, theme, text("loading.title"), |ui| {
        ui.label(format!("{} {}", text("loading.map"), model.loading_map));
        ui.add(egui::ProgressBar::new(model.loading.clamp(0.0, 1.0)));
    });
}

/// Pause menu, drawn over the running match.
pub fn pause(ctx: &egui::Context, theme: &Theme, cmds: &mut Vec<UiCommand>) {
    panel(ctx, theme, text("pause.title"), |ui| {
        let entries = [
            ("pause.resume", UiCommand::Pop),
            ("pause.classes", UiCommand::Push(AppState::Loadout)),
            ("pause.settings", UiCommand::Push(AppState::Settings)),
            ("pause.leave", UiCommand::LeaveMatch),
        ];
        for (key, command) in entries {
            if wide_button(ui, key) {
                cmds.push(command);
            }
            ui.add_space(4.0);
        }
    });
}

/// Draws the screen of `state`. [`AppState::InGame`] draws nothing: the HUD
/// belongs to the mini-game.
pub fn show_state(
    state: AppState,
    ctx: &egui::Context,
    theme: &Theme,
    model: &mut MenuModel,
    cmds: &mut Vec<UiCommand>,
) {
    match state {
        AppState::Boot => boot(ctx, theme, cmds),
        AppState::MainMenu => main_menu(ctx, theme, cmds),
        AppState::ServerBrowser => crate::browser::server_browser(ctx, theme, model, cmds),
        AppState::HostSetup => host_setup(ctx, theme, model, cmds),
        AppState::Loadout => loadout_screen(ctx, theme, &mut model.loadout, cmds),
        AppState::Settings => settings(ctx, theme, cmds),
        AppState::Connecting => connecting(ctx, theme, cmds),
        AppState::Loading => loading(ctx, theme, model),
        AppState::InGame => {}
        AppState::Editor => editor(ctx, theme, cmds),
        AppState::Pause => pause(ctx, theme, cmds),
    }
}

/// A [`Screen`] showing the screen of one state and collecting its commands.
pub struct StateScreen<'a> {
    /// State to draw.
    pub state: AppState,
    /// Data the screens use.
    pub model: &'a mut MenuModel,
    /// Commands the screen issued this frame.
    pub commands: Vec<UiCommand>,
}

impl<'a> StateScreen<'a> {
    /// A screen for `state`.
    pub fn new(state: AppState, model: &'a mut MenuModel) -> Self {
        Self {
            state,
            model,
            commands: Vec::new(),
        }
    }
}

impl Screen for StateScreen<'_> {
    fn show(&mut self, ctx: &egui::Context, theme: &Theme) {
        show_state(self.state, ctx, theme, self.model, &mut self.commands);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(state: AppState, model: &mut MenuModel) -> Vec<UiCommand> {
        let ctx = egui::Context::default();
        let theme = Theme::default();
        let mut cmds = Vec::new();
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 720.0),
            )),
            ..egui::RawInput::default()
        };
        ctx.begin_pass(raw);
        show_state(state, &ctx, &theme, model, &mut cmds);
        let mut output = ctx.end_pass();
        output.textures_delta.clear();
        cmds
    }

    #[test]
    fn every_screen_runs_headless_without_commands() {
        let mut model = MenuModel::default();
        for state in AppState::ALL {
            assert!(run(state, &mut model).is_empty(), "{state:?}");
        }
    }

    #[test]
    fn model_without_modes_has_an_empty_form() {
        let model = MenuModel::new(&Registry::new(), vec!["a".into()]);
        assert!(model.modes.is_empty());
        assert!(model.form.mode.is_empty());
    }
}

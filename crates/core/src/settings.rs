//! User settings: one [`Settings`] struct described by metadata ([`META`]),
//! stored as TOML with a version number.
//!
//! The metadata is the single source of truth: the menu of `faset-ui` is
//! generated from it, defaults and ranges come from it, and
//! [`apply_settings`] walks it to tell a [`SettingsSink`] what changed.
//! Categories: display and quality (lot 6.4), controls, audio, network,
//! game and accessibility (lot 6.5).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::bindings::Bindings;

/// Version written to `settings.toml`. Older files are migrated on load.
pub const CURRENT_VERSION: u32 = 1;

/// Seconds a resolution change waits for confirmation before reverting.
pub const RESOLUTION_CONFIRM_SECONDS: f64 = 15.0;

/// Errors of saving or editing settings.
#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    /// The file or its folder could not be written.
    #[error("cannot write settings: {0}")]
    Io(#[from] std::io::Error),
    /// The settings could not be serialised.
    #[error("cannot serialise settings: {0}")]
    Serialize(#[from] toml::ser::Error),
    /// No setting has this id.
    #[error("unknown setting `{0}`")]
    UnknownSetting(String),
    /// The value has the wrong type for the setting.
    #[error("wrong value type for setting `{0}`")]
    WrongType(String),
}

/// Settings categories (menu tabs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    /// Window, resolution, frame pacing, field of view, brightness.
    Display,
    /// Render scale, upscaling and visual effects.
    Quality,
    /// Mouse, key behaviour and gamepad (key bindings have their own view).
    Controls,
    /// Volumes, output device, key sound.
    Audio,
    /// Pseudonym, snapshot rate, interpolation, port, discovery.
    Network,
    /// Reticle, HUD, weapon position, language.
    Game,
    /// Colour-blind filters, reduced motion, UI scale.
    Accessibility,
}

impl Category {
    /// Every category, in menu order.
    pub const ALL: [Self; 7] = [
        Self::Display,
        Self::Quality,
        Self::Controls,
        Self::Audio,
        Self::Network,
        Self::Game,
        Self::Accessibility,
    ];

    /// Translation key of the category title.
    pub fn label_key(self) -> &'static str {
        match self {
            Self::Display => "settings.category.display",
            Self::Quality => "settings.category.quality",
            Self::Controls => "settings.category.controls",
            Self::Audio => "settings.category.audio",
            Self::Network => "settings.category.network",
            Self::Game => "settings.category.game",
            Self::Accessibility => "settings.category.accessibility",
        }
    }
}

/// When a changed setting takes effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyMode {
    /// At once, without restarting.
    Immediate,
    /// After a restart (unused by this lot).
    Restart,
}

/// The value of one setting, in the form the menu edits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value {
    /// A switch.
    Bool(bool),
    /// An integer.
    Int(i64),
    /// A real number.
    Float(f32),
    /// The index of an option of [`SettingType::Choice`].
    Choice(usize),
}

/// The type and range of a setting.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SettingType {
    /// A switch.
    Bool,
    /// An integer in `min..=max`.
    Int {
        /// Lowest value.
        min: i64,
        /// Highest value.
        max: i64,
    },
    /// A real number in `min..=max`.
    Float {
        /// Lowest value.
        min: f32,
        /// Highest value.
        max: f32,
    },
    /// One of a list of options (their TOML names).
    Choice(&'static [&'static str]),
}

/// Metadata of one setting.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SettingMeta {
    /// Stable identifier, also the TOML key.
    pub id: &'static str,
    /// Menu category.
    pub category: Category,
    /// Translation key of the label (translated in lot 6.5).
    pub label_key: &'static str,
    /// Type, range or options.
    pub ty: SettingType,
    /// Default value.
    pub default: Value,
    /// When a change takes effect.
    pub apply: ApplyMode,
}

macro_rules! choice_enum {
    ($(#[$m:meta])* $name:ident { $($var:ident = $s:literal),+ $(,)? }) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        pub enum $name {
            $(#[doc = $s] #[serde(rename = $s)] $var),+
        }
        impl $name {
            /// TOML names of the options, in menu order.
            pub const OPTIONS: &'static [&'static str] = &[$($s),+];
            /// The options, in menu order.
            pub const ALL: &'static [Self] = &[$(Self::$var),+];
            /// Position in [`Self::ALL`].
            pub fn index(self) -> usize {
                Self::ALL.iter().position(|v| *v == self).unwrap_or(0)
            }
            /// The option at `index`.
            pub fn from_index(index: usize) -> Option<Self> {
                Self::ALL.get(index).copied()
            }
        }
    };
}

choice_enum! {
    /// How the window covers the screen.
    WindowMode { Windowed = "windowed", Borderless = "borderless", Fullscreen = "fullscreen" }
}
choice_enum! {
    /// Display resolution presets.
    Resolution {
        R1280x720 = "1280x720",
        R1600x900 = "1600x900",
        R1920x1080 = "1920x1080",
        R2560x1440 = "2560x1440",
        R3840x2160 = "3840x2160",
    }
}
choice_enum! {
    /// Which upscaler to use (the kinds of `faset-upscale`).
    UpscalerChoice { Native = "native", Taa = "taa", Fsr1 = "fsr1", Fsr3 = "fsr3", Dlss = "dlss" }
}
choice_enum! {
    /// Upscale level; `Custom` uses the free render scale.
    UpscaleLevel {
        Quality = "quality",
        Balanced = "balanced",
        Performance = "performance",
        UltraPerformance = "ultra_performance",
        Custom = "custom",
    }
}
choice_enum! {
    /// Anti-aliasing method.
    AntiAliasing { Off = "off", Fxaa = "fxaa", Taa = "taa" }
}
choice_enum! {
    /// Shadow quality.
    ShadowQuality { Off = "off", Low = "low", Medium = "medium", High = "high" }
}
choice_enum! {
    /// Shape of the reticle.
    ReticleStyle { Cross = "cross", Dot = "dot", Circle = "circle", CrossDot = "cross_dot" }
}
choice_enum! {
    /// Where the weapon is drawn in the view.
    WeaponPosition { Right = "right", Left = "left", Center = "center" }
}
choice_enum! {
    /// Interface language.
    Language { English = "english", French = "french" }
}
choice_enum! {
    /// Colour-blind filter.
    ColourBlind { Off = "off", Protanopia = "protanopia", Deuteranopia = "deuteranopia", Tritanopia = "tritanopia" }
}

impl Resolution {
    /// Width and height in pixels.
    pub fn size(self) -> (u32, u32) {
        match self {
            Self::R1280x720 => (1280, 720),
            Self::R1600x900 => (1600, 900),
            Self::R1920x1080 => (1920, 1080),
            Self::R2560x1440 => (2560, 1440),
            Self::R3840x2160 => (3840, 2160),
        }
    }
}

impl UpscaleLevel {
    /// Factor per axis of a preset level (`None` for `Custom`).
    pub fn factor(self) -> Option<f32> {
        match self {
            Self::Quality => Some(1.5),
            Self::Balanced => Some(1.7),
            Self::Performance => Some(2.0),
            Self::UltraPerformance => Some(3.0),
            Self::Custom => None,
        }
    }
}

/// Display category.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DisplaySettings {
    /// Window mode.
    pub window_mode: WindowMode,
    /// Resolution (windowed size, or the video mode in fullscreen).
    pub resolution: Resolution,
    /// Monitor index.
    pub monitor: u32,
    /// Vertical synchronisation.
    pub vsync: bool,
    /// Frame rate cap; 0 is unlimited.
    pub fps_cap: u32,
    /// Vertical field of view in degrees.
    pub fov: u32,
    /// Brightness multiplier (1 is neutral).
    pub brightness: f32,
}

impl Default for DisplaySettings {
    fn default() -> Self {
        Self {
            window_mode: WindowMode::Windowed,
            resolution: Resolution::R1600x900,
            monitor: 0,
            vsync: true,
            fps_cap: 0,
            fov: 80,
            brightness: 1.0,
        }
    }
}

/// Quality category.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct QualitySettings {
    /// Free render scale in percent (50 to 100; Native and FSR 1 only).
    pub render_scale: u32,
    /// Upscaler.
    pub upscaler: UpscalerChoice,
    /// Upscale level.
    pub upscale_level: UpscaleLevel,
    /// Upscaler sharpening, 0 to 1.
    pub sharpness: f32,
    /// Anti-aliasing.
    pub anti_aliasing: AntiAliasing,
    /// Shadows.
    pub shadows: ShadowQuality,
    /// Ambient occlusion.
    pub occlusion: bool,
    /// Bloom.
    pub bloom: bool,
    /// Outlines (contours).
    pub outlines: bool,
    /// Weather particle density, 0 to 1.
    pub weather_density: f32,
    /// Gameplay screen effects (damage flash, speed lines).
    pub screen_effects: bool,
    /// Draw distance in metres.
    pub draw_distance: u32,
}

impl Default for QualitySettings {
    fn default() -> Self {
        Self {
            render_scale: 100,
            upscaler: UpscalerChoice::Fsr1,
            upscale_level: UpscaleLevel::Custom,
            sharpness: 0.9,
            anti_aliasing: AntiAliasing::Fxaa,
            shadows: ShadowQuality::Medium,
            occlusion: true,
            bloom: true,
            outlines: true,
            weather_density: 1.0,
            screen_effects: true,
            draw_distance: 1000,
        }
    }
}

/// Controls category (key bindings are in [`Settings::bindings`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ControlSettings {
    /// In-game sensitivity (`sens` of [`cm_per_360`]).
    pub sensitivity: f32,
    /// Multiplier of the sensitivity while aiming.
    pub aim_sensitivity: f32,
    /// Mouse resolution in dots per inch (`dpi` of [`cm_per_360`]).
    pub mouse_dpi: u32,
    /// Invert the vertical axis.
    pub invert_y: bool,
    /// Crouch toggles instead of being held.
    pub crouch_toggle: bool,
    /// Aim toggles instead of being held.
    pub aim_toggle: bool,
    /// Holding jump keeps jumping.
    pub hold_jump: bool,
    /// Gamepad input.
    pub gamepad: bool,
}

impl Default for ControlSettings {
    fn default() -> Self {
        Self {
            sensitivity: 2.0,
            aim_sensitivity: 1.0,
            mouse_dpi: 800,
            invert_y: false,
            crouch_toggle: false,
            aim_toggle: false,
            hold_jump: true,
            gamepad: true,
        }
    }
}

/// Audio category. There is no music volume: there is no music.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioSettings {
    /// Master volume, 0 to 1.
    pub master: f32,
    /// Effects volume, 0 to 1.
    pub effects: f32,
    /// Interface volume, 0 to 1.
    pub ui: f32,
    /// Output device number (decision: 0 is the system default; the audio
    /// crate has no device enumeration yet, so the host maps the number).
    pub output_device: u32,
    /// Typing sound on keys.
    pub key_sound: bool,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            master: 1.0,
            effects: 1.0,
            ui: 1.0,
            output_device: 0,
            key_sound: true,
        }
    }
}

/// Longest pseudonym, in characters.
pub const MAX_PSEUDONYM_CHARS: usize = 24;

/// Network category.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NetworkSettings {
    /// Player name shown to others (text, outside [`META`]).
    pub pseudonym: String,
    /// Snapshot frequency asked of the server, 20 to 60 Hz.
    pub snapshot_hz: u32,
    /// Interpolation delay computed from the jitter.
    pub interpolation_auto: bool,
    /// Manual interpolation delay in milliseconds.
    pub interpolation_delay_ms: u32,
    /// UDP port.
    pub port: u16,
    /// Discover servers on the local network.
    pub lan_discovery: bool,
    /// Show the network graph.
    pub net_graph: bool,
}

impl Default for NetworkSettings {
    fn default() -> Self {
        Self {
            pseudonym: "Player".to_owned(),
            snapshot_hz: 30,
            interpolation_auto: true,
            interpolation_delay_ms: 100,
            port: 7777,
            lan_discovery: true,
            net_graph: false,
        }
    }
}

/// Game category.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GameSettings {
    /// Reticle shape.
    pub reticle_style: ReticleStyle,
    /// Reticle size in pixels.
    pub reticle_size: u32,
    /// Reticle line thickness in pixels.
    pub reticle_thickness: u32,
    /// Gap at the reticle centre in pixels.
    pub reticle_gap: u32,
    /// Reticle opacity, 0 to 1.
    pub reticle_opacity: f32,
    /// HUD scale factor.
    pub hud_scale: f32,
    /// Show the speed indicator.
    pub speed_indicator: bool,
    /// Show damage numbers.
    pub damage_numbers: bool,
    /// Weapon position in the view.
    pub weapon_position: WeaponPosition,
    /// Interface language.
    pub language: Language,
}

impl Default for GameSettings {
    fn default() -> Self {
        Self {
            reticle_style: ReticleStyle::Cross,
            reticle_size: 16,
            reticle_thickness: 2,
            reticle_gap: 4,
            reticle_opacity: 1.0,
            hud_scale: 1.0,
            speed_indicator: false,
            damage_numbers: true,
            weapon_position: WeaponPosition::Right,
            language: Language::English,
        }
    }
}

/// Accessibility category.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AccessibilitySettings {
    /// Colour-blind filter.
    pub colour_blind: ColourBlind,
    /// Reduce camera shake.
    pub reduce_shake: bool,
    /// Reduce flashes.
    pub reduce_flashes: bool,
    /// Interface scale factor.
    pub ui_scale: f32,
}

impl Default for AccessibilitySettings {
    fn default() -> Self {
        Self {
            colour_blind: ColourBlind::Off,
            reduce_shake: false,
            reduce_flashes: false,
            ui_scale: 1.0,
        }
    }
}

/// Centimetres of mouse travel for a full turn (plan section 19, constant
/// 0.022 as written there): `360 * 2.54 / (0.022 * sens * dpi)`.
pub fn cm_per_360(sens: f32, dpi: u32) -> f32 {
    let denom = 0.022 * sens * dpi as f32;
    if denom <= 0.0 {
        return f32::INFINITY;
    }
    360.0 * 2.54 / denom
}

/// Automatic interpolation delay `d = max(2I, I + 2j)` in seconds, `I` being
/// the snapshot interval and `j` the smoothed jitter (plan section 6).
pub fn auto_interpolation_delay(interval: f64, jitter: f64) -> f64 {
    (2.0 * interval).max(interval + 2.0 * jitter)
}

/// Every user setting.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// File format version.
    pub version: u32,
    /// Display category.
    pub display: DisplaySettings,
    /// Quality category.
    pub quality: QualitySettings,
    /// Controls category.
    pub controls: ControlSettings,
    /// Audio category.
    pub audio: AudioSettings,
    /// Network category.
    pub network: NetworkSettings,
    /// Game category.
    pub game: GameSettings,
    /// Accessibility category.
    pub accessibility: AccessibilitySettings,
    /// Key bindings.
    pub bindings: Bindings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: CURRENT_VERSION,
            display: DisplaySettings::default(),
            quality: QualitySettings::default(),
            controls: ControlSettings::default(),
            audio: AudioSettings::default(),
            network: NetworkSettings::default(),
            game: GameSettings::default(),
            accessibility: AccessibilitySettings::default(),
            bindings: Bindings::default(),
        }
    }
}

const fn meta(
    id: &'static str,
    category: Category,
    label_key: &'static str,
    ty: SettingType,
    default: Value,
) -> SettingMeta {
    SettingMeta {
        id,
        category,
        label_key,
        ty,
        default,
        apply: ApplyMode::Immediate,
    }
}

const D: Category = Category::Display;
const Q: Category = Category::Quality;
const C: Category = Category::Controls;
const A: Category = Category::Audio;
const N: Category = Category::Network;
const G: Category = Category::Game;
const X: Category = Category::Accessibility;

/// Metadata of every setting, in menu order.
#[rustfmt::skip]
pub const META: &[SettingMeta] = &[
    meta("window_mode", D, "settings.window_mode", SettingType::Choice(WindowMode::OPTIONS), Value::Choice(0)),
    meta("resolution", D, "settings.resolution", SettingType::Choice(Resolution::OPTIONS), Value::Choice(1)),
    meta("monitor", D, "settings.monitor", SettingType::Int { min: 0, max: 15 }, Value::Int(0)),
    meta("vsync", D, "settings.vsync", SettingType::Bool, Value::Bool(true)),
    meta("fps_cap", D, "settings.fps_cap", SettingType::Int { min: 0, max: 500 }, Value::Int(0)),
    meta("fov", D, "settings.fov", SettingType::Int { min: 60, max: 120 }, Value::Int(80)),
    meta("brightness", D, "settings.brightness", SettingType::Float { min: 0.5, max: 1.5 }, Value::Float(1.0)),
    meta("render_scale", Q, "settings.render_scale", SettingType::Int { min: 50, max: 100 }, Value::Int(100)),
    meta("upscaler", Q, "settings.upscaler", SettingType::Choice(UpscalerChoice::OPTIONS), Value::Choice(2)),
    meta("upscale_level", Q, "settings.upscale_level", SettingType::Choice(UpscaleLevel::OPTIONS), Value::Choice(4)),
    meta("sharpness", Q, "settings.sharpness", SettingType::Float { min: 0.0, max: 1.0 }, Value::Float(0.9)),
    meta("anti_aliasing", Q, "settings.anti_aliasing", SettingType::Choice(AntiAliasing::OPTIONS), Value::Choice(1)),
    meta("shadows", Q, "settings.shadows", SettingType::Choice(ShadowQuality::OPTIONS), Value::Choice(2)),
    meta("occlusion", Q, "settings.occlusion", SettingType::Bool, Value::Bool(true)),
    meta("bloom", Q, "settings.bloom", SettingType::Bool, Value::Bool(true)),
    meta("outlines", Q, "settings.outlines", SettingType::Bool, Value::Bool(true)),
    meta("weather_density", Q, "settings.weather_density", SettingType::Float { min: 0.0, max: 1.0 }, Value::Float(1.0)),
    meta("screen_effects", Q, "settings.screen_effects", SettingType::Bool, Value::Bool(true)),
    meta("draw_distance", Q, "settings.draw_distance", SettingType::Int { min: 100, max: 2000 }, Value::Int(1000)),
    meta("sensitivity", C, "settings.sensitivity", SettingType::Float { min: 0.1, max: 20.0 }, Value::Float(2.0)),
    meta("aim_sensitivity", C, "settings.aim_sensitivity", SettingType::Float { min: 0.1, max: 2.0 }, Value::Float(1.0)),
    meta("mouse_dpi", C, "settings.mouse_dpi", SettingType::Int { min: 100, max: 32000 }, Value::Int(800)),
    meta("invert_y", C, "settings.invert_y", SettingType::Bool, Value::Bool(false)),
    meta("crouch_toggle", C, "settings.crouch_toggle", SettingType::Bool, Value::Bool(false)),
    meta("aim_toggle", C, "settings.aim_toggle", SettingType::Bool, Value::Bool(false)),
    meta("hold_jump", C, "settings.hold_jump", SettingType::Bool, Value::Bool(true)),
    meta("gamepad", C, "settings.gamepad", SettingType::Bool, Value::Bool(true)),
    meta("master_volume", A, "settings.master_volume", SettingType::Float { min: 0.0, max: 1.0 }, Value::Float(1.0)),
    meta("effects_volume", A, "settings.effects_volume", SettingType::Float { min: 0.0, max: 1.0 }, Value::Float(1.0)),
    meta("ui_volume", A, "settings.ui_volume", SettingType::Float { min: 0.0, max: 1.0 }, Value::Float(1.0)),
    meta("output_device", A, "settings.output_device", SettingType::Int { min: 0, max: 15 }, Value::Int(0)),
    meta("key_sound", A, "settings.key_sound", SettingType::Bool, Value::Bool(true)),
    meta("snapshot_hz", N, "settings.snapshot_hz", SettingType::Int { min: 20, max: 60 }, Value::Int(30)),
    meta("interpolation_auto", N, "settings.interpolation_auto", SettingType::Bool, Value::Bool(true)),
    meta("interpolation_delay_ms", N, "settings.interpolation_delay_ms", SettingType::Int { min: 20, max: 500 }, Value::Int(100)),
    meta("port", N, "settings.port", SettingType::Int { min: 1024, max: 65535 }, Value::Int(7777)),
    meta("lan_discovery", N, "settings.lan_discovery", SettingType::Bool, Value::Bool(true)),
    meta("net_graph", N, "settings.net_graph", SettingType::Bool, Value::Bool(false)),
    meta("reticle_style", G, "settings.reticle_style", SettingType::Choice(ReticleStyle::OPTIONS), Value::Choice(0)),
    meta("reticle_size", G, "settings.reticle_size", SettingType::Int { min: 4, max: 64 }, Value::Int(16)),
    meta("reticle_thickness", G, "settings.reticle_thickness", SettingType::Int { min: 1, max: 8 }, Value::Int(2)),
    meta("reticle_gap", G, "settings.reticle_gap", SettingType::Int { min: 0, max: 20 }, Value::Int(4)),
    meta("reticle_opacity", G, "settings.reticle_opacity", SettingType::Float { min: 0.1, max: 1.0 }, Value::Float(1.0)),
    meta("hud_scale", G, "settings.hud_scale", SettingType::Float { min: 0.5, max: 2.0 }, Value::Float(1.0)),
    meta("speed_indicator", G, "settings.speed_indicator", SettingType::Bool, Value::Bool(false)),
    meta("damage_numbers", G, "settings.damage_numbers", SettingType::Bool, Value::Bool(true)),
    meta("weapon_position", G, "settings.weapon_position", SettingType::Choice(WeaponPosition::OPTIONS), Value::Choice(0)),
    meta("language", G, "settings.language", SettingType::Choice(Language::OPTIONS), Value::Choice(0)),
    meta("colour_blind", X, "settings.colour_blind", SettingType::Choice(ColourBlind::OPTIONS), Value::Choice(0)),
    meta("reduce_shake", X, "settings.reduce_shake", SettingType::Bool, Value::Bool(false)),
    meta("reduce_flashes", X, "settings.reduce_flashes", SettingType::Bool, Value::Bool(false)),
    meta("ui_scale", X, "settings.ui_scale", SettingType::Float { min: 0.75, max: 1.5 }, Value::Float(1.0)),
];

/// The metadata of setting `id`.
pub fn meta_of(id: &str) -> Option<&'static SettingMeta> {
    META.iter().find(|m| m.id == id)
}

impl SettingType {
    /// Brings `value` into this type's range; a value of another type
    /// becomes `None`.
    pub fn clamp(&self, value: Value) -> Option<Value> {
        match (self, value) {
            (Self::Bool, Value::Bool(b)) => Some(Value::Bool(b)),
            (Self::Int { min, max }, Value::Int(v)) => Some(Value::Int(v.clamp(*min, *max))),
            (Self::Float { min, max }, Value::Float(v)) => {
                let v = if v.is_nan() {
                    *min
                } else {
                    v.clamp(*min, *max)
                };
                Some(Value::Float(v))
            }
            (Self::Choice(options), Value::Choice(i)) => {
                Some(Value::Choice(i.min(options.len().saturating_sub(1))))
            }
            _ => None,
        }
    }
}

impl Settings {
    /// The value of setting `id`.
    pub fn get(&self, id: &str) -> Option<Value> {
        let (d, q) = (&self.display, &self.quality);
        Some(match id {
            "window_mode" => Value::Choice(d.window_mode.index()),
            "resolution" => Value::Choice(d.resolution.index()),
            "monitor" => Value::Int(i64::from(d.monitor)),
            "vsync" => Value::Bool(d.vsync),
            "fps_cap" => Value::Int(i64::from(d.fps_cap)),
            "fov" => Value::Int(i64::from(d.fov)),
            "brightness" => Value::Float(d.brightness),
            "render_scale" => Value::Int(i64::from(q.render_scale)),
            "upscaler" => Value::Choice(q.upscaler.index()),
            "upscale_level" => Value::Choice(q.upscale_level.index()),
            "sharpness" => Value::Float(q.sharpness),
            "anti_aliasing" => Value::Choice(q.anti_aliasing.index()),
            "shadows" => Value::Choice(q.shadows.index()),
            "occlusion" => Value::Bool(q.occlusion),
            "bloom" => Value::Bool(q.bloom),
            "outlines" => Value::Bool(q.outlines),
            "weather_density" => Value::Float(q.weather_density),
            "screen_effects" => Value::Bool(q.screen_effects),
            "draw_distance" => Value::Int(i64::from(q.draw_distance)),
            "sensitivity" => Value::Float(self.controls.sensitivity),
            "aim_sensitivity" => Value::Float(self.controls.aim_sensitivity),
            "mouse_dpi" => Value::Int(i64::from(self.controls.mouse_dpi)),
            "invert_y" => Value::Bool(self.controls.invert_y),
            "crouch_toggle" => Value::Bool(self.controls.crouch_toggle),
            "aim_toggle" => Value::Bool(self.controls.aim_toggle),
            "hold_jump" => Value::Bool(self.controls.hold_jump),
            "gamepad" => Value::Bool(self.controls.gamepad),
            "master_volume" => Value::Float(self.audio.master),
            "effects_volume" => Value::Float(self.audio.effects),
            "ui_volume" => Value::Float(self.audio.ui),
            "output_device" => Value::Int(i64::from(self.audio.output_device)),
            "key_sound" => Value::Bool(self.audio.key_sound),
            "snapshot_hz" => Value::Int(i64::from(self.network.snapshot_hz)),
            "interpolation_auto" => Value::Bool(self.network.interpolation_auto),
            "interpolation_delay_ms" => Value::Int(i64::from(self.network.interpolation_delay_ms)),
            "port" => Value::Int(i64::from(self.network.port)),
            "lan_discovery" => Value::Bool(self.network.lan_discovery),
            "net_graph" => Value::Bool(self.network.net_graph),
            "reticle_style" => Value::Choice(self.game.reticle_style.index()),
            "reticle_size" => Value::Int(i64::from(self.game.reticle_size)),
            "reticle_thickness" => Value::Int(i64::from(self.game.reticle_thickness)),
            "reticle_gap" => Value::Int(i64::from(self.game.reticle_gap)),
            "reticle_opacity" => Value::Float(self.game.reticle_opacity),
            "hud_scale" => Value::Float(self.game.hud_scale),
            "speed_indicator" => Value::Bool(self.game.speed_indicator),
            "damage_numbers" => Value::Bool(self.game.damage_numbers),
            "weapon_position" => Value::Choice(self.game.weapon_position.index()),
            "language" => Value::Choice(self.game.language.index()),
            "colour_blind" => Value::Choice(self.accessibility.colour_blind.index()),
            "reduce_shake" => Value::Bool(self.accessibility.reduce_shake),
            "reduce_flashes" => Value::Bool(self.accessibility.reduce_flashes),
            "ui_scale" => Value::Float(self.accessibility.ui_scale),
            _ => return None,
        })
    }

    /// Sets setting `id`, clamping the value to its range.
    pub fn set(&mut self, id: &str, value: Value) -> Result<(), SettingsError> {
        let m = meta_of(id).ok_or_else(|| SettingsError::UnknownSetting(id.to_owned()))?;
        let wrong = || SettingsError::WrongType(id.to_owned());
        let value = m.ty.clamp(value).ok_or_else(wrong)?;
        let (d, q) = (&mut self.display, &mut self.quality);
        match (id, value) {
            ("window_mode", Value::Choice(i)) => {
                d.window_mode = WindowMode::from_index(i).ok_or_else(wrong)?
            }
            ("resolution", Value::Choice(i)) => {
                d.resolution = Resolution::from_index(i).ok_or_else(wrong)?
            }
            ("monitor", Value::Int(v)) => d.monitor = v as u32,
            ("vsync", Value::Bool(v)) => d.vsync = v,
            ("fps_cap", Value::Int(v)) => d.fps_cap = v as u32,
            ("fov", Value::Int(v)) => d.fov = v as u32,
            ("brightness", Value::Float(v)) => d.brightness = v,
            ("render_scale", Value::Int(v)) => q.render_scale = v as u32,
            ("upscaler", Value::Choice(i)) => {
                q.upscaler = UpscalerChoice::from_index(i).ok_or_else(wrong)?
            }
            ("upscale_level", Value::Choice(i)) => {
                q.upscale_level = UpscaleLevel::from_index(i).ok_or_else(wrong)?
            }
            ("sharpness", Value::Float(v)) => q.sharpness = v,
            ("anti_aliasing", Value::Choice(i)) => {
                q.anti_aliasing = AntiAliasing::from_index(i).ok_or_else(wrong)?
            }
            ("shadows", Value::Choice(i)) => {
                q.shadows = ShadowQuality::from_index(i).ok_or_else(wrong)?
            }
            ("occlusion", Value::Bool(v)) => q.occlusion = v,
            ("bloom", Value::Bool(v)) => q.bloom = v,
            ("outlines", Value::Bool(v)) => q.outlines = v,
            ("weather_density", Value::Float(v)) => q.weather_density = v,
            ("screen_effects", Value::Bool(v)) => q.screen_effects = v,
            ("draw_distance", Value::Int(v)) => q.draw_distance = v as u32,
            ("sensitivity", Value::Float(v)) => self.controls.sensitivity = v,
            ("aim_sensitivity", Value::Float(v)) => self.controls.aim_sensitivity = v,
            ("mouse_dpi", Value::Int(v)) => self.controls.mouse_dpi = v as u32,
            ("invert_y", Value::Bool(v)) => self.controls.invert_y = v,
            ("crouch_toggle", Value::Bool(v)) => self.controls.crouch_toggle = v,
            ("aim_toggle", Value::Bool(v)) => self.controls.aim_toggle = v,
            ("hold_jump", Value::Bool(v)) => self.controls.hold_jump = v,
            ("gamepad", Value::Bool(v)) => self.controls.gamepad = v,
            ("master_volume", Value::Float(v)) => self.audio.master = v,
            ("effects_volume", Value::Float(v)) => self.audio.effects = v,
            ("ui_volume", Value::Float(v)) => self.audio.ui = v,
            ("output_device", Value::Int(v)) => self.audio.output_device = v as u32,
            ("key_sound", Value::Bool(v)) => self.audio.key_sound = v,
            ("snapshot_hz", Value::Int(v)) => self.network.snapshot_hz = v as u32,
            ("interpolation_auto", Value::Bool(v)) => self.network.interpolation_auto = v,
            ("interpolation_delay_ms", Value::Int(v)) => {
                self.network.interpolation_delay_ms = v as u32
            }
            ("port", Value::Int(v)) => self.network.port = v as u16,
            ("lan_discovery", Value::Bool(v)) => self.network.lan_discovery = v,
            ("net_graph", Value::Bool(v)) => self.network.net_graph = v,
            ("reticle_style", Value::Choice(i)) => {
                self.game.reticle_style = ReticleStyle::from_index(i).ok_or_else(wrong)?
            }
            ("reticle_size", Value::Int(v)) => self.game.reticle_size = v as u32,
            ("reticle_thickness", Value::Int(v)) => self.game.reticle_thickness = v as u32,
            ("reticle_gap", Value::Int(v)) => self.game.reticle_gap = v as u32,
            ("reticle_opacity", Value::Float(v)) => self.game.reticle_opacity = v,
            ("hud_scale", Value::Float(v)) => self.game.hud_scale = v,
            ("speed_indicator", Value::Bool(v)) => self.game.speed_indicator = v,
            ("damage_numbers", Value::Bool(v)) => self.game.damage_numbers = v,
            ("weapon_position", Value::Choice(i)) => {
                self.game.weapon_position = WeaponPosition::from_index(i).ok_or_else(wrong)?
            }
            ("language", Value::Choice(i)) => {
                self.game.language = Language::from_index(i).ok_or_else(wrong)?
            }
            ("colour_blind", Value::Choice(i)) => {
                self.accessibility.colour_blind = ColourBlind::from_index(i).ok_or_else(wrong)?
            }
            ("reduce_shake", Value::Bool(v)) => self.accessibility.reduce_shake = v,
            ("reduce_flashes", Value::Bool(v)) => self.accessibility.reduce_flashes = v,
            ("ui_scale", Value::Float(v)) => self.accessibility.ui_scale = v,
            _ => return Err(wrong()),
        }
        Ok(())
    }

    /// Brings every field into its range (after loading a hand-edited file).
    pub fn clamp(&mut self) {
        self.clean_pseudonym();
        for m in META {
            if let Some(v) = self.get(m.id).and_then(|v| m.ty.clamp(v)) {
                let _ = self.set(m.id, v);
            }
        }
    }

    /// Resets the display category to its defaults (`--safe`).
    pub fn reset_display(&mut self) {
        self.display = DisplaySettings::default();
    }

    /// Centimetres per full turn at the current sensitivity and dpi.
    pub fn cm_per_360(&self) -> f32 {
        cm_per_360(self.controls.sensitivity, self.controls.mouse_dpi)
    }

    /// Interval between snapshots in seconds.
    pub fn snapshot_interval(&self) -> f64 {
        1.0 / f64::from(self.network.snapshot_hz.clamp(20, 60))
    }

    /// Interpolation delay in seconds for the smoothed `jitter` (seconds):
    /// the formula when automatic, the manual value otherwise.
    pub fn interpolation_delay(&self, jitter: f64) -> f64 {
        if self.network.interpolation_auto {
            auto_interpolation_delay(self.snapshot_interval(), jitter)
        } else {
            f64::from(self.network.interpolation_delay_ms) / 1000.0
        }
    }

    /// Keeps the pseudonym printable and at most [`MAX_PSEUDONYM_CHARS`]
    /// long; an empty one becomes the default.
    pub fn clean_pseudonym(&mut self) {
        let cleaned: String = self
            .network
            .pseudonym
            .chars()
            .filter(|c| !c.is_control())
            .take(MAX_PSEUDONYM_CHARS)
            .collect();
        let cleaned = cleaned.trim().to_owned();
        self.network.pseudonym = if cleaned.is_empty() {
            NetworkSettings::default().pseudonym
        } else {
            cleaned
        };
    }

    /// Ids of the settings whose value differs from `other`'s, in menu order.
    pub fn changed_from(&self, other: &Self) -> Vec<&'static str> {
        META.iter()
            .filter(|m| self.get(m.id) != other.get(m.id))
            .map(|m| m.id)
            .collect()
    }
}

/// What applies a changed setting to the running game (the renderer, the
/// window, the camera). Implemented by the executable.
pub trait SettingsSink {
    /// Applies setting `meta` with the value it has in `settings`. Returns
    /// whether it took effect.
    fn apply(&mut self, meta: &SettingMeta, settings: &Settings) -> bool;
}

/// Applies every [`ApplyMode::Immediate`] setting that differs between `old`
/// and `new` through `sink`. Returns the ids that were applied; settings with
/// [`ApplyMode::Restart`] are left out.
pub fn apply_settings(
    old: &Settings,
    new: &Settings,
    sink: &mut dyn SettingsSink,
) -> Vec<&'static str> {
    let mut applied = Vec::new();
    for m in META {
        if old.get(m.id) == new.get(m.id) || m.apply != ApplyMode::Immediate {
            continue;
        }
        if sink.apply(m, new) {
            applied.push(m.id);
        }
    }
    applied
}

/// State of a pending resolution change.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConfirmState {
    /// Still waiting; seconds left.
    Waiting(f64),
    /// The time ran out: go back to the previous resolution.
    Expired,
}

/// A resolution change the player must confirm within
/// [`RESOLUTION_CONFIRM_SECONDS`]. Time is passed in (seconds on any
/// monotonic clock), so tests drive it with a simulated clock.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResolutionConfirm {
    /// The resolution to go back to.
    pub previous: Resolution,
    started: f64,
}

impl ResolutionConfirm {
    /// Starts the countdown at `now`.
    pub fn start(previous: Resolution, now: f64) -> Self {
        Self {
            previous,
            started: now,
        }
    }

    /// Where the countdown stands at `now`.
    pub fn poll(&self, now: f64) -> ConfirmState {
        let left = RESOLUTION_CONFIRM_SECONDS - (now - self.started);
        if left > 0.0 {
            ConfirmState::Waiting(left)
        } else {
            ConfirmState::Expired
        }
    }
}

/// The OS configuration folder of Faset (`%APPDATA%\Faset`,
/// `~/Library/Application Support/Faset`, `$XDG_CONFIG_HOME/faset`).
pub fn config_dir() -> Option<PathBuf> {
    let var = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    if cfg!(windows) {
        var("APPDATA").map(|p| p.join("Faset"))
    } else if cfg!(target_os = "macos") {
        var("HOME").map(|p| p.join("Library/Application Support/Faset"))
    } else {
        var("XDG_CONFIG_HOME")
            .or_else(|| var("HOME").map(|p| p.join(".config")))
            .map(|p| p.join("faset"))
    }
}

/// Default path of `settings.toml`.
pub fn settings_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("settings.toml"))
}

/// Upgrades a parsed file of an older version to [`CURRENT_VERSION`].
///
/// Version 0 (no `version` key) kept every key at the top level, had a
/// boolean `fullscreen` and a `render_scale` fraction (0.5 to 1.0).
fn migrate(mut table: toml::Table) -> toml::Table {
    let version = table
        .get("version")
        .and_then(toml::Value::as_integer)
        .unwrap_or(0);
    if version < 1 {
        let mut display = toml::Table::new();
        let mut quality = toml::Table::new();
        if let Some(toml::Value::Boolean(full)) = table.remove("fullscreen") {
            let mode = if full { "fullscreen" } else { "windowed" };
            display.insert("window_mode".into(), mode.into());
        }
        if let Some(scale) = table.remove("render_scale") {
            let percent = match scale {
                toml::Value::Float(f) => (f * 100.0).round() as i64,
                toml::Value::Integer(i) => i,
                _ => 100,
            };
            quality.insert("render_scale".into(), percent.into());
        }
        for key in ["vsync", "fps_cap", "fov", "brightness", "monitor"] {
            if let Some(v) = table.remove(key) {
                display.insert(key.into(), v);
            }
        }
        for key in ["sharpness", "bloom", "shadows", "draw_distance"] {
            if let Some(v) = table.remove(key) {
                quality.insert(key.into(), v);
            }
        }
        table.insert("display".into(), toml::Value::Table(display));
        table.insert("quality".into(), toml::Value::Table(quality));
    }
    table.insert("version".into(), i64::from(CURRENT_VERSION).into());
    table
}

/// Parses and migrates settings text; unknown keys are ignored.
pub fn parse_settings(text: &str) -> Result<Settings, toml::de::Error> {
    let table: toml::Table = text.parse()?;
    let mut settings = Settings::deserialize(toml::Value::Table(migrate(table)))?;
    settings.version = CURRENT_VERSION;
    settings.clamp();
    Ok(settings)
}

/// Loads `settings.toml`. A missing file gives the defaults; an invalid one
/// is logged and gives the defaults too.
pub fn load_settings(path: &Path) -> Settings {
    match std::fs::read_to_string(path) {
        Ok(text) => parse_settings(&text).unwrap_or_else(|e| {
            tracing::warn!("{}: {e}; using the default settings", path.display());
            Settings::default()
        }),
        Err(_) => Settings::default(),
    }
}

/// [`load_settings`], with the display category reset to its defaults when
/// `safe` is set (`--safe`: a resolution or mode the monitor cannot show must
/// not lock the player out). The file itself is not touched.
pub fn load_settings_with(path: &Path, safe: bool) -> Settings {
    let mut settings = load_settings(path);
    if safe {
        settings.reset_display();
    }
    settings
}

/// Serialises `settings` to TOML text.
pub fn settings_to_string(settings: &Settings) -> Result<String, toml::ser::Error> {
    toml::to_string_pretty(settings)
}

/// Writes `settings` to `path`, creating the folder, through a temporary file.
pub fn save_settings(path: &Path, settings: &Settings) -> Result<(), SettingsError> {
    let text = settings_to_string(settings)?;
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Records every applied id.
    struct Recorder(Vec<&'static str>);
    impl SettingsSink for Recorder {
        fn apply(&mut self, meta: &SettingMeta, _: &Settings) -> bool {
            self.0.push(meta.id);
            true
        }
    }

    /// A value different from the current one, inside the range.
    fn other(m: &SettingMeta, cur: Value) -> Value {
        match (m.ty, cur) {
            (SettingType::Bool, Value::Bool(b)) => Value::Bool(!b),
            (SettingType::Int { min, max }, Value::Int(v)) => {
                Value::Int(if v == min { max } else { min })
            }
            (SettingType::Float { min, max }, Value::Float(v)) => {
                Value::Float(if v == min { max } else { min })
            }
            (SettingType::Choice(o), Value::Choice(i)) => Value::Choice((i + 1) % o.len()),
            _ => unreachable!(),
        }
    }

    fn temp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("faset-{name}-{}/settings.toml", std::process::id()))
    }

    #[test]
    fn every_field_has_metadata_matching_its_default() {
        let defaults = Settings::default();
        assert_eq!(META.len(), 19 + 33);
        for m in META {
            assert_eq!(defaults.get(m.id), Some(m.default), "{}", m.id);
            assert_eq!(m.ty.clamp(m.default), Some(m.default), "{}", m.id);
            assert_eq!(m.apply, ApplyMode::Immediate, "{}", m.id);
        }
        let mut ids: Vec<_> = META.iter().map(|m| m.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), META.len());
    }

    #[test]
    fn changing_each_field_applies_it_immediately() {
        for m in META {
            let old = Settings::default();
            let mut new = old.clone();
            new.set(m.id, other(m, m.default)).unwrap();
            let mut sink = Recorder(Vec::new());
            let applied = apply_settings(&old, &new, &mut sink);
            assert_eq!(applied, vec![m.id]);
            assert_eq!(sink.0, vec![m.id]);
        }
        let mut sink = Recorder(Vec::new());
        let s = Settings::default();
        assert!(apply_settings(&s, &s, &mut sink).is_empty());
    }

    #[test]
    fn set_clamps_and_rejects_wrong_types() {
        let mut s = Settings::default();
        s.set("fov", Value::Int(500)).unwrap();
        assert_eq!(s.display.fov, 120);
        s.set("render_scale", Value::Int(1)).unwrap();
        assert_eq!(s.quality.render_scale, 50);
        assert!(s.set("fov", Value::Bool(true)).is_err());
        assert!(s.set("nope", Value::Bool(true)).is_err());
    }

    #[test]
    fn toml_round_trip() {
        let mut s = Settings::default();
        s.set("fov", Value::Int(100)).unwrap();
        s.set("window_mode", Value::Choice(2)).unwrap();
        s.set("brightness", Value::Float(1.25)).unwrap();
        let path = temp("roundtrip");
        save_settings(&path, &s).unwrap();
        assert_eq!(load_settings(&path), s);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
        assert_eq!(load_settings(&path), Settings::default());
    }

    #[test]
    fn unknown_keys_are_ignored_and_values_clamped() {
        let text = "version = 1\nmystery = 3\n[display]\nfov = 999\nextra = true\n";
        let s = parse_settings(text).unwrap();
        assert_eq!(s.display.fov, 120);
    }

    #[test]
    fn old_version_migrates_to_current() {
        let old = "fullscreen = true\nrender_scale = 0.75\nvsync = false\nfov = 90\n";
        let s = parse_settings(old).unwrap();
        assert_eq!(s.version, CURRENT_VERSION);
        assert_eq!(s.display.window_mode, WindowMode::Fullscreen);
        assert_eq!(s.quality.render_scale, 75);
        assert!(!s.display.vsync);
        assert_eq!(s.display.fov, 90);
        assert!(settings_to_string(&s).unwrap().contains("version = 1"));
    }

    #[test]
    fn resolution_reverts_after_fifteen_seconds_without_confirmation() {
        let c = ResolutionConfirm::start(Resolution::R1920x1080, 100.0);
        assert_eq!(c.poll(100.0), ConfirmState::Waiting(15.0));
        assert!(matches!(c.poll(114.9), ConfirmState::Waiting(_)));
        assert_eq!(c.poll(115.0), ConfirmState::Expired);
        assert_eq!(c.previous, Resolution::R1920x1080);
    }

    #[test]
    fn sensitivity_in_centimetres_per_turn() {
        // 360 * 2.54 / (0.022 * 2 * 800) = 25.98 cm.
        assert!((cm_per_360(2.0, 800) - 25.977).abs() < 0.01);
        assert!(cm_per_360(0.0, 800).is_infinite());
        assert!((Settings::default().cm_per_360() - 25.977).abs() < 0.01);
    }

    #[test]
    fn interpolation_delay_is_automatic_or_manual() {
        let i = 1.0 / 30.0;
        assert!((auto_interpolation_delay(i, 0.0) - 2.0 * i).abs() < 1e-12);
        assert!((auto_interpolation_delay(i, 0.05) - (i + 0.1)).abs() < 1e-12);
        let mut s = Settings::default();
        assert!((s.interpolation_delay(0.0) - 2.0 / 30.0).abs() < 1e-9);
        s.set("interpolation_auto", Value::Bool(false)).unwrap();
        s.set("interpolation_delay_ms", Value::Int(150)).unwrap();
        assert!((s.interpolation_delay(0.5) - 0.15).abs() < 1e-9);
    }

    #[test]
    fn pseudonym_is_cleaned_and_new_categories_round_trip() {
        let mut s = Settings::default();
        s.network.pseudonym = "  a\u{7}bc".repeat(20);
        s.clean_pseudonym();
        assert!(s.network.pseudonym.chars().count() <= MAX_PSEUDONYM_CHARS);
        assert!(!s.network.pseudonym.chars().any(char::is_control));
        s.network.pseudonym = "   ".into();
        s.clean_pseudonym();
        assert_eq!(s.network.pseudonym, "Player");
        s.network.pseudonym = "Gab".into();
        s.set("snapshot_hz", Value::Int(100)).unwrap();
        assert_eq!(s.network.snapshot_hz, 60);
        s.set("language", Value::Choice(1)).unwrap();
        s.set("master_volume", Value::Float(0.3)).unwrap();
        s.bindings.swap(
            crate::bindings::Action::Jump,
            "KeyW".parse().unwrap(),
            crate::bindings::Action::Forward,
        );
        let path = temp("categories");
        save_settings(&path, &s).unwrap();
        assert_eq!(load_settings(&path), s);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn safe_mode_resets_display_only() {
        let mut s = Settings::default();
        s.set("window_mode", Value::Choice(2)).unwrap();
        s.set("fov", Value::Int(110)).unwrap();
        s.set("shadows", Value::Choice(0)).unwrap();
        let path = temp("safe");
        save_settings(&path, &s).unwrap();
        let safe = load_settings_with(&path, true);
        assert_eq!(safe.display, DisplaySettings::default());
        assert_eq!(safe.quality, s.quality);
        assert_eq!(load_settings_with(&path, false), s);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}

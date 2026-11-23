//! Applies display and quality settings to the running `faset` window and
//! renderer, through the existing interfaces of `faset-render` and
//! `faset-upscale` (lot 6.4).

use std::time::Duration;

use faset_audio::Engine;
use faset_core::settings::{
    AccessibilitySettings, AntiAliasing, Category, ControlSettings, GameSettings, NetworkSettings,
    SettingMeta, Settings, SettingsSink, ShadowQuality, UpscaleLevel, UpscalerChoice, WindowMode,
};
use faset_render::{PostSettings, Renderer};
use faset_upscale::{effective_quality, Fsr1, Quality, Registry, UpscaleBridge, UpscalerKind};
use winit::dpi::PhysicalSize;
use winit::window::{Fullscreen, Window};

/// Settings the frame loop reads every frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tuning {
    /// Vertical field of view in degrees.
    pub fov_deg: f32,
    /// Exposure multiplier.
    pub brightness: f32,
    /// Weather particle density, 0 to 1.
    pub weather_density: f32,
    /// Frame cap in frames per second (0: none).
    pub fps_cap: u32,
}

impl Tuning {
    /// The frame budget the cap asks for.
    pub fn frame_budget(&self) -> Option<Duration> {
        (self.fps_cap > 0).then(|| Duration::from_secs_f64(1.0 / f64::from(self.fps_cap)))
    }
}

/// What is installed in the renderer now.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Installed {
    kind: UpscalerKind,
    quality: Quality,
    sharpness_bits: u32,
}

/// Live state of the applied settings.
pub struct Live {
    /// Values read by the frame loop.
    pub tuning: Tuning,
    /// Upscalers this GPU can run (the menu offers only these).
    pub available: Vec<UpscalerChoice>,
    /// Controls read by the input code.
    pub controls: ControlSettings,
    /// Network values read when a session starts and by the interpolation.
    pub network: NetworkSettings,
    /// Game values read by the HUD and the weapon view.
    pub game: GameSettings,
    /// Accessibility values read by the camera, the post chain and the UI.
    pub accessibility: AccessibilitySettings,
    /// The audio engine, if a device could be opened.
    audio: Option<Engine>,
    registry: Registry<faset_render::GpuHandles>,
    installed: Option<Installed>,
}

fn new_audio(settings: &Settings) -> Option<Engine> {
    Engine::new(crate::audio::audio_config(settings))
        .map_err(|e| tracing::warn!("no audio engine: {e}"))
        .ok()
}

fn kind_of(choice: UpscalerChoice) -> UpscalerKind {
    match choice {
        UpscalerChoice::Native => UpscalerKind::Native,
        UpscalerChoice::Taa => UpscalerKind::Taa,
        UpscalerChoice::Fsr1 => UpscalerKind::Fsr1,
        UpscalerChoice::Fsr3 => UpscalerKind::Fsr3,
        UpscalerChoice::Dlss => UpscalerKind::Dlss,
    }
}

fn choice_of(kind: UpscalerKind) -> Option<UpscalerChoice> {
    Some(match kind {
        UpscalerKind::Native => UpscalerChoice::Native,
        UpscalerKind::Taa => UpscalerChoice::Taa,
        UpscalerKind::Fsr1 => UpscalerChoice::Fsr1,
        UpscalerKind::Fsr3 => UpscalerChoice::Fsr3,
        UpscalerKind::Dlss => UpscalerChoice::Dlss,
        _ => return None,
    })
}

/// The upscaler kind that really runs: TAA anti-aliasing with no upscaler
/// chosen uses the TAA pass (decision).
fn wanted_kind(settings: &Settings) -> UpscalerKind {
    let q = &settings.quality;
    if q.anti_aliasing == AntiAliasing::Taa && q.upscaler == UpscalerChoice::Native {
        UpscalerKind::Taa
    } else {
        kind_of(q.upscaler)
    }
}

fn wanted_quality(settings: &Settings, kind: UpscalerKind) -> Quality {
    let q = &settings.quality;
    let requested = match q.upscale_level {
        UpscaleLevel::Quality => Quality::Quality,
        UpscaleLevel::Balanced => Quality::Balanced,
        UpscaleLevel::Performance => Quality::Performance,
        UpscaleLevel::UltraPerformance => Quality::UltraPerformance,
        UpscaleLevel::Custom if q.render_scale >= 100 => Quality::Native,
        UpscaleLevel::Custom => Quality::Free(q.render_scale as f32 / 100.0),
    };
    effective_quality(kind, requested)
}

/// RCAS stops for the menu's 0..1 sharpness (1 is the sharpest).
fn rcas_stops(sharpness: f32) -> f32 {
    2.0 * (1.0 - sharpness.clamp(0.0, 1.0))
}

impl Live {
    /// Detects the usable upscalers and starts from the given settings.
    pub fn new(renderer: &Renderer, settings: &Settings) -> Self {
        let registry = Registry::builtin();
        let available = registry
            .available(&renderer.gpu_handles())
            .into_iter()
            .filter_map(choice_of)
            .collect();
        Self {
            tuning: tuning_of(settings),
            controls: settings.controls.clone(),
            network: settings.network.clone(),
            game: settings.game.clone(),
            accessibility: settings.accessibility.clone(),
            audio: new_audio(settings),
            available,
            registry,
            installed: None,
        }
    }

    /// Applies every setting (start-up). Returns the ids that took effect.
    pub fn apply_all(
        &mut self,
        renderer: &mut Renderer,
        window: &Window,
        settings: &Settings,
    ) -> Vec<&'static str> {
        let mut sink = RenderSink {
            live: self,
            renderer,
            window,
        };
        faset_core::settings::META
            .iter()
            .filter(|m| sink.apply(m, settings))
            .map(|m| m.id)
            .collect()
    }

    /// Applies what differs between `old` and `new`.
    pub fn apply_changes(
        &mut self,
        renderer: &mut Renderer,
        window: &Window,
        old: &Settings,
        new: &Settings,
    ) -> Vec<&'static str> {
        let mut sink = RenderSink {
            live: self,
            renderer,
            window,
        };
        faset_core::settings::apply_settings(old, new, &mut sink)
    }

    fn apply_upscaler(&mut self, renderer: &mut Renderer, settings: &Settings) -> bool {
        let kind = wanted_kind(settings);
        let quality = wanted_quality(settings, kind);
        let wanted = Installed {
            kind,
            quality,
            sharpness_bits: settings.quality.sharpness.to_bits(),
        };
        if self.installed == Some(wanted) {
            return true;
        }
        let gpu = renderer.gpu_handles();
        let pass = if kind == UpscalerKind::Fsr1 {
            Fsr1::new(&gpu).ok().map(|mut f| {
                f.set_sharpness(rcas_stops(settings.quality.sharpness));
                (
                    kind,
                    Box::new(UpscaleBridge::new(kind, Box::new(f), quality)) as _,
                )
            })
        } else {
            None
        };
        let created = match pass {
            Some(p) => Ok(p),
            None => self.registry.create_pass(kind, quality, &gpu),
        };
        match created {
            Ok((built, pass)) => {
                if built != kind {
                    tracing::warn!("{} is unavailable, {} is used", kind.name(), built.name());
                }
                if let Err(e) = renderer.set_upscaler(Some(pass)) {
                    tracing::error!("cannot install the upscaler: {e}");
                    return false;
                }
                self.installed = Some(Installed {
                    kind: built,
                    ..wanted
                });
                true
            }
            Err(e) => {
                tracing::error!("no upscaler could be built: {e}");
                false
            }
        }
    }
}

fn tuning_of(s: &Settings) -> Tuning {
    Tuning {
        fov_deg: s.display.fov as f32,
        brightness: s.display.brightness,
        weather_density: s.quality.weather_density,
        fps_cap: s.display.fps_cap,
    }
}

fn post_of(s: &Settings) -> PostSettings {
    let q = &s.quality;
    PostSettings {
        contours: q.outlines,
        bloom: q.bloom,
        tonemap: true,
        grading: true,
        screen_effects: q.screen_effects,
        fxaa: q.anti_aliasing == AntiAliasing::Fxaa,
    }
}

/// How far the sun casts shadows for a shadow quality, metres.
fn shadow_distance(quality: ShadowQuality) -> f32 {
    match quality {
        ShadowQuality::Off => 0.0,
        ShadowQuality::Low => faset_render::SHADOW_DISTANCE * 0.5,
        ShadowQuality::Medium => faset_render::SHADOW_DISTANCE,
        ShadowQuality::High => faset_render::SHADOW_DISTANCE * 1.5,
    }
}

/// Window mode, resolution and monitor.
pub fn apply_window(window: &Window, settings: &Settings) {
    let d = &settings.display;
    let monitor = window
        .available_monitors()
        .nth(d.monitor as usize)
        .or_else(|| window.current_monitor());
    let (w, h) = d.resolution.size();
    match d.window_mode {
        WindowMode::Windowed => {
            window.set_fullscreen(None);
            let _ = window.request_inner_size(PhysicalSize::new(w, h));
        }
        WindowMode::Borderless => window.set_fullscreen(Some(Fullscreen::Borderless(monitor))),
        WindowMode::Fullscreen => {
            let mode = monitor.as_ref().and_then(|m| {
                m.video_modes()
                    .filter(|v| v.size().width == w && v.size().height == h)
                    .max_by_key(|v| v.refresh_rate_millihertz())
            });
            window.set_fullscreen(Some(match mode {
                Some(mode) => Fullscreen::Exclusive(mode),
                None => Fullscreen::Borderless(monitor),
            }));
        }
    }
}

/// The settings sink of the `faset` executable.
pub struct RenderSink<'a> {
    live: &'a mut Live,
    renderer: &'a mut Renderer,
    window: &'a Window,
}

impl SettingsSink for RenderSink<'_> {
    fn apply(&mut self, meta: &SettingMeta, settings: &Settings) -> bool {
        match meta.id {
            "window_mode" | "resolution" | "monitor" => {
                apply_window(self.window, settings);
                true
            }
            "fps_cap" | "fov" | "brightness" | "weather_density" => {
                self.live.tuning = tuning_of(settings);
                true
            }
            "vsync" => {
                self.renderer.set_vsync(settings.display.vsync);
                true
            }
            "shadows" => {
                self.renderer
                    .set_shadow_distance(shadow_distance(settings.quality.shadows));
                true
            }
            "bloom" | "outlines" | "screen_effects" => self.apply_post(settings),
            "anti_aliasing" => {
                let post = self.apply_post(settings);
                self.live.apply_upscaler(self.renderer, settings) && post
            }
            "upscaler" | "upscale_level" | "render_scale" | "sharpness" => {
                self.live.apply_upscaler(self.renderer, settings)
            }
            "master_volume" | "effects_volume" | "ui_volume" | "key_sound" => {
                match self.live.audio.as_mut() {
                    Some(engine) => {
                        crate::audio::apply_volume(engine, meta.id, settings).unwrap_or(false)
                    }
                    None => false,
                }
            }
            "output_device" => {
                // A new engine on the chosen device (the audio crate cannot
                // switch device on a running engine).
                self.live.audio = new_audio(settings);
                self.live.audio.is_some()
            }
            "language" => {
                faset_ui::set_language(settings.game.language);
                self.live.game = settings.game.clone();
                true
            }
            _ if meta.category == Category::Controls => {
                self.live.controls = settings.controls.clone();
                true
            }
            _ if meta.category == Category::Network => {
                self.live.network = settings.network.clone();
                true
            }
            _ if meta.category == Category::Game => {
                self.live.game = settings.game.clone();
                true
            }
            _ if meta.category == Category::Accessibility => {
                self.live.accessibility = settings.accessibility.clone();
                true
            }
            // No runtime hook in the renderer's public interface yet: the
            // value is stored and takes effect when the renderer offers one.
            id => {
                tracing::warn!("setting `{id}` has no runtime hook in the renderer yet");
                false
            }
        }
    }
}

impl RenderSink<'_> {
    fn apply_post(&mut self, settings: &Settings) -> bool {
        self.renderer
            .set_post_settings(Some(post_of(settings)))
            .map_err(|e| tracing::error!("cannot update the post chain: {e}"))
            .is_ok()
    }
}

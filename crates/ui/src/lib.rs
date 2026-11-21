//! egui interface of Faset: pass 9 of the frame graph.
//!
//! `egui` is drawn at the display resolution, after the upscaler and the post
//! chain, through the renderer's [`UiPass`](faset_render::UiPass) hook, so the
//! render scale never touches it. The crate provides:
//!
//! - [`Ui`]: [`Ui::begin_frame`] takes the frame's input, [`Ui::draw`] runs
//!   one [`Screen`] (and the debug [`Overlay`]) and queues the result for the
//!   renderer. The application state stack that picks the screen belongs to
//!   phase 6;
//! - [`Theme`]: the colour tokens every screen uses;
//! - [`text`]: interface text looked up by translation key;
//! - [`Overlay`]: profiling, frame times, network graph and simulation
//!   statistics, shown or hidden by a key set in the configuration
//!   ([`UiConfig::overlay_key`]).
//!
//! ```ignore
//! let mut ui = faset_ui::Ui::new(&mut renderer, UiConfig::default(), Theme::default())?;
//! // every frame, before `renderer.render_frame`:
//! ui.begin_frame(FrameInput { size_px: renderer.size(), ..Default::default() });
//! ui.draw(&mut my_screen);
//! ```

mod error;
mod i18n;
mod overlay;
mod paint;
mod theme;

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Instant;

use egui::{Event, RawInput, ViewportId};
use faset_render::Renderer;
use serde::{Deserialize, Serialize};

pub use error::UiError;
pub use i18n::text;
pub use overlay::{NetSample, Overlay, SimStats, UiStats};
pub use paint::EguiPass;
pub use theme::Theme;

/// egui, re-exported so screens use the version this crate was built with.
pub use egui;

/// Settings of the UI, read from the user's TOML settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    /// Name of the key that shows and hides the debug overlay, as egui names
    /// keys (`"F3"`, `"Backtick"`, ...).
    pub overlay_key: String,
    /// Starts with the overlay visible.
    pub overlay_visible: bool,
    /// Interface scale: physical pixels per point is this times the
    /// platform's scale factor.
    pub scale: f32,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            overlay_key: "F3".to_owned(),
            overlay_visible: false,
            scale: 1.0,
        }
    }
}

/// Input of one frame.
#[derive(Debug, Clone)]
pub struct FrameInput {
    /// Size of the display target in physical pixels.
    pub size_px: (u32, u32),
    /// The platform's scale factor (window DPI scaling).
    pub scale_factor: f32,
    /// Seconds since the application started.
    pub time: f64,
    /// Seconds since the previous frame.
    pub dt: f32,
    /// Input events (pointer, keys, text), in egui's form.
    pub events: Vec<Event>,
}

impl Default for FrameInput {
    fn default() -> Self {
        Self {
            size_px: (1280, 720),
            scale_factor: 1.0,
            time: 0.0,
            dt: 1.0 / 60.0,
            events: Vec::new(),
        }
    }
}

/// One interface screen. The state stack of phase 6 picks which one is drawn.
pub trait Screen {
    /// Adds the screen's widgets for this frame.
    fn show(&mut self, ctx: &egui::Context, theme: &Theme);
}

/// A screen with nothing on it (used under the overlay, and in tests).
pub struct EmptyScreen;

impl Screen for EmptyScreen {
    fn show(&mut self, _ctx: &egui::Context, _theme: &Theme) {}
}

/// The egui context, its theme, the overlay and the link to the renderer.
pub struct Ui {
    ctx: egui::Context,
    theme: Theme,
    config: UiConfig,
    overlay_key: egui::Key,
    overlay: Overlay,
    shared: Arc<paint::Shared>,
    pending: Option<RawInput>,
    pixels_per_point: f32,
    cpu_ms: f32,
    pinned: Option<(UiStats, Option<f32>)>,
    /// Duration of the last puffin frame in microseconds (0: none yet).
    profiler_us: Arc<AtomicU32>,
    sink: puffin::FrameSinkId,
}

impl Ui {
    /// Creates the UI and installs its paint pass in `renderer`.
    pub fn new(renderer: &mut Renderer, config: UiConfig, theme: Theme) -> Result<Self, UiError> {
        let overlay_key = egui::Key::from_name(&config.overlay_key)
            .ok_or_else(|| UiError::UnknownKey(config.overlay_key.clone()))?;
        let shared = Arc::new(paint::Shared::default());
        let pass = EguiPass::new(&renderer.gpu_handles(), shared.clone())?;
        renderer.set_ui_pass(Some(Box::new(pass)))?;

        let ctx = egui::Context::default();
        theme.apply(&ctx);
        let profiler_us = Arc::new(AtomicU32::new(0));
        let sink = {
            let us = profiler_us.clone();
            puffin::GlobalProfiler::lock().add_sink(Box::new(move |frame| {
                us.store(
                    (frame.duration_ns() / 1000).max(1) as u32,
                    Ordering::Relaxed,
                );
            }))
        };
        let mut overlay = Overlay::default();
        overlay.set_visible(config.overlay_visible);
        Ok(Self {
            ctx,
            theme,
            config,
            overlay_key,
            overlay,
            shared,
            pending: None,
            pixels_per_point: 1.0,
            cpu_ms: 0.0,
            pinned: None,
            profiler_us,
            sink,
        })
    }

    /// Starts a frame: stores the input, handles the overlay toggle key and
    /// records the frame time.
    pub fn begin_frame(&mut self, input: FrameInput) {
        puffin::GlobalProfiler::lock().new_frame();
        for event in &input.events {
            if let Event::Key {
                key,
                pressed: true,
                repeat: false,
                ..
            } = event
            {
                if *key == self.overlay_key {
                    self.overlay.toggle();
                }
            }
        }
        self.overlay.record_frame_time(input.dt);
        let us = self.profiler_us.load(Ordering::Relaxed);
        let profiler_ms = match self.pinned {
            Some((_, ms)) => ms,
            None => (us > 0).then(|| us as f32 / 1000.0),
        };
        self.overlay.set_profiler_frame_ms(profiler_ms);

        self.pixels_per_point = (input.scale_factor * self.config.scale).max(0.25);
        let mut raw = RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(input.size_px.0 as f32, input.size_px.1 as f32) / self.pixels_per_point,
            )),
            time: Some(input.time),
            predicted_dt: input.dt,
            events: input.events,
            ..RawInput::default()
        };
        raw.viewports
            .entry(ViewportId::ROOT)
            .or_default()
            .native_pixels_per_point = Some(self.pixels_per_point);
        self.pending = Some(raw);
    }

    /// Runs `screen` and the overlay, tessellates the result and queues it for
    /// the renderer's next frame.
    pub fn draw(&mut self, screen: &mut dyn Screen) {
        puffin::profile_function!();
        let started = Instant::now();
        let raw = self.pending.take().unwrap_or_default();
        self.ctx.begin_pass(raw);
        screen.show(&self.ctx, &self.theme);
        let shown = self.pinned.map_or_else(|| self.stats(), |(stats, _)| stats);
        self.overlay.show(&self.ctx, &self.theme, shown);
        let output = self.ctx.end_pass();
        let primitives = self.ctx.tessellate(output.shapes, output.pixels_per_point);
        self.shared.submit(paint::PaintData {
            primitives,
            textures: output.textures_delta,
            pixels_per_point: output.pixels_per_point,
        });
        self.cpu_ms = started.elapsed().as_secs_f32() * 1000.0;
    }

    /// The debug overlay (record network samples and simulation statistics
    /// into it).
    pub fn overlay(&mut self) -> &mut Overlay {
        &mut self.overlay
    }

    /// Makes the overlay show fixed readings (UI cost and last profiled frame
    /// in milliseconds) instead of live ones, so screenshots and reference
    /// images are reproducible. `None` goes back to live readings.
    pub fn pin_readings(&mut self, readings: Option<(UiStats, Option<f32>)>) {
        self.pinned = readings;
    }

    /// The theme in use.
    pub fn theme(&self) -> &Theme {
        &self.theme
    }

    /// Replaces the theme.
    pub fn set_theme(&mut self, theme: Theme) {
        theme.apply(&self.ctx);
        self.theme = theme;
    }

    /// The egui context, for screens that need it outside [`Screen::show`].
    pub fn context(&self) -> &egui::Context {
        &self.ctx
    }

    /// What the UI cost: CPU time of the last [`Ui::draw`], draw calls and
    /// vertices of the last pass the renderer executed.
    pub fn stats(&self) -> UiStats {
        UiStats {
            cpu_ms: self.cpu_ms,
            draw_calls: self.shared.stats.draw_calls.load(Ordering::Relaxed),
            vertices: self.shared.stats.vertices.load(Ordering::Relaxed),
        }
    }
}

impl Drop for Ui {
    fn drop(&mut self) {
        puffin::GlobalProfiler::lock().remove_sink(self.sink);
    }
}

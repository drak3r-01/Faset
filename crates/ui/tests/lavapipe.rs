//! Headless UI tests on Mesa lavapipe (run by `cargo xtask render-test`).
//!
//! Every test asserts zero validation messages, counted twice: by the
//! renderer's debug messenger, and by a tracing layer that also sees messages
//! emitted while the device and instance are destroyed.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use ash::vk;
use faset_render::{
    FrameCtx, ImageHandle, RenderConfig, RenderWorld, Renderer, UiPass, UpscaleInputs, UpscalePass,
};
use faset_ui::egui;
use faset_ui::{
    EmptyScreen, FrameInput, NetSample, Screen, SimStats, Theme, Ui, UiConfig, UiStats,
};
use tracing::subscriber::DefaultGuard;
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::Layer;

/// Display size of the reference images.
const DISPLAY: (u32, u32) = (640, 480);

/// Counts events logged by the renderer's Vulkan debug callback.
struct VulkanMessages(Arc<AtomicUsize>);

impl<S: tracing::Subscriber> Layer<S> for VulkanMessages {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        if event.metadata().target() == "vulkan" {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
}

fn capture_messages() -> (Arc<AtomicUsize>, DefaultGuard) {
    let count = Arc::new(AtomicUsize::new(0));
    let subscriber = tracing_subscriber::registry()
        .with(VulkanMessages(count.clone()))
        .with(tracing_subscriber::fmt::layer().with_test_writer());
    (count, tracing::subscriber::set_default(subscriber))
}

fn headless(size: (u32, u32)) -> Renderer {
    let renderer = Renderer::new_headless(&RenderConfig {
        size,
        ..RenderConfig::default()
    })
    .expect("headless renderer on lavapipe");
    assert!(
        renderer.validation_enabled() || !cfg!(debug_assertions),
        "validation layer must be active in test builds"
    );
    renderer
}

fn out_path(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("faset-ui-tests");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(format!("{}-{name}", std::process::id()))
}

fn reference(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/refs/{name}.png"))
}

fn load_rgba(path: &Path) -> image::RgbaImage {
    image::open(path)
        .unwrap_or_else(|e| panic!("cannot open {}: {e}", path.display()))
        .to_rgba8()
}

fn max_channel_error(a: &image::RgbaImage, b: &image::RgbaImage) -> u8 {
    assert_eq!(a.dimensions(), b.dimensions(), "image sizes differ");
    a.as_raw()
        .iter()
        .zip(b.as_raw())
        .map(|(x, y)| x.abs_diff(*y))
        .max()
        .unwrap_or(0)
}

/// A window with a few widgets, to exercise text, fills and clipping.
struct DemoScreen;

impl Screen for DemoScreen {
    fn show(&mut self, ctx: &egui::Context, theme: &Theme) {
        egui::Window::new("Demo")
            .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-8.0, -8.0))
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label("Interface at display resolution");
                ui.colored_label(theme.accent, "Accent colour");
                let _ = ui.button("Button");
                let mut on = true;
                ui.checkbox(&mut on, "Checkbox");
            });
    }
}

fn fixed_ui(renderer: &mut Renderer, overlay: bool) -> Ui {
    let mut ui = Ui::new(
        renderer,
        UiConfig {
            overlay_visible: overlay,
            ..UiConfig::default()
        },
        Theme::default(),
    )
    .expect("UI on lavapipe");
    ui.pin_readings(Some((
        UiStats {
            cpu_ms: 0.5,
            draw_calls: 12,
            vertices: 3456,
        },
        Some(2.5),
    )));
    ui
}

/// Input at `second` seconds: egui fades windows in over a fraction of a
/// second, so the references are taken a few seconds in.
fn input(second: u32) -> FrameInput {
    FrameInput {
        size_px: DISPLAY,
        time: f64::from(second),
        ..FrameInput::default()
    }
}

/// Renders `frames` frames (so every frame-in-flight slot is reused) and
/// writes the last to `path`.
fn render_ui(renderer: &mut Renderer, ui: &mut Ui, screen: &mut dyn Screen, path: &Path) {
    for second in 20..23 {
        ui.begin_frame(input(second));
        ui.draw(screen);
        renderer.render_frame(&RenderWorld::default()).unwrap();
    }
    renderer.read_back_png(path).unwrap();
}

fn feed_overlay(ui: &mut Ui) {
    let overlay = ui.overlay();
    for i in 0..60 {
        overlay.record_network(NetSample {
            rtt_ms: 30.0 + 20.0 * ((i as f32) * 0.2).sin(),
            in_kbps: 120.0,
            out_kbps: 24.0,
            loss: 0.01,
        });
    }
    overlay.set_sim_stats(Some(SimStats {
        tick: 1234,
        tick_ms: 0.75,
        entities: 96,
        players: 8,
    }));
}

/// Compares `got` with the reference `name`; max channel error of 2.
fn assert_matches_reference(got: &Path, name: &str) {
    let error = max_channel_error(&load_rgba(got), &load_rgba(&reference(name)));
    assert!(
        error <= 2,
        "{name}: max channel error {error} (output: {})",
        got.display()
    );
}

#[test]
fn ui_frame_matches_its_reference() {
    let (messages, _guard) = capture_messages();
    let out = out_path("demo.png");
    {
        let mut renderer = headless(DISPLAY);
        let mut ui = fixed_ui(&mut renderer, false);
        render_ui(&mut renderer, &mut ui, &mut DemoScreen, &out);
        assert!(ui.stats().draw_calls > 0, "the UI pass drew nothing");
        assert!(
            ui.stats().draw_calls < 500,
            "{} draw calls",
            ui.stats().draw_calls
        );
        assert_eq!(renderer.validation_message_count(), 0);
    }
    assert_matches_reference(&out, "demo");
    assert_eq!(messages.load(Ordering::SeqCst), 0, "validation messages");
}

/// Renders the overlay off, presses the toggle key, and renders it on. The
/// reference image comes from this same sequence (see `bless_references`).
fn run_overlay(renderer: &mut Renderer, off: &Path, on: &Path) {
    let mut ui = fixed_ui(renderer, false);
    feed_overlay(&mut ui);
    render_ui(renderer, &mut ui, &mut EmptyScreen, off);
    assert_eq!(ui.stats().draw_calls, 0, "overlay off must draw nothing");

    // The toggle key comes from the configuration (F3 by default).
    let mut press = input(10);
    press.events.push(egui::Event::Key {
        key: egui::Key::F3,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    });
    ui.begin_frame(press);
    ui.draw(&mut EmptyScreen);
    assert!(ui.overlay().is_visible());
    renderer.render_frame(&RenderWorld::default()).unwrap();
    render_ui(renderer, &mut ui, &mut EmptyScreen, on);
    assert!(ui.stats().draw_calls > 0);
}

#[test]
fn overlay_is_absent_when_off_and_present_when_on() {
    let (messages, _guard) = capture_messages();
    let (off, on, bare) = (
        out_path("overlay_off.png"),
        out_path("overlay_on.png"),
        out_path("bare.png"),
    );
    {
        let mut renderer = headless(DISPLAY);
        // No UI at all: the frame the overlay must leave untouched.
        renderer.render_frame(&RenderWorld::default()).unwrap();
        renderer.read_back_png(&bare).unwrap();

        run_overlay(&mut renderer, &off, &on);
        assert_eq!(renderer.validation_message_count(), 0);
    }
    let (bare_img, off_img, on_img) = (load_rgba(&bare), load_rgba(&off), load_rgba(&on));
    assert_eq!(bare_img, off_img, "overlay off changes the frame");
    assert_ne!(off_img, on_img, "overlay on changes nothing");
    assert_matches_reference(&on, "overlay_on");
    assert_eq!(messages.load(Ordering::SeqCst), 0, "validation messages");
}

#[test]
fn an_unknown_toggle_key_is_an_error() {
    let mut renderer = headless(DISPLAY);
    let result = Ui::new(
        &mut renderer,
        UiConfig {
            overlay_key: "NoSuchKey".to_owned(),
            ..UiConfig::default()
        },
        Theme::default(),
    );
    assert!(matches!(result, Err(faset_ui::UiError::UnknownKey(_))));
}

#[test]
fn ui_cpu_time_is_within_budget() {
    let mut renderer = headless(DISPLAY);
    let mut ui = fixed_ui(&mut renderer, true);
    feed_overlay(&mut ui);
    // Warm up: the first frame builds the font atlas.
    for second in 0..3 {
        ui.begin_frame(input(second));
        ui.draw(&mut DemoScreen);
    }
    let started = std::time::Instant::now();
    let frames = 20;
    for second in 0..frames {
        ui.begin_frame(input(10 + second));
        ui.draw(&mut DemoScreen);
    }
    let per_frame = started.elapsed().as_secs_f32() * 1000.0 / frames as f32;
    println!("UI CPU time per frame: {per_frame:.3} ms");
    // The 4 ms budget covers the whole frame in release builds; a debug
    // build of egui is an order of magnitude slower.
    let limit = if cfg!(debug_assertions) { 40.0 } else { 4.0 };
    assert!(per_frame < limit, "{per_frame} ms per frame");
}

/// What the passes of one frame recorded, in order.
type Log = Arc<Mutex<Vec<(&'static str, vk::Extent2D)>>>;

/// An upscaler that renders at half size and records when it runs.
struct SpyUpscaler(Log);

impl UpscalePass for SpyUpscaler {
    fn render_extent(&self, display: vk::Extent2D) -> vk::Extent2D {
        vk::Extent2D {
            width: display.width / 2,
            height: display.height / 2,
        }
    }
    fn jitter(&self, _frame_index: u32) -> faset_render::glam::Vec2 {
        faset_render::glam::Vec2::ZERO
    }
    fn is_temporal(&self) -> bool {
        false
    }
    fn execute(&mut self, _ctx: &mut FrameCtx<'_>, inputs: &UpscaleInputs) -> ImageHandle {
        self.0
            .lock()
            .unwrap()
            .push(("upscale", inputs.colour.extent));
        // The scene is returned as is: the renderer stretches it.
        inputs.colour
    }
}

/// A UI pass that records when it runs and the extent it is given.
struct SpyUi(Log);

impl UiPass for SpyUi {
    fn execute(&mut self, _ctx: &mut FrameCtx<'_>, target: &ImageHandle) {
        self.0.lock().unwrap().push(("ui", target.extent));
    }
}

#[test]
fn the_ui_pass_follows_the_upscale_pass_at_display_resolution() {
    let (messages, _guard) = capture_messages();
    let log: Log = Log::default();
    {
        let mut renderer = headless(DISPLAY);
        renderer
            .set_upscaler(Some(Box::new(SpyUpscaler(log.clone()))))
            .unwrap();
        renderer
            .set_ui_pass(Some(Box::new(SpyUi(log.clone()))))
            .unwrap();
        renderer.render_frame(&RenderWorld::default()).unwrap();
        assert_eq!(renderer.validation_message_count(), 0);
    }
    let log = log.lock().unwrap();
    let names: Vec<_> = log.iter().map(|(n, _)| *n).collect();
    assert_eq!(names, ["upscale", "ui"], "frame order");
    // The scene is drawn at half size, the UI at the display size.
    assert_eq!(log[0].1.width, DISPLAY.0 / 2);
    assert_eq!(
        (log[1].1.width, log[1].1.height),
        (DISPLAY.0, DISPLAY.1),
        "the render scale must not affect the UI"
    );
    assert_eq!(messages.load(Ordering::SeqCst), 0, "validation messages");
}

/// Regenerates the references from lavapipe renders. Inspect the images
/// before committing them.
#[test]
#[ignore = "regenerates tests/refs/*.png"]
fn bless_references() {
    let mut renderer = headless(DISPLAY);
    let mut ui = fixed_ui(&mut renderer, false);
    render_ui(&mut renderer, &mut ui, &mut DemoScreen, &reference("demo"));
    drop(ui);
    drop(renderer);

    let mut renderer = headless(DISPLAY);
    run_overlay(
        &mut renderer,
        &out_path("overlay_off_unused.png"),
        &reference("overlay_on"),
    );
}

//! The Faset client: a window, the renderer, the egui menus and the
//! application state stack of `faset-client`.
//!
//! `faset [--maps DIR] [--seconds N] [--no-vsync]`
//!
//! The main menu runs over a slowly rotating view of a map. The play screen
//! starts an integrated server and joins it; the match is shown from the
//! local player's eye with no controls yet (lot 6.1 delivers the application
//! layer: the playable first-person loop comes later). `Escape` opens and
//! closes the pause menu during a match.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use faset_client::{App, MapCatalogue};
use faset_core::Tick;
use faset_minigame_api::Registry;
use faset_mode_fps::FpsMode;
use faset_mode_race::RaceMode;
use faset_protocol::InputCommand;
use faset_render::glam::{Mat4, Vec3};
use faset_render::{
    perspective_reverse_z, AmbienceProfile, Camera, Instance, RenderConfig, RenderWorld, Renderer,
    SkyParams,
};
use faset_ui::egui::{self, Event, PointerButton};
use faset_ui::{AppState, FrameInput, StateScreen, Theme, Ui, UiConfig};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

/// Seconds for one turn of the menu camera.
const MENU_TURN_SECONDS: f32 = 120.0;
/// Eye height above the capsule centre, metres.
const EYE_OFFSET: f32 = 0.7;

struct Client {
    app: App,
    vsync: bool,
    limit: Option<Duration>,
    start: Instant,
    last_frame: Instant,
    events: Vec<Event>,
    pointer_px: (f32, f32),
    instances: Vec<Instance>,
    prev_view_proj: Option<Mat4>,
    // Field order: UI and renderer are dropped before the window.
    ui: Option<Ui>,
    renderer: Option<Renderer>,
    window: Option<Window>,
}

fn registry() -> Registry {
    let mut registry = Registry::new();
    registry.register::<FpsMode>().expect("fps mode registers");
    registry
        .register::<RaceMode>()
        .expect("race mode registers");
    registry
}

fn idle(tick: Tick) -> InputCommand {
    InputCommand {
        tick,
        buttons: 0,
        move_x: 0,
        move_y: 0,
        yaw: 0,
        pitch: 0,
        weapon_slot: 1,
    }
}

fn egui_key(key: &Key) -> Option<egui::Key> {
    Some(match key {
        Key::Named(NamedKey::Escape) => egui::Key::Escape,
        Key::Named(NamedKey::Enter) => egui::Key::Enter,
        Key::Named(NamedKey::Tab) => egui::Key::Tab,
        Key::Named(NamedKey::Backspace) => egui::Key::Backspace,
        Key::Named(NamedKey::Delete) => egui::Key::Delete,
        Key::Named(NamedKey::ArrowLeft) => egui::Key::ArrowLeft,
        Key::Named(NamedKey::ArrowRight) => egui::Key::ArrowRight,
        Key::Named(NamedKey::ArrowUp) => egui::Key::ArrowUp,
        Key::Named(NamedKey::ArrowDown) => egui::Key::ArrowDown,
        Key::Named(NamedKey::Home) => egui::Key::Home,
        Key::Named(NamedKey::End) => egui::Key::End,
        Key::Named(NamedKey::F3) => egui::Key::F3,
        _ => return None,
    })
}

impl Client {
    fn create(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        let attrs = Window::default_attributes()
            .with_title("Faset")
            .with_inner_size(winit::dpi::PhysicalSize::new(1600, 900));
        let window = event_loop
            .create_window(attrs)
            .context("cannot create the window")?;
        let size = window.inner_size();
        let mut renderer = Renderer::new_windowed(
            &window,
            &RenderConfig {
                vsync: self.vsync,
                size: (size.width.max(1), size.height.max(1)),
                ..RenderConfig::default()
            },
        )
        .context("cannot create the renderer")?;
        let ui = Ui::new(&mut renderer, UiConfig::default(), Theme::default())
            .context("cannot create the UI")?;
        self.ui = Some(ui);
        self.renderer = Some(renderer);
        self.window = Some(window);
        Ok(())
    }

    fn on_window_event(&mut self, event: WindowEvent, event_loop: &ActiveEventLoop) {
        let ppp = self
            .window
            .as_ref()
            .map_or(1.0, |w| w.scale_factor() as f32);
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(r) = &mut self.renderer {
                    r.resize(size.width, size.height);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.pointer_px = (position.x as f32, position.y as f32);
                self.events.push(Event::PointerMoved(egui::pos2(
                    position.x as f32 / ppp,
                    position.y as f32 / ppp,
                )));
            }
            WindowEvent::CursorLeft { .. } => self.events.push(Event::PointerGone),
            WindowEvent::MouseInput { state, button, .. } => {
                let button = match button {
                    MouseButton::Left => PointerButton::Primary,
                    MouseButton::Right => PointerButton::Secondary,
                    MouseButton::Middle => PointerButton::Middle,
                    _ => return,
                };
                self.events.push(Event::PointerButton {
                    pos: egui::pos2(self.pointer_px.0 / ppp, self.pointer_px.1 / ppp),
                    button,
                    pressed: state == ElementState::Pressed,
                    modifiers: egui::Modifiers::NONE,
                });
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (unit, delta) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => {
                        (egui::MouseWheelUnit::Line, egui::vec2(x, y))
                    }
                    MouseScrollDelta::PixelDelta(p) => (
                        egui::MouseWheelUnit::Point,
                        egui::vec2(p.x as f32 / ppp, p.y as f32 / ppp),
                    ),
                };
                self.events.push(Event::MouseWheel {
                    unit,
                    delta,
                    phase: egui::TouchPhase::Move,
                    source: egui::MouseWheelSource::Wheel,
                    modifiers: egui::Modifiers::NONE,
                });
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let pressed = event.state == ElementState::Pressed;
                if pressed && event.logical_key == Key::Named(NamedKey::Escape) && !event.repeat {
                    self.toggle_pause();
                }
                if let Some(key) = egui_key(&event.logical_key) {
                    self.events.push(Event::Key {
                        key,
                        physical_key: None,
                        pressed,
                        repeat: event.repeat,
                        modifiers: egui::Modifiers::NONE,
                    });
                }
                if pressed {
                    if let Some(text) = event
                        .text
                        .as_ref()
                        .filter(|t| !t.chars().any(char::is_control))
                    {
                        self.events.push(Event::Text(text.to_string()));
                    }
                }
            }
            WindowEvent::RedrawRequested => self.redraw(event_loop),
            _ => {}
        }
    }

    fn toggle_pause(&mut self) {
        match self.app.stack().top() {
            AppState::InGame => {
                let _ = self.app.pause();
            }
            AppState::Pause => self.app.handle(vec![faset_ui::UiCommand::Pop]),
            _ => {}
        }
    }

    fn camera(&mut self, aspect: f32, t: f32) -> Camera {
        let proj = perspective_reverse_z(70f32.to_radians(), aspect, 0.1);
        let in_match = matches!(self.app.stack().states().first(), Some(AppState::InGame));
        let (eye, target) = match self.app.session().and_then(|s| s.display_position()) {
            Some(position) if in_match => {
                let eye = position + Vec3::Y * EYE_OFFSET;
                (eye, eye - Vec3::Z)
            }
            _ => {
                let (center, radius) = self.app.map_extent();
                let angle = t * std::f32::consts::TAU / MENU_TURN_SECONDS;
                let eye = center + Vec3::new(angle.cos(), 0.55, angle.sin()) * radius * 0.9;
                (eye, center)
            }
        };
        let view = faset_render::glam::camera::rh::view::look_at_mat4(eye, target, Vec3::Y);
        let mut camera = Camera::new(view, proj);
        if let Some(prev) = self.prev_view_proj {
            camera.prev_view_proj = prev;
        }
        self.prev_view_proj = Some(camera.view_proj());
        camera
    }

    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        let dt = now - self.last_frame;
        self.last_frame = now;
        let elapsed = self.start.elapsed();

        self.app.update(dt, &mut idle);
        if self.app.quit_requested() || self.limit.is_some_and(|l| elapsed >= l) {
            event_loop.exit();
            return;
        }
        let (Some(renderer), Some(ui)) = (&mut self.renderer, &mut self.ui) else {
            return;
        };
        if let Some(map) = self.app.take_map() {
            match map.upload(renderer) {
                Ok(instances) => self.instances = instances,
                Err(e) => tracing::error!("cannot upload the map: {e}"),
            }
        }
        if let Some(status) = self.app.take_status() {
            tracing::warn!("{status}");
        }

        let size_px = renderer.size();
        let state = self.app.stack().top();
        ui.begin_frame(FrameInput {
            size_px,
            scale_factor: self
                .window
                .as_ref()
                .map_or(1.0, |w| w.scale_factor() as f32),
            time: elapsed.as_secs_f64(),
            dt: dt.as_secs_f32(),
            events: std::mem::take(&mut self.events),
        });
        let commands = {
            let mut screen = StateScreen::new(state, self.app.model_mut());
            ui.draw(&mut screen);
            screen.commands
        };
        self.app.handle(commands);

        let aspect = size_px.0.max(1) as f32 / size_px.1.max(1) as f32;
        let mut camera = self.camera(aspect, elapsed.as_secs_f32());
        let Some(renderer) = &mut self.renderer else {
            return;
        };
        camera.jitter_px = renderer.upscaler_jitter(renderer.frames_rendered() as u32);
        let world = RenderWorld {
            camera,
            instances: self.instances.clone(),
            ambience: AmbienceProfile::builtin("day").expect("built-in profile"),
            sky: SkyParams {
                time_h: 15.0,
                latitude_deg: 45.0,
                ..SkyParams::default()
            },
            draw_sky: true,
            ..RenderWorld::default()
        };
        if let Err(e) = renderer.render_frame(&world) {
            tracing::error!("frame failed: {e}");
            event_loop.exit();
        }
    }
}

impl ApplicationHandler for Client {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        if let Err(e) = self.create(event_loop) {
            tracing::error!("{e:#}");
            event_loop.exit();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        self.on_window_event(event, event_loop);
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .map(String::as_str)
    };
    let maps_dir = value("--maps").map_or_else(|| PathBuf::from("assets/maps"), PathBuf::from);
    let seconds = value("--seconds")
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);

    let event_loop = EventLoop::new().context("cannot create the event loop")?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut client = Client {
        app: App::new(registry(), MapCatalogue::scan(&maps_dir)),
        vsync: !args.iter().any(|a| a == "--no-vsync"),
        limit: (seconds > 0).then(|| Duration::from_secs(seconds)),
        start: Instant::now(),
        last_frame: Instant::now(),
        events: Vec::new(),
        pointer_px: (0.0, 0.0),
        instances: Vec::new(),
        prev_view_proj: None,
        ui: None,
        renderer: None,
        window: None,
    };
    event_loop.run_app(&mut client)?;
    tracing::info!("exited cleanly");
    Ok(())
}

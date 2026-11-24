//! The Faset client: a window, the renderer, the egui menus and the
//! application state stack of `faset-client`.
//!
//! `faset [--maps DIR] [--seconds N] [--no-vsync] [--safe] [--settings FILE]`
//!
//! `--safe` resets the display settings to their defaults; `--settings` picks
//! another `settings.toml` than the one in the OS configuration folder.
//!
//! The main menu runs over a slowly rotating view of a map. The play screen
//! starts an integrated server and joins it (or joins a server by address or
//! from the LAN list); the match is played from the first-person view with
//! the keys and mouse of the bindings, the HUD, the other players, the
//! pickups and the weapon in hand (lot 6.6). `Escape` opens and closes the
//! pause menu during a match.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use faset_client::{
    App, EguiInput, FirstPersonCamera, FpsHud, GameInput, HudOptions, LocalFrame, MapCatalogue,
    MapLook, MatchScene, RawInput, SceneFrame, SurfaceMap,
};
use faset_core::bindings::Input;
use faset_core::settings::{self, ConfirmState, ResolutionConfirm, Settings};
use faset_minigame_api::Registry;
use faset_mode_fps::{register_modes, FpsMode};
use faset_mode_race::RaceMode;
use faset_render::glam::{Mat4, Quat, Vec3};
use faset_render::{perspective_reverse_z, Camera, Instance, RenderConfig, RenderWorld, Renderer};
use faset_ui::egui::{self, Event};
use faset_ui::{
    AppState, FrameInput, Screen, SettingsEvent, SettingsScreen, StateScreen, Theme, Ui, UiConfig,
};
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{CursorGrabMode, Window, WindowId};

#[path = "faset_settings/apply.rs"]
mod apply;
#[path = "faset_settings/audio.rs"]
mod audio;

/// Seconds for one turn of the menu camera.
const MENU_TURN_SECONDS: f32 = 120.0;
/// Longest frame the simulation is asked to catch up on, seconds.
const MAX_FRAME_SECONDS: f32 = 0.25;

struct Client {
    app: App,
    /// Settings being edited, and the last ones applied.
    settings: Settings,
    applied: Settings,
    settings_path: Option<PathBuf>,
    settings_tab: usize,
    rebind: faset_ui::RebindView,
    confirm: Option<ResolutionConfirm>,
    live: Option<apply::Live>,
    limit: Option<Duration>,
    start: Instant,
    last_frame: Instant,
    /// Window events turned into egui events and game input.
    input: EguiInput,
    /// The player's keys, mouse and look.
    game: GameInput,
    /// Session whose spawn orientation the look was taken from.
    look_from: Option<u64>,
    camera: FirstPersonCamera,
    /// The mouse is grabbed and hidden: the match is being played.
    grabbed: bool,
    focused: bool,
    instances: Vec<Instance>,
    /// What the map's theme asks of the renderer (ambience, sky, weather,
    /// water, lights), and when the map was loaded.
    look: MapLook,
    look_since: Instant,
    /// Meshes of the loadout screen's character, uploaded on first use.
    preview: Option<faset_client::CharacterPreview>,
    /// Boxes of the match scene, uploaded on first use.
    scene: Option<MatchScene>,
    /// What each collision volume of the map is made of (impacts, footsteps).
    surfaces: Arc<SurfaceMap>,
    /// Camera shake of the last frame (shots, blasts, damage).
    shake: Quat,
    /// Directions of the damage taken, for the HUD.
    damage: Vec<faset_client::DamageArc>,
    prev_view_proj: Option<Mat4>,
    // Field order: UI and renderer are dropped before the window.
    ui: Option<Ui>,
    renderer: Option<Renderer>,
    window: Option<Window>,
}

fn registry() -> Registry {
    let mut registry = Registry::new();
    // The playable modes first (they take bots), then the core test mode.
    register_modes(&mut registry).expect("fps modes register");
    registry.register::<FpsMode>().expect("fps mode registers");
    registry
        .register::<RaceMode>()
        .expect("race mode registers");
    registry
}

/// The HUD and the pause menu share one frame: the HUD under, the screen of
/// the state on top.
struct Composite<'a> {
    hud: Option<FpsHud>,
    top: &'a mut dyn Screen,
}

impl Screen for Composite<'_> {
    fn show(&mut self, ctx: &egui::Context, theme: &Theme) {
        if let Some(hud) = &mut self.hud {
            hud.show(ctx, theme);
        }
        self.top.show(ctx, theme);
    }
}

impl Client {
    fn create(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        let attrs = Window::default_attributes()
            .with_title("Faset")
            .with_inner_size({
                let (w, h) = self.settings.display.resolution.size();
                winit::dpi::PhysicalSize::new(w, h)
            });
        let window = event_loop
            .create_window(attrs)
            .context("cannot create the window")?;
        let size = window.inner_size();
        let mut renderer = Renderer::new_windowed(
            &window,
            &RenderConfig {
                vsync: self.settings.display.vsync,
                size: (size.width.max(1), size.height.max(1)),
                ..RenderConfig::default()
            },
        )
        .context("cannot create the renderer")?;
        let ui = Ui::new(&mut renderer, UiConfig::default(), Theme::default())
            .context("cannot create the UI")?;
        let mut live = apply::Live::new(&renderer, &self.settings);
        let applied = live.apply_all(&mut renderer, &window, &self.settings);
        tracing::info!("applied {} settings at start-up", applied.len());
        faset_ui::set_language(self.settings.game.language);
        let mut ui = ui;
        ui.set_scale(self.settings.accessibility.ui_scale);
        ui.overlay().set_visible(self.settings.network.net_graph);
        self.live = Some(live);
        self.ui = Some(ui);
        self.renderer = Some(renderer);
        self.window = Some(window);
        Ok(())
    }

    /// Physical pixels per egui point.
    fn points_per_pixel(&self) -> f32 {
        let factor = self
            .window
            .as_ref()
            .map_or(1.0, |w| w.scale_factor() as f32);
        factor * self.settings.accessibility.ui_scale
    }

    /// Whether the keyboard and mouse drive the player.
    fn playing(&self) -> bool {
        self.focused
            && self.app.accepts_game_input()
            && self.app.session().is_some_and(|s| s.in_game())
    }

    fn on_window_event(&mut self, event: WindowEvent, event_loop: &ActiveEventLoop) {
        match &event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(r) = &mut self.renderer {
                    r.resize(size.width, size.height);
                }
            }
            WindowEvent::Focused(focused) => {
                self.focused = *focused;
                if !focused {
                    self.game.release_all();
                }
            }
            WindowEvent::RedrawRequested => {
                self.redraw(event_loop);
                return;
            }
            _ => {}
        }
        let ppp = self.points_per_pixel();
        self.input.on_window_event(&event, ppp);
    }

    fn save_settings(&self) {
        if let Some(path) = &self.settings_path {
            if let Err(e) = settings::save_settings(path, &self.settings) {
                tracing::error!("{e}");
            }
        }
    }

    fn toggle_pause(&mut self) {
        match self.app.stack().top() {
            AppState::InGame => {
                self.game.release_all();
                let _ = self.app.pause();
            }
            AppState::Pause => self.app.handle(vec![faset_ui::UiCommand::Pop]),
            _ => {}
        }
    }

    /// Routes the raw keys, buttons and wheel of the frame: to the game while
    /// it is played, to the pause key always, to the rebinding view when it
    /// waits for a key egui does not name.
    fn route_raw_input(&mut self) {
        let playing = self.playing();
        for raw in self.input.take_raw() {
            let (input, pressed) = match raw {
                RawInput::Key { name, pressed } => {
                    if pressed && name == faset_core::bindings::RESERVED_KEY {
                        self.toggle_pause();
                        continue;
                    }
                    (Input::key(&name), pressed)
                }
                RawInput::Mouse { index, pressed } => (Input::Mouse(index), pressed),
                RawInput::Wheel(lines) => {
                    if playing {
                        self.game.wheel(lines);
                    }
                    continue;
                }
            };
            // Releases always go through, so no key stays stuck.
            if playing || !pressed {
                self.game.press(
                    input,
                    pressed,
                    &self.settings.bindings,
                    &self.settings.controls,
                );
            }
        }
    }

    /// Grabs and hides the mouse while the match is played.
    fn update_cursor(&mut self, playing: bool) {
        if playing == self.grabbed {
            return;
        }
        let Some(window) = &self.window else { return };
        if playing {
            let grabbed = window
                .set_cursor_grab(CursorGrabMode::Locked)
                .or_else(|_| window.set_cursor_grab(CursorGrabMode::Confined));
            if let Err(e) = grabbed {
                tracing::warn!("cannot grab the mouse: {e}");
            }
            window.set_cursor_visible(false);
        } else {
            let _ = window.set_cursor_grab(CursorGrabMode::None);
            window.set_cursor_visible(true);
        }
        self.grabbed = playing;
    }

    /// The camera of the frame and the field of view of the 3D view.
    fn camera(&mut self, aspect: f32, t: f32, dt: f32) -> (Camera, f32) {
        let fov = self.live.as_ref().map_or(80.0, |l| l.tuning.fov_deg);
        let calm = self.settings.accessibility.reduce_shake;
        let mut params = self.camera.params().clone();
        params.fov_base_deg = fov;
        params.roll_max_deg = if calm { 0.0 } else { 1.5 };
        params.landing_max_dip = if calm { 0.0 } else { 0.3 };
        if &params != self.camera.params() {
            self.camera.set_params(params);
        }
        let in_match = matches!(self.app.stack().states().first(), Some(AppState::InGame));
        let pose = self
            .app
            .session()
            .filter(|_| in_match)
            .and_then(|s| s.prediction())
            .and_then(|p| {
                let mut state = p.controller_state()?;
                state.pos = p.display_position()?;
                Some(state)
            })
            .map(|state| self.camera.update(dt, &state, self.game.look()));
        let shake = if calm { Quat::IDENTITY } else { self.shake };
        let (view, fov_y) = match pose {
            Some(pose) => (
                Mat4::from_rotation_translation(pose.orientation * shake, pose.eye).inverse(),
                pose.fov_y,
            ),
            None => {
                let (center, radius) = self.app.map_extent();
                let angle = t * std::f32::consts::TAU / MENU_TURN_SECONDS;
                let eye = center + Vec3::new(angle.cos(), 0.55, angle.sin()) * radius * 0.9;
                (
                    faset_render::glam::camera::rh::view::look_at_mat4(eye, center, Vec3::Y),
                    fov.to_radians(),
                )
            }
        };
        let proj = perspective_reverse_z(fov_y, aspect, 0.1);
        let mut camera = Camera::new(view, proj);
        if let Some(prev) = self.prev_view_proj {
            camera.prev_view_proj = prev;
        }
        self.prev_view_proj = Some(camera.view_proj());
        (camera, fov_y)
    }

    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        let (Some(mut renderer), Some(mut ui)) = (self.renderer.take(), self.ui.take()) else {
            return;
        };
        self.draw_frame(event_loop, &mut renderer, &mut ui);
        self.renderer = Some(renderer);
        self.ui = Some(ui);
    }

    fn draw_frame(&mut self, event_loop: &ActiveEventLoop, renderer: &mut Renderer, ui: &mut Ui) {
        let now_frame = Instant::now();
        let now = now_frame;
        let dt = now - self.last_frame;
        self.last_frame = now;
        let elapsed = self.start.elapsed();
        let dt_s = dt.as_secs_f32().min(MAX_FRAME_SECONDS);

        self.route_raw_input();
        let playing = self.playing();
        self.update_cursor(playing);
        // The look starts where the server put the player.
        if let Some(session) = self.app.session() {
            if playing && self.look_from != Some(session.instance_id()) {
                if let Some(state) = session.prediction().and_then(|p| p.state()) {
                    self.game.set_look(state.yaw, state.pitch);
                    self.look_from = Some(session.instance_id());
                }
            }
        }

        {
            let (bindings, controls) = (&self.settings.bindings, &self.settings.controls);
            let game = &mut self.game;
            self.app
                .update(dt, &mut |tick| game.command(tick, bindings, controls));
        }
        if self.app.quit_requested() || self.limit.is_some_and(|l| elapsed >= l) {
            event_loop.exit();
            return;
        }
        if let Some(map) = self.app.take_map() {
            self.look = MapLook::of(&map.map);
            self.look_since = Instant::now();
            self.surfaces = Arc::new(
                SurfaceMap::from_map(&map.map, map.collision.volume_count())
                    .with_water(self.look.water.as_ref().map(|w| w.height)),
            );
            if let Some(scene) = &mut self.scene {
                scene.set_surfaces(self.surfaces.clone());
            }
            match map.upload(renderer) {
                Ok(instances) => self.instances = instances,
                Err(e) => tracing::error!("cannot upload the map: {e}"),
            }
        }
        if let Some(status) = self.app.take_status() {
            tracing::warn!("{status}");
        }
        let in_match = matches!(self.app.stack().states().first(), Some(AppState::InGame))
            && self.app.session().is_some_and(|s| s.in_game());
        if in_match && self.scene.is_none() {
            match MatchScene::upload(renderer) {
                Ok(mut scene) => {
                    scene.set_surfaces(self.surfaces.clone());
                    self.scene = Some(scene);
                }
                Err(e) => tracing::error!("cannot upload the match scene: {e}"),
            }
        }

        let size_px = renderer.size();
        let state = self.app.stack().top();
        let mut events = self.input.take_events();
        if playing {
            // The mouse and the typing belong to the game.
            events.retain(|e| matches!(e, Event::Key { .. } | Event::WindowFocused(_)));
        }
        ui.begin_frame(FrameInput {
            size_px,
            scale_factor: self
                .window
                .as_ref()
                .map_or(1.0, |w| w.scale_factor() as f32),
            time: elapsed.as_secs_f64(),
            dt: dt_s,
            events,
        });
        let now = elapsed.as_secs_f64();
        let aspect = size_px.0.max(1) as f32 / size_px.1.max(1) as f32;
        let (camera, fov_y) = self.camera(aspect, elapsed.as_secs_f32(), dt_s);

        // The HUD of the match, under the screen of the state.
        let hud = if in_match {
            let frame = self.local_frame(dt_s);
            let options = HudOptions::from_settings(&self.settings.game);
            self.app
                .session_mut()
                .and_then(|s| s.view_mut().hud_model(&frame, dt_s))
                .map(|model| {
                    FpsHud::new(model, fov_y)
                        .with_options(options)
                        .with_damage(self.damage.clone())
                })
        } else {
            self.shake = Quat::IDENTITY;
            self.damage.clear();
            None
        };

        let mut commands = Vec::new();
        if state == AppState::Settings {
            let available = self
                .live
                .as_ref()
                .map(|l| l.available.clone())
                .unwrap_or_default();
            let mut screen = SettingsScreen::new(&mut self.settings, &available);
            screen.tab = self.settings_tab;
            screen.rebind = std::mem::take(&mut self.rebind);
            screen.confirm = self.confirm.map(|c| c.poll(now));
            ui.draw(&mut Composite {
                hud,
                top: &mut screen,
            });
            self.settings_tab = screen.tab;
            self.rebind = std::mem::take(&mut screen.rebind);
            for event in screen.events {
                match event {
                    SettingsEvent::BindingsChanged | SettingsEvent::PseudonymChanged => {}
                    SettingsEvent::Back => commands.push(faset_ui::UiCommand::Pop),
                    SettingsEvent::KeepResolution => self.confirm = None,
                    SettingsEvent::RevertResolution => {
                        if let Some(c) = self.confirm.take() {
                            self.settings.display.resolution = c.previous;
                        }
                    }
                    SettingsEvent::Changed(id) => {
                        if id == "resolution" && self.confirm.is_none() {
                            self.confirm = Some(ResolutionConfirm::start(
                                self.applied.display.resolution,
                                now,
                            ));
                        }
                    }
                }
            }
        } else {
            let mut screen = StateScreen::new(state, self.app.model_mut());
            ui.draw(&mut Composite {
                hud,
                top: &mut screen,
            });
            commands = screen.commands;
        }
        if self
            .confirm
            .is_some_and(|c| c.poll(now) == ConfirmState::Expired)
        {
            if let Some(c) = self.confirm.take() {
                self.settings.display.resolution = c.previous;
            }
        }
        if let (Some(live), Some(window)) = (&mut self.live, &self.window) {
            if self.settings != self.applied {
                let old = std::mem::replace(&mut self.applied, self.settings.clone());
                let changed = live.apply_changes(renderer, window, &old, &self.settings);
                if changed.contains(&"ui_scale") {
                    ui.set_scale(self.settings.accessibility.ui_scale);
                }
                if changed.contains(&"net_graph") {
                    ui.overlay().set_visible(self.settings.network.net_graph);
                }
            }
        }
        if commands.contains(&faset_ui::UiCommand::Pop) && state == AppState::Settings {
            self.save_settings();
        }
        if commands.contains(&faset_ui::UiCommand::LeaveMatch) {
            self.game.release_all();
            self.look_from = None;
        }
        self.app.handle(commands);

        let mut camera = camera;
        camera.jitter_px = renderer.upscaler_jitter(renderer.frames_rendered() as u32);
        let mut world = RenderWorld {
            camera,
            instances: self.instances.clone(),
            ..RenderWorld::default()
        };
        self.look
            .apply(&mut world, self.look_since.elapsed().as_secs_f32());
        renderer.set_material_time(elapsed.as_secs_f32());
        if in_match {
            self.add_scene(&mut world, dt_s);
        }
        if state == AppState::Loadout {
            if self.preview.is_none() {
                match faset_client::CharacterPreview::upload(renderer) {
                    Ok(p) => self.preview = Some(p),
                    Err(e) => tracing::error!("cannot upload the preview: {e}"),
                }
            }
            if let Some(preview) = &self.preview {
                let request = self.app.model().loadout.preview_request();
                let yaw = elapsed.as_secs_f32() * std::f32::consts::TAU
                    / faset_client::preview::TURN_SECONDS;
                let jitter = world.camera.jitter_px;
                world = preview.render_world(&request, yaw, aspect);
                world.camera.jitter_px = jitter;
            }
        }
        if let Some(live) = &self.live {
            let d = live.tuning.weather_density;
            let w = &mut world.weather;
            w.rain *= d;
            w.snow *= d;
            w.sand *= d;
            w.storm *= d;
            if live.tuning.brightness != 1.0 {
                let mut post = faset_render::PostParams::from_profile(&world.ambience, false);
                post.exposure *= live.tuning.brightness;
                world.post = Some(post);
            }
        }
        if let Err(e) = renderer.render_frame(&world) {
            tracing::error!("frame failed: {e}");
            event_loop.exit();
        }
        if let Some(budget) = self.live.as_ref().and_then(|l| l.tuning.frame_budget()) {
            if let Some(rest) = budget.checked_sub(now_frame.elapsed()) {
                std::thread::sleep(rest);
            }
        }
    }

    /// What the HUD needs of the local player this frame.
    fn local_frame(&self, _dt: f32) -> LocalFrame {
        let state = self
            .app
            .session()
            .and_then(|s| s.prediction())
            .and_then(|p| p.controller_state());
        LocalFrame {
            speed: state.map_or(0.0, |s| Vec3::new(s.vel.x, 0.0, s.vel.z).length()),
            airborne: state.is_some_and(|s| !s.on_ground),
            sighted: self
                .game
                .aiming(&self.settings.bindings, &self.settings.controls),
            scoreboard: self.game.scoreboard(&self.settings.bindings),
        }
    }

    /// Adds the other players, pickups, projectiles and the weapon in hand.
    fn add_scene(&mut self, world: &mut RenderWorld, dt: f32) {
        let (Some(scene), Some(session)) = (&mut self.scene, self.app.session_mut()) else {
            return;
        };
        let camera = world.camera;
        let cam_world = camera.view.inverse();
        let (_, orientation, eye) = cam_world.to_scale_rotation_translation();
        let state = session.prediction().and_then(|p| p.controller_state());
        let frame = SceneFrame {
            eye,
            orientation,
            dt,
            speed: state.map_or(0.0, |s| Vec3::new(s.vel.x, 0.0, s.vel.z).length()),
            grounded: state.is_none_or(|s| s.on_ground),
            weapon_position: self.settings.game.weapon_position,
            look: self.game.look(),
        };
        let out = scene.instances(session, &frame);
        world.instances.extend(out.world);
        world.viewmodel = out.viewmodel;
        world.lights.extend(out.lights);
        self.shake = out.shake;
        self.damage = out.damage;
        if let Some(engine) = self.live.as_mut().and_then(apply::Live::audio_mut) {
            engine.set_listener(eye, orientation * Vec3::NEG_Z, orientation * Vec3::Y);
            engine.set_underwater(world.underwater);
            for (cue, at) in out.cues {
                faset_client::sounds::play(engine, cue, at);
            }
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

    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device: DeviceId,
        event: DeviceEvent,
    ) {
        if let (DeviceEvent::MouseMotion { delta: (dx, dy) }, true) = (event, self.grabbed) {
            self.game
                .mouse_motion(dx, dy, &self.settings.bindings, &self.settings.controls);
        }
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

    let settings_path = value("--settings")
        .map(PathBuf::from)
        .or_else(settings::settings_path);
    let mut settings = match &settings_path {
        Some(path) => settings::load_settings_with(path, args.iter().any(|a| a == "--safe")),
        None => Settings::default(),
    };
    if args.iter().any(|a| a == "--no-vsync") {
        settings.display.vsync = false;
    }

    let event_loop = EventLoop::new().context("cannot create the event loop")?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App::new(registry(), MapCatalogue::scan(&maps_dir));
    if let Some(dir) = faset_client::data_dir() {
        app.use_favorites_file(dir.join(faset_client::favorites::FILE_NAME));
        app.use_profile_file(dir.join(faset_client::profile::FILE_NAME));
    }
    let mut client = Client {
        app,
        applied: settings.clone(),
        settings,
        settings_path,
        settings_tab: 0,
        rebind: faset_ui::RebindView::default(),
        confirm: None,
        live: None,
        limit: (seconds > 0).then(|| Duration::from_secs(seconds)),
        start: Instant::now(),
        last_frame: Instant::now(),
        input: EguiInput::new(),
        game: GameInput::new(),
        look_from: None,
        camera: FirstPersonCamera::new(faset_client::FeelParams::default()),
        grabbed: false,
        focused: true,
        instances: Vec::new(),
        look: MapLook::default(),
        look_since: Instant::now(),
        preview: None,
        scene: None,
        surfaces: Arc::new(SurfaceMap::default()),
        shake: Quat::IDENTITY,
        damage: Vec::new(),
        prev_view_proj: None,
        ui: None,
        renderer: None,
        window: None,
    };
    event_loop.run_app(&mut client)?;
    tracing::info!("exited cleanly");
    Ok(())
}

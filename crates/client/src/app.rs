//! The application layer: the state stack, the menus' data, the match
//! [`Session`] and the glue between them. It owns no window and no GPU
//! object, so the whole state machine runs in tests; the `faset` executable
//! drives it with a window, a renderer and egui.
//!
//! Flow of a solo game: the play screen emits `StartMatch`; the stack is
//! reset to `Loading`; on the next [`App::update`] the map is parsed and built
//! (mesh data queued for the renderer, see [`App::take_map`]), an integrated
//! server starts, and when the first snapshot has synchronised the clock the
//! state becomes `InGame`. Leaving the match drops the session and returns to
//! the main menu.

use std::collections::BTreeMap;
use std::net::{SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use faset_core::glam::Vec3;
use faset_core::Tick;
use faset_minigame_api::Registry;
use faset_protocol::InputCommand;
use faset_server::ServerConfig;
use faset_ui::{AppState, MenuModel, PlayRequest, UiCommand};

use crate::map_load::{load_map, LoadedMap};
use crate::session::{map_config, Session, SessionError};
use crate::state::{StateError, StateStack, Transition};

/// Map embedded in the executable, so the menus work without an assets
/// folder.
const EMBEDDED_MAP: (&str, &str) = ("example", include_str!("../../../assets/maps/example.ron"));

/// How long a remote connection may take before the player is sent back.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Maps the player can pick: the embedded one plus every `*.ron` file of a
/// folder (name = file stem; the embedded name wins).
#[derive(Debug, Clone, Default)]
pub struct MapCatalogue {
    maps: BTreeMap<String, Option<PathBuf>>,
}

impl MapCatalogue {
    /// The embedded map plus the files of `dir` (a missing folder is not an
    /// error).
    #[must_use]
    pub fn scan(dir: &Path) -> Self {
        let mut maps = BTreeMap::new();
        if let Ok(entries) = std::fs::read_dir(dir) {
            for path in entries.filter_map(Result::ok).map(|e| e.path()) {
                let is_map = path.extension().is_some_and(|e| e == "ron");
                if let (true, Some(stem)) = (is_map, path.file_stem().and_then(|s| s.to_str())) {
                    maps.insert(stem.to_owned(), Some(path.clone()));
                }
            }
        }
        maps.insert(EMBEDDED_MAP.0.to_owned(), None);
        Self { maps }
    }

    /// Map names, sorted.
    #[must_use]
    pub fn names(&self) -> Vec<String> {
        self.maps.keys().cloned().collect()
    }

    /// The file bytes of map `name`.
    ///
    /// # Errors
    /// Unknown name or unreadable file.
    pub fn bytes(&self, name: &str) -> Result<Vec<u8>, String> {
        match self.maps.get(name) {
            Some(None) => Ok(EMBEDDED_MAP.1.as_bytes().to_vec()),
            Some(Some(path)) => std::fs::read(path).map_err(|e| format!("{}: {e}", path.display())),
            None => Err(format!("unknown map `{name}`")),
        }
    }
}

/// A match waiting for its map to load.
struct PendingMatch {
    request: PlayRequest,
    /// Frames shown so far: the loading screen is drawn once before the
    /// blocking load starts.
    frames: u32,
}

/// The application.
pub struct App {
    stack: StateStack,
    model: MenuModel,
    registry: Registry,
    catalogue: MapCatalogue,
    session: Option<Session>,
    pending: Option<PendingMatch>,
    connecting_since: Option<Instant>,
    map: Option<LoadedMap>,
    map_bounds: (Vec3, Vec3),
    status: Option<String>,
    quit: bool,
}

/// The two points a menu camera looks between, from the bounds of a map.
fn bounds_of(map: &LoadedMap) -> (Vec3, Vec3) {
    let mut min = Vec3::splat(f32::MAX);
    let mut max = Vec3::splat(f32::MIN);
    for v in map.chunks.iter().flat_map(|c| &c.vertices) {
        let p = Vec3::from_array(v.position);
        min = min.min(p);
        max = max.max(p);
    }
    if min.x > max.x {
        (Vec3::splat(-8.0), Vec3::splat(8.0))
    } else {
        (min, max)
    }
}

impl App {
    /// A new application in `Boot`, listing the modes of `registry` and the
    /// maps of `catalogue`.
    #[must_use]
    pub fn new(registry: Registry, catalogue: MapCatalogue) -> Self {
        let model = MenuModel::new(&registry, catalogue.names());
        Self {
            stack: StateStack::default(),
            model,
            registry,
            catalogue,
            session: None,
            pending: None,
            connecting_since: None,
            map: None,
            map_bounds: (Vec3::splat(-8.0), Vec3::splat(8.0)),
            status: None,
            quit: false,
        }
    }

    /// The state stack.
    #[must_use]
    pub fn stack(&self) -> &StateStack {
        &self.stack
    }

    /// Mutable access to the state stack (tests, and hosts of the app).
    pub fn stack_mut(&mut self) -> &mut StateStack {
        &mut self.stack
    }

    /// The menus' data.
    #[must_use]
    pub fn model(&self) -> &MenuModel {
        &self.model
    }

    /// Mutable access to the menus' data.
    pub fn model_mut(&mut self) -> &mut MenuModel {
        &mut self.model
    }

    /// The mode registry.
    #[must_use]
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// The running match, if any.
    #[must_use]
    pub fn session(&self) -> Option<&Session> {
        self.session.as_ref()
    }

    /// Mutable access to the running match.
    pub fn session_mut(&mut self) -> Option<&mut Session> {
        self.session.as_mut()
    }

    /// Installs a session as the running match (used by tests and by hosts
    /// that build the session themselves). The stack is not touched.
    pub fn set_session(&mut self, session: Option<Session>) {
        self.session = session;
    }

    /// Whether the player asked to close the application.
    #[must_use]
    pub fn quit_requested(&self) -> bool {
        self.quit
    }

    /// The last error to show the player (a match that failed to start, a
    /// connection that timed out), cleared when read.
    pub fn take_status(&mut self) -> Option<String> {
        self.status.take()
    }

    /// The map the renderer must upload, once per map: the menu background
    /// or the match's map.
    pub fn take_map(&mut self) -> Option<LoadedMap> {
        self.map.take()
    }

    /// Centre and radius of the current map, for the menu camera.
    #[must_use]
    pub fn map_extent(&self) -> (Vec3, f32) {
        let (min, max) = self.map_bounds;
        ((min + max) * 0.5, ((max - min).length() * 0.5).max(1.0))
    }

    /// Whether the keyboard and mouse drive the player (the match is on top).
    #[must_use]
    pub fn accepts_game_input(&self) -> bool {
        self.stack.top() == AppState::InGame && self.session.is_some()
    }

    /// Applies commands issued by a screen.
    pub fn handle(&mut self, commands: Vec<UiCommand>) {
        for command in commands {
            match self.stack.apply(command) {
                Ok(None) => {}
                Ok(Some(other)) => self.handle_app_command(other),
                Err(e) => tracing::warn!(error = %e, "state change refused"),
            }
        }
        self.after_transitions();
    }

    fn handle_app_command(&mut self, command: UiCommand) {
        match command {
            UiCommand::Quit => self.quit = true,
            UiCommand::StartMatch(request) => {
                self.close_session();
                self.stack.reset(AppState::Loading);
                self.model.loading = 0.0;
                self.model.loading_map = self
                    .model
                    .maps
                    .get(request.map)
                    .cloned()
                    .unwrap_or_default();
                self.pending = Some(PendingMatch { request, frames: 0 });
            }
            UiCommand::Connect(address) => match resolve(&address) {
                Some(addr) => match Session::connect(addr) {
                    Ok(session) => {
                        self.close_session();
                        self.session = Some(session);
                        self.connecting_since = Some(Instant::now());
                        if let Err(e) = self.stack.push(AppState::Connecting) {
                            tracing::warn!(error = %e, "cannot show the connection screen");
                        }
                        if !self.model.history.contains(&address) {
                            self.model.history.insert(0, address);
                            self.model.history.truncate(8);
                        }
                    }
                    Err(e) => self.status = Some(e.to_string()),
                },
                None => self.status = Some(format!("invalid address `{address}`")),
            },
            UiCommand::LeaveMatch => {
                self.close_session();
                self.stack.reset(AppState::MainMenu);
                self.queue_background();
            }
            UiCommand::Push(_) | UiCommand::Pop | UiCommand::Replace(_) => {}
        }
    }

    /// Reacts to the transitions of the stack: cancelling a connection
    /// drops its session.
    fn after_transitions(&mut self) {
        for t in self.stack.drain_events() {
            if let Transition::Popped(AppState::Connecting) = t {
                self.connecting_since = None;
                if self.session.as_ref().is_some_and(|s| !s.in_game()) {
                    self.close_session();
                }
            }
        }
    }

    fn close_session(&mut self) {
        if let Some(session) = self.session.take() {
            session.leave();
        }
        self.connecting_since = None;
    }

    /// Queues the first catalogue map as the menu background.
    fn queue_background(&mut self) {
        let Some(name) = self.model.maps.first().cloned() else {
            return;
        };
        match self
            .catalogue
            .bytes(&name)
            .and_then(|b| load_map(&b).map_err(|e| e.to_string()))
        {
            Ok(map) => {
                self.map_bounds = bounds_of(&map);
                self.map = Some(map);
            }
            Err(e) => tracing::warn!(error = %e, "menu background not loaded"),
        }
    }

    fn start_pending(&mut self, request: &PlayRequest) -> Result<(), String> {
        let name = self
            .model
            .maps
            .get(request.map)
            .cloned()
            .ok_or("no map selected")?;
        let bytes = self.catalogue.bytes(&name)?;
        let loaded = load_map(&bytes).map_err(|e| e.to_string())?;
        let (map, _) = map_config(&bytes).map_err(|e: SessionError| e.to_string())?;
        let mode = self
            .registry
            .find(&request.mode)
            .map_or(0, |entry| entry.id.0);
        let mut server = ServerConfig {
            mode,
            map,
            seed: seed_now(),
            ..ServerConfig::default()
        };
        server.limits.max_players = usize::from(request.max_players.max(1));
        let session = Session::solo(server, &name, request.lan).map_err(|e| e.to_string())?;
        self.map_bounds = bounds_of(&loaded);
        self.map = Some(loaded);
        self.session = Some(session);
        Ok(())
    }

    /// Advances the application by `dt`: finishes `Boot`, runs the blocking
    /// part of a map load once its screen has been shown, steps the session
    /// (asking `input` for the local player's commands) and moves from
    /// `Loading` or `Connecting` to `InGame` when the match is ready.
    pub fn update(&mut self, dt: Duration, input: &mut dyn FnMut(Tick) -> InputCommand) {
        if self.stack.top() == AppState::Boot {
            self.queue_background();
            if let Err(e) = self.stack.replace(AppState::MainMenu) {
                tracing::error!(error = %e, "boot cannot finish");
            }
        }
        if let Some(pending) = &mut self.pending {
            pending.frames += 1;
            if pending.frames > 1 {
                let pending = self.pending.take().expect("checked above");
                let started = Instant::now();
                match self.start_pending(&pending.request) {
                    Ok(()) => {
                        self.model.loading = 0.5;
                        tracing::info!(ms = started.elapsed().as_millis(), "map loaded");
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "match not started");
                        self.status = Some(e);
                        self.stack.reset(AppState::MainMenu);
                    }
                }
            }
        }
        if let Some(session) = self.session.as_mut() {
            if let Err(e) = session.frame(dt, input) {
                tracing::error!(error = %e, "connection lost");
                self.status = Some(e.to_string());
                self.close_session();
                self.stack.reset(AppState::MainMenu);
            }
        }
        let ready = self.session.as_ref().is_some_and(Session::in_game);
        match self.stack.top() {
            AppState::Loading if ready => {
                self.model.loading = 1.0;
                let _ = self.stack.replace(AppState::InGame);
            }
            AppState::Connecting if ready => self.stack.reset(AppState::InGame),
            AppState::Connecting
                if self
                    .connecting_since
                    .is_some_and(|t| t.elapsed() > CONNECT_TIMEOUT) =>
            {
                self.status = Some("the server did not answer".to_owned());
                self.close_session();
                let _ = self.stack.pop();
            }
            _ => {}
        }
        self.after_transitions();
    }

    /// Pushes the pause menu over the match (no-op elsewhere).
    ///
    /// # Errors
    /// The stack refused: the match is not on top.
    pub fn pause(&mut self) -> Result<(), StateError> {
        self.stack.push(AppState::Pause)
    }
}

fn resolve(address: &str) -> Option<SocketAddr> {
    address.to_socket_addrs().ok()?.next().or_else(|| {
        // A bare host uses the default game port.
        (address, crate::discovery::GAME_PORT)
            .to_socket_addrs()
            .ok()?
            .next()
    })
}

fn seed_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64)
}

//! The integrated server: the client hosts a game in-process.
//!
//! [`host_game`] runs a [`Server`] on a dedicated thread. The host is an
//! ordinary client: it joins over a [`MemoryTransport`] (latency zero, no
//! special case in game code), see [`HostHandle::connect_local`].
//!
//! A solo game (with bots) is a hosted game that is never opened to the
//! network: it binds no UDP socket at all. [`HostHandle::open_to_lan`] adds a
//! UDP listener on the game port and starts answering discovery requests
//! while the server keeps running. The server accepts its transports at
//! construction only, so it is given an inert link up front (`LanSlot`) and
//! the UDP socket is installed into it later.

use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;

use faset_minigame_api::Match;
use faset_net::{
    Datagram, MemoryAddr, MemoryNetwork, MemoryTransport, Transport, TransportError, UdpTransport,
};
use faset_server::{
    Bot, ConfigError, MapProvider, RotationConfig, Server, ServerConfig, TickClock,
};

use crate::discovery::{DiscoveryResponder, ServerInfo, DISCOVERY_PORT, GAME_PORT};

/// Errors from hosting a game.
#[derive(Debug, thiserror::Error)]
pub enum HostError {
    /// The server configuration is invalid.
    #[error("invalid server configuration: {0}")]
    Config(#[from] ConfigError),
    /// A socket could not be bound.
    #[error("cannot open the game to the LAN: {0}")]
    Bind(#[from] io::Error),
    /// The server thread could not be started or has stopped.
    #[error("the server thread is not running")]
    NotRunning,
}

/// Settings of a hosted game.
pub struct HostConfig {
    /// The server configuration.
    pub server: ServerConfig,
    /// Name announced to the LAN.
    pub name: String,
    /// Map name announced to the LAN.
    pub map_name: String,
    /// UDP game port used when opened to the LAN (0 picks a free port).
    pub game_port: u16,
    /// UDP port answered for discovery when opened to the LAN.
    pub discovery_port: u16,
    /// Bots added when the server starts.
    pub bots: Vec<Box<dyn Bot>>,
    /// Builds the mini-game match the server hosts (lot 6.2), if any. Runs
    /// on the server thread: a [`Match`] cannot cross threads.
    pub game: Option<GameFactory>,
    /// The map rotation (lot 6.2), if any.
    pub rotation: Option<HostRotation>,
    /// Names of the rotation's maps, in the provider's order; the LAN
    /// announcement follows the map in play.
    pub map_names: Vec<String>,
    /// Called on the server thread after every tick (bots get new brains
    /// there when the map changes).
    pub tick_hook: Option<TickHook>,
}

/// Builds the hosted match on the server thread.
pub type GameFactory = Box<dyn FnOnce() -> Option<Match> + Send>;

/// Called on the server thread after every tick.
pub type TickHook = Box<dyn FnMut(&mut Server) + Send>;

/// A map rotation for the integrated server.
pub struct HostRotation {
    /// Timing and order.
    pub config: RotationConfig,
    /// The maps.
    pub provider: Arc<dyn MapProvider>,
    /// Index of the map the server starts on.
    pub current: usize,
}

impl HostConfig {
    /// Default ports ([`GAME_PORT`], [`DISCOVERY_PORT`]), no bots, and the
    /// placeholder names "Faset game" and "unnamed".
    #[must_use]
    pub fn new(server: ServerConfig) -> Self {
        Self {
            server,
            name: "Faset game".into(),
            map_name: "unnamed".into(),
            game_port: GAME_PORT,
            discovery_port: DISCOVERY_PORT,
            bots: Vec::new(),
            game: None,
            rotation: None,
            map_names: Vec::new(),
            tick_hook: None,
        }
    }
}

type Slot = Arc<Mutex<Option<UdpTransport>>>;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A link that does nothing until a UDP socket is installed into it.
#[derive(Debug)]
struct LanSlot(Slot);

impl Transport for LanSlot {
    type Addr = SocketAddr;

    fn send_to(&mut self, to: SocketAddr, packet: &[u8]) -> Result<(), TransportError> {
        match lock(&self.0).as_mut() {
            Some(udp) => udp.send_to(to, packet),
            None => Ok(()),
        }
    }

    fn poll(&mut self) -> Result<Option<Datagram<SocketAddr>>, TransportError> {
        match lock(&self.0).as_mut() {
            Some(udp) => udp.poll(),
            None => Ok(None),
        }
    }
}

/// What opening to the LAN produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LanStatus {
    /// The UDP address the server now accepts clients on.
    pub game_addr: SocketAddr,
    /// Whether discovery requests are answered. `false` when the discovery
    /// port was already taken (another game on this machine); players can
    /// still join by address.
    pub discoverable: bool,
}

/// Handle of a running hosted game. Stops the server when dropped.
pub struct HostHandle {
    net: MemoryNetwork,
    server_addr: MemoryAddr,
    slot: Slot,
    game_port: u16,
    discovery_port: u16,
    to_thread: Sender<(DiscoveryResponder, u16)>,
    stop: Arc<AtomicBool>,
    players: Arc<AtomicUsize>,
    lan: Option<LanStatus>,
    thread: Option<JoinHandle<()>>,
}

/// Starts a server on a dedicated thread, listening on a memory transport,
/// and optionally opens it to the LAN right away.
///
/// # Errors
/// An invalid configuration, or the LAN game port cannot be bound (the
/// server is stopped again in that case).
pub fn host_game(config: HostConfig, open_to_lan: bool) -> Result<HostHandle, HostError> {
    config.server.validate()?;
    let net = MemoryNetwork::new();
    let server_t = net.endpoint();
    let server_addr = server_t.addr();
    let slot: Slot = Arc::default();
    let stop = Arc::new(AtomicBool::new(false));
    let players = Arc::new(AtomicUsize::new(0));
    let (to_thread, from_handle) = mpsc::channel();

    let (game_port, discovery_port) = (config.game_port, config.discovery_port);
    let thread = {
        let (slot, stop, players) = (slot.clone(), stop.clone(), players.clone());
        std::thread::Builder::new()
            .name("faset-integrated-server".into())
            .spawn(move || {
                run_server(config, server_t, slot, &stop, &players, &from_handle);
            })
            .map_err(|_| HostError::NotRunning)?
    };
    let mut handle = HostHandle {
        net,
        server_addr,
        slot,
        game_port,
        discovery_port,
        to_thread,
        stop,
        players,
        lan: None,
        thread: Some(thread),
    };
    if open_to_lan {
        handle.open_to_lan()?;
    }
    Ok(handle)
}

fn run_server(
    config: HostConfig,
    memory: MemoryTransport,
    slot: Slot,
    stop: &AtomicBool,
    players: &AtomicUsize,
    from_handle: &Receiver<(DiscoveryResponder, u16)>,
) {
    let HostConfig {
        server: server_config,
        name,
        mut map_name,
        bots,
        game,
        rotation,
        map_names,
        mut tick_hook,
        ..
    } = config;
    let transports = vec![memory.into(), LanSlot(slot).into()];
    let mut server = match Server::new(server_config, transports) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = %e, "integrated server not started");
            return;
        }
    };
    if let Some(game) = game.and_then(|build| build()) {
        server.host_match(game);
    }
    if let Some(r) = rotation {
        server.set_rotation(r.config, r.provider, r.current);
    }
    for bot in bots {
        if server.add_bot(bot).is_none() {
            tracing::warn!("no room for a bot");
        }
    }
    let mut responder: Option<(DiscoveryResponder, u16)> = None;
    let mut clock = TickClock::new(server.config().tick_rate);
    loop {
        for _ in 0..clock.wait() {
            server.tick();
            if let Some(hook) = tick_hook.as_mut() {
                hook(&mut server);
            }
        }
        let count = server.players().count();
        players.store(count, Ordering::Relaxed);
        if let Some(name) = server
            .rotation_current()
            .and_then(|i| map_names.get(i))
            .filter(|n| **n != map_name)
        {
            map_name.clone_from(name);
        }
        if let Ok(r) = from_handle.try_recv() {
            responder = Some(r);
        }
        if let Some((r, game_port)) = &responder {
            r.poll(&ServerInfo {
                name: name.clone(),
                mode: server.config().mode,
                map: map_name.clone(),
                players: u16::try_from(count).unwrap_or(u16::MAX),
                max_players: u16::try_from(server.config().limits.max_players).unwrap_or(u16::MAX),
                game_port: *game_port,
            });
        }
        if stop.load(Ordering::Relaxed) {
            return;
        }
    }
}

impl HostHandle {
    /// A new in-memory client endpoint and the server address to connect it
    /// to (`ClientEndpoint::new(transport, addr)`). The host joins this way.
    #[must_use]
    pub fn connect_local(&self) -> (MemoryTransport, MemoryAddr) {
        (self.net.endpoint(), self.server_addr)
    }

    /// Opens the running game to the LAN: a UDP listener on the configured
    /// game port plus the discovery responder. The server is not restarted.
    /// Does nothing if already open.
    ///
    /// # Errors
    /// The game port cannot be bound, or the server thread has stopped.
    pub fn open_to_lan(&mut self) -> Result<LanStatus, HostError> {
        if let Some(lan) = self.lan {
            return Ok(lan);
        }
        if self.thread.as_ref().is_none_or(JoinHandle::is_finished) {
            return Err(HostError::NotRunning);
        }
        let udp = UdpTransport::bind(("0.0.0.0", self.game_port))?;
        let game_addr = udp.local_addr()?;
        let discoverable = match DiscoveryResponder::bind(self.discovery_port) {
            Ok(r) => {
                self.to_thread
                    .send((r, game_addr.port()))
                    .map_err(|_| HostError::NotRunning)?;
                true
            }
            Err(e) => {
                tracing::warn!(error = %e, "discovery port unavailable, game not announced");
                false
            }
        };
        *lock(&self.slot) = Some(udp);
        let status = LanStatus {
            game_addr,
            discoverable,
        };
        self.lan = Some(status);
        Ok(status)
    }

    /// The LAN status, `None` for a solo game.
    #[must_use]
    pub fn lan(&self) -> Option<LanStatus> {
        self.lan
    }

    /// Number of UDP sockets this game holds (0 for a solo game).
    #[must_use]
    pub fn udp_sockets(&self) -> usize {
        self.lan.map_or(0, |l| 1 + usize::from(l.discoverable))
    }

    /// Players currently in the game, bots included (updated every tick).
    #[must_use]
    pub fn player_count(&self) -> usize {
        self.players.load(Ordering::Relaxed)
    }

    /// Stops the server, closes its sockets and waits for the thread.
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            if thread.join().is_err() {
                tracing::error!("integrated server thread panicked");
            }
        }
        *lock(&self.slot) = None;
        self.lan = None;
    }
}

impl Drop for HostHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl std::fmt::Debug for HostHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostHandle")
            .field("lan", &self.lan)
            .finish_non_exhaustive()
    }
}

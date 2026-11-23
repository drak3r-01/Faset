//! A client session: one connection to one server, from the handshake to the
//! predicted, interpolated view of the match.
//!
//! This is the reusable core of the game client (lot 6.1): the application
//! owns at most one [`Session`], created when a match starts and dropped when
//! the player leaves it. States stacked on top of `InGame` (settings, pause)
//! never touch it, so popping them gives back the same session, with the same
//! [`Session::instance_id`] and the same world.
//!
//! A session either hosts its own integrated server ([`Session::solo`]; the
//! server runs on its own thread and the session joins it through a memory
//! transport, no socket is bound unless the game is opened to the LAN) or
//! connects to a remote one ([`Session::connect`]).
//!
//! Each frame, [`Session::frame`] receives packets, reconciles the prediction
//! against snapshots, simulates the ticks the clock says are due, sends the
//! unacknowledged commands and feeds the remote interpolation.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use faset_core::glam::Vec3;
use faset_core::{NetId, Tick};
use faset_mapfmt::MapFile;
use faset_minigame_api::{GameEvent, ModeId, ModeMessage, Registry};
use faset_mode_fps::ServerMsg;
use faset_net::{Channel, ClientEndpoint, SendError, Transport, TransportError, UdpTransport};
use faset_protocol::{Event, InputCommand, Message, PROTOCOL_VERSION};
use faset_server::{
    build_fingerprint, map_fingerprint, InputPacket, MapConfig, ServerConfig, SnapshotDecoder,
};
use faset_sim::{MapCollision, MoveScale, StaticWorld, Zones};

use crate::clock::ClockSync;
use crate::hosting::{host_game, HostConfig, HostError, HostHandle};
use crate::interpolation::{RemoteInterpolation, RemoteState};
use crate::map_sync::{MapCache, MapEvent, MapSync};
use crate::match_view::MatchView;
use crate::prediction::{AuthoritativeState, PredictedState, Prediction};

/// Height of a spawn's capsule centre above the spawn's feet position: half
/// the standing capsule plus 1 cm, so the first tick lands (same value as the
/// dedicated server).
const SPAWN_LIFT: f32 = 0.91;

/// Seconds without news of an awaited map before the session plays on the
/// default arena.
const MAP_WAIT_SECONDS: f64 = 4.0;

/// A session that cannot start.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// The map file is invalid.
    #[error("invalid map: {0}")]
    Map(#[from] faset_mapfmt::MapError),
    /// The map has no spawn point.
    #[error("the map has no spawn point")]
    NoSpawn,
    /// The integrated server could not start.
    #[error("cannot host: {0}")]
    Host(#[from] HostError),
    /// No socket for a remote connection.
    #[error("cannot open a socket: {0}")]
    Socket(#[from] std::io::Error),
}

/// A map prepared for an integrated server: the server's view of a map file.
///
/// # Errors
/// An invalid map file, or one without a spawn point.
pub fn map_config(bytes: &[u8]) -> Result<(MapConfig, MapFile), SessionError> {
    let map = faset_mapfmt::expand(&faset_mapfmt::parse(bytes)?);
    let spawn_points: Vec<Vec3> = map
        .spawns
        .iter()
        .map(|s| Vec3::from_array(s.at) + Vec3::Y * SPAWN_LIFT)
        .collect();
    if spawn_points.is_empty() {
        return Err(SessionError::NoSpawn);
    }
    let config = MapConfig {
        statics: MapCollision::build(&map).world().clone(),
        fingerprint: map_fingerprint(bytes),
        spawn_points,
        zones: Zones::from_map(&map),
    };
    Ok((config, map))
}

/// The connection, without its transport's address type.
trait Conn {
    fn update(&mut self, dt: Duration) -> Result<(), TransportError>;
    fn send(&mut self, channel: Channel, payload: &[u8]) -> Result<(), SendError>;
    fn recv(&mut self, channel: Channel) -> Option<Vec<u8>>;
    fn rtt(&self) -> f64;
    fn disconnect(&mut self);
}

impl<T: Transport> Conn for ClientEndpoint<T> {
    fn update(&mut self, dt: Duration) -> Result<(), TransportError> {
        ClientEndpoint::update(self, dt)
    }
    fn send(&mut self, channel: Channel, payload: &[u8]) -> Result<(), SendError> {
        ClientEndpoint::send(self, channel, payload)
    }
    fn recv(&mut self, channel: Channel) -> Option<Vec<u8>> {
        ClientEndpoint::recv(self, channel)
    }
    fn rtt(&self) -> f64 {
        ClientEndpoint::rtt(self)
    }
    fn disconnect(&mut self) {
        ClientEndpoint::disconnect(self);
    }
}

static NEXT_INSTANCE: AtomicU64 = AtomicU64::new(1);

/// One match as the client sees it.
pub struct Session {
    instance: u64,
    conn: Box<dyn Conn>,
    // Field order: the connection is dropped before the server it talks to.
    host: Option<HostHandle>,
    hello_sent: bool,
    seed: u64,
    statics: StaticWorld,
    zones: Zones,
    decoder: SnapshotDecoder,
    player: Option<NetId>,
    /// Mode byte of the server's hello (lot 6.3).
    mode: Option<u8>,
    /// Reason of the last loadout replacement the server reported.
    loadout_replaced: Option<u8>,
    clock: ClockSync,
    prediction: Option<Prediction>,
    remotes: RemoteInterpolation,
    entities: usize,
    now: f64,
    /// The match as the client sees it (lot 6.6).
    view: MatchView,
    /// Map announcements and downloads (lot 6.6).
    maps: MapSync,
    /// Fingerprint of the map the session's collision world is built from.
    map_fp: Option<u64>,
    /// The next map file to show, set when a map became ready.
    new_map: Option<Vec<u8>>,
    /// Bytes of the map download received, `(chunks, total)`.
    map_progress: Option<(u16, u16)>,
    /// Why the session cannot go on (a refused map), once.
    failure: Option<String>,
    /// Server tick of the latest snapshot.
    server_tick: Tick,
    /// Session time of the last news of a map being awaited.
    map_wait: Option<f64>,
}

impl Session {
    /// Starts an integrated server for `server` and joins it. `map_name` is
    /// what the server announces on the LAN.
    ///
    /// # Errors
    /// The server configuration is invalid or its thread cannot start.
    pub fn solo(
        server: ServerConfig,
        map_name: &str,
        open_to_lan: bool,
    ) -> Result<Self, SessionError> {
        let mut config = HostConfig::new(server);
        config.map_name = map_name.to_owned();
        Self::hosted(config, open_to_lan)
    }

    /// Starts the integrated server described by `config` (match, rotation
    /// and bots included, see [`crate::host_config`]) and joins it.
    ///
    /// # Errors
    /// The server configuration is invalid or its thread cannot start.
    pub fn hosted(config: HostConfig, open_to_lan: bool) -> Result<Self, SessionError> {
        let (seed, statics, zones) = (
            config.server.seed,
            config.server.map.statics.clone(),
            config.server.map.zones.clone(),
        );
        let fingerprint = config.server.map.fingerprint;
        let host = host_game(config, open_to_lan)?;
        let (transport, addr) = host.connect_local();
        let conn = Box::new(ClientEndpoint::new(transport, addr));
        let mut session = Self::with_conn(conn, Some(host), seed, statics, zones);
        session.map_fp = Some(fingerprint);
        Ok(session)
    }

    /// Connects to the server at `address`.
    ///
    /// The prediction uses the default arena until the map transfer (lot
    /// 5.1's `MapSync`) is wired into the session: only the collision world
    /// is affected, the server stays authoritative.
    ///
    /// # Errors
    /// No UDP socket could be opened.
    pub fn connect(address: SocketAddr) -> Result<Self, SessionError> {
        let transport = UdpTransport::bind(("0.0.0.0", 0))?;
        let conn = Box::new(ClientEndpoint::new(transport, address));
        let map = MapConfig::default();
        Ok(Self::with_conn(conn, None, 0, map.statics, map.zones))
    }

    fn with_conn(
        conn: Box<dyn Conn>,
        host: Option<HostHandle>,
        seed: u64,
        statics: StaticWorld,
        zones: Zones,
    ) -> Self {
        Self {
            instance: NEXT_INSTANCE.fetch_add(1, Ordering::Relaxed),
            conn,
            host,
            hello_sent: false,
            seed,
            statics,
            zones,
            decoder: SnapshotDecoder::new(),
            player: None,
            mode: None,
            loadout_replaced: None,
            clock: ClockSync::default(),
            prediction: None,
            remotes: RemoteInterpolation::default(),
            entities: 0,
            now: 0.0,
            view: MatchView::new(Registry::new()),
            maps: MapSync::new(MapCache::new()),
            map_fp: None,
            new_map: None,
            map_progress: None,
            failure: None,
            server_tick: Tick(0),
            map_wait: None,
        }
    }

    /// Gives the session the modes it can mirror (lot 6.6): without them
    /// there is no match state, HUD or scoreboard.
    #[must_use]
    pub fn with_registry(mut self, registry: Registry) -> Self {
        self.view = MatchView::new(registry);
        self
    }

    /// Gives the session the maps it already has (lot 6.6): a map announced
    /// by the server is looked up here by fingerprint before it is
    /// downloaded.
    #[must_use]
    pub fn with_map_cache(mut self, cache: MapCache) -> Self {
        self.maps = MapSync::new(cache);
        self
    }

    /// The match as the client sees it: phase, scores, players, pickups,
    /// projectiles and the HUD model.
    pub fn view_mut(&mut self) -> &mut MatchView {
        &mut self.view
    }

    /// The match as the client sees it.
    #[must_use]
    pub fn view(&self) -> &MatchView {
        &self.view
    }

    /// The bytes of a map the renderer must show, once per map: the one a
    /// remote server announced at the handshake or a rotation.
    pub fn take_new_map(&mut self) -> Option<Vec<u8>> {
        self.new_map.take()
    }

    /// Chunks `(received, total)` of the map being downloaded.
    #[must_use]
    pub fn map_progress(&self) -> Option<(u16, u16)> {
        self.map_progress
    }

    /// Why the session cannot go on, once (a map that failed its check).
    pub fn take_failure(&mut self) -> Option<String> {
        self.failure.take()
    }

    /// Process-unique id of this match instance: two sessions never share it,
    /// so it tells whether the world a state sees is the same one.
    #[must_use]
    pub fn instance_id(&self) -> u64 {
        self.instance
    }

    /// Entities in the latest snapshot (the world's entity count as the
    /// client knows it), 0 before the first one.
    #[must_use]
    pub fn entity_count(&self) -> usize {
        self.entities
    }

    /// Whether the clock is synchronised and the local player predicts.
    #[must_use]
    pub fn in_game(&self) -> bool {
        self.prediction.is_some()
    }

    /// The local player's entity, once the server said hello.
    #[must_use]
    pub fn player(&self) -> Option<NetId> {
        self.player
    }

    /// The integrated server's handle (LAN opening), for a hosted match.
    #[must_use]
    pub fn host(&self) -> Option<&HostHandle> {
        self.host.as_ref()
    }

    /// Mutable access to the integrated server's handle.
    pub fn host_mut(&mut self) -> Option<&mut HostHandle> {
        self.host.as_mut()
    }

    /// The local player's prediction.
    #[must_use]
    pub fn prediction(&self) -> Option<&Prediction> {
        self.prediction.as_ref()
    }

    /// Displayed position of the local player (prediction plus visual
    /// offset).
    #[must_use]
    pub fn display_position(&self) -> Option<Vec3> {
        self.prediction
            .as_ref()
            .and_then(Prediction::display_position)
    }

    /// The other players, interpolated.
    #[must_use]
    pub fn remotes(&self) -> &RemoteInterpolation {
        &self.remotes
    }

    /// The clock synchronisation.
    #[must_use]
    pub fn clock(&self) -> &ClockSync {
        &self.clock
    }

    /// Local real time since the session started, in seconds.
    #[must_use]
    pub fn time(&self) -> f64 {
        self.now
    }

    /// Server tick of the latest snapshot.
    #[must_use]
    pub fn server_tick(&self) -> Tick {
        self.server_tick
    }

    /// Mode byte the server announced (`None` before its hello).
    #[must_use]
    pub fn mode(&self) -> Option<u8> {
        self.mode
    }

    /// Sends mode-defined bytes to the server's mode (lot 6.3). Returns
    /// whether they were queued: false before the server's hello.
    pub fn send_mode_message(&mut self, bytes: Vec<u8>) -> bool {
        let Some(mode) = self.mode else {
            return false;
        };
        let message = ModeMessage {
            mode: ModeId(mode),
            bytes,
        };
        self.send_event(&Message::Event(Event::Mode(message.encode())));
        true
    }

    /// The reason code of the last loadout the server replaced with the
    /// default class, once ([`LoadoutError::code`](faset_mode_fps::LoadoutError::code)).
    pub fn take_loadout_replaced(&mut self) -> Option<u8> {
        self.loadout_replaced.take()
    }

    fn on_event(&mut self, event: &Event) {
        if let Event::MapChange { map_fingerprint } = event {
            if self.map_fp == Some(*map_fingerprint) {
                self.send_event(&Message::Ready);
            } else {
                self.begin_map(*map_fingerprint);
            }
            return;
        }
        self.view.on_event(event, self.now);
        if let Some(GameEvent::Mode(bytes)) = GameEvent::from_protocol(event) {
            if let Some(ServerMsg::LoadoutReplaced { player, reason }) = ServerMsg::decode(&bytes) {
                if Some(player) == self.player {
                    self.loadout_replaced = Some(reason);
                }
            }
        }
    }

    /// The server announced map `fingerprint`: use the copy we have or ask
    /// for it.
    fn begin_map(&mut self, fingerprint: u64) {
        self.map_wait = Some(self.now);
        if let Some(request) = self.maps.on_map_change(fingerprint) {
            self.send_event(&request);
        }
    }

    /// A map is ready: build the collision world of the prediction from it,
    /// hand the file to the renderer and tell the server.
    fn install_map(&mut self, fingerprint: u64, bytes: Vec<u8>) {
        match map_config(&bytes) {
            Ok((config, _)) => {
                self.statics = config.statics;
                self.zones = config.zones;
                self.map_fp = Some(fingerprint);
                self.map_wait = None;
                self.map_progress = None;
                if self.prediction.take().is_some() {
                    // A new world: synchronise the clock and predict again.
                    self.clock = ClockSync::default();
                    self.remotes = RemoteInterpolation::default();
                }
                self.new_map = Some(bytes);
                self.send_event(&Message::Ready);
            }
            Err(e) => self.failure = Some(format!("the server's map is unusable: {e}")),
        }
    }

    fn poll_maps(&mut self) {
        while let Some(bytes) = self.conn.recv(Channel::MapTransfer) {
            if let Ok(msg) = Message::decode(&bytes) {
                self.maps.on_chunk(&msg);
            }
        }
        while let Some(event) = self.maps.poll_event() {
            match event {
                MapEvent::Progress {
                    received, total, ..
                } => {
                    self.map_progress = Some((received, total));
                    self.map_wait = Some(self.now);
                }
                MapEvent::Ready { fingerprint, bytes } => self.install_map(fingerprint, bytes),
                MapEvent::Failed { error, .. } => {
                    self.failure = Some(format!("the server's map was refused: {error}"));
                }
            }
        }
        // A server that does not publish its map (no rotation) never
        // answers: play on the default arena as before, the server stays
        // authoritative.
        if self
            .map_wait
            .is_some_and(|t| self.now - t > MAP_WAIT_SECONDS)
        {
            tracing::warn!("the server did not send its map; predicting on the default arena");
            self.map_wait = None;
            self.send_event(&Message::Ready);
        }
    }

    fn send_event(&mut self, msg: &Message) {
        if let Ok(bytes) = msg.encode() {
            // A full channel only delays the event; the handshake is
            // repeated by the server's timeouts, not by us.
            let _ = self.conn.send(Channel::Event, &bytes);
        }
    }

    fn on_snapshot(&mut self, bytes: &[u8]) {
        let Ok(snap) = self.decoder.decode_bytes(bytes) else {
            return;
        };
        self.entities = snap.entities.len();
        self.server_tick = snap.tick;
        self.view.on_server_tick(snap.tick);
        let Some(me) = self.player else { return };
        let remotes = snap.entities.iter().filter(|r| r.id != me).map(|r| {
            let state = RemoteState {
                position: r.position,
                velocity: r.velocity,
                yaw: r.yaw,
                pitch: r.pitch,
            };
            (r.id, state)
        });
        self.remotes.on_snapshot(snap.tick, self.now, remotes);
        if let Some(render) = self.remotes.render_time(self.now, self.clock.jitter()) {
            self.remotes.prune(render);
        }
        let Some(e) = snap.entity(me) else { return };
        self.clock.observe_rtt(self.conn.rtt());
        let server = AuthoritativeState {
            tick: snap.tick,
            last_command: snap.last_command,
            state: PredictedState {
                position: e.position,
                velocity: e.velocity,
                yaw: e.yaw,
                pitch: e.pitch,
                weapon: e.weapon,
                on_ground: e.on_ground,
            },
        };
        if self.clock.on_snapshot(snap.tick) {
            let mut p =
                Prediction::new(me, self.seed, self.statics.clone()).with_zones(self.zones.clone());
            p.start(self.clock.next_tick(), server.state);
            self.prediction = Some(p);
            return;
        }
        if let Some(p) = self.prediction.as_mut() {
            p.reconcile(&server);
        }
    }

    /// One client frame of `real` time: receive, reconcile, simulate the
    /// ticks due (asking `input` for each command), send the newest
    /// unacknowledged commands. Returns the ticks simulated.
    ///
    /// # Errors
    /// The transport failed.
    pub fn frame(
        &mut self,
        real: Duration,
        input: &mut dyn FnMut(Tick) -> InputCommand,
    ) -> Result<u32, TransportError> {
        self.now += real.as_secs_f64();
        if !self.hello_sent {
            self.hello_sent = true;
            self.send_event(&Message::ClientHello {
                protocol_version: PROTOCOL_VERSION,
                build_fingerprint: build_fingerprint(),
            });
        }
        self.conn.update(real)?;
        while let Some(bytes) = self.conn.recv(Channel::Event) {
            match Message::decode(&bytes) {
                Ok(Message::ServerHello {
                    mode,
                    player_id,
                    map_fingerprint,
                    ..
                }) => {
                    self.player = Some(player_id);
                    self.mode = Some(mode);
                    self.view.start(mode, player_id);
                    if self.map_fp == Some(map_fingerprint) {
                        self.send_event(&Message::Ready);
                    } else {
                        self.begin_map(map_fingerprint);
                    }
                }
                Ok(Message::Event(event)) => self.on_event(&event),
                _ => {}
            }
        }
        self.poll_maps();
        while let Some(bytes) = self.conn.recv(Channel::Snapshot) {
            self.on_snapshot(&bytes);
        }
        let due: Vec<Tick> = self.clock.advance(real).collect();
        if let Some(p) = self.prediction.as_mut() {
            p.set_frozen(!self.view.movement_allowed() || !self.view.local_alive());
            let (run, jump) = self.view.move_scale();
            p.set_move_scale(MoveScale { run, jump });
            for &tick in &due {
                let mut command = input(tick);
                command.tick = tick;
                // A tick the prediction refuses (history gap) is skipped; the
                // next reconciliation realigns it.
                let _ = p.predict(command);
            }
            if !due.is_empty() {
                for command in p.unacknowledged() {
                    let packet = InputPacket {
                        command,
                        ack: self.decoder.ack(),
                    };
                    // A full channel only loses this copy; the next packet
                    // repeats the command.
                    let _ = self.conn.send(Channel::Input, &packet.encode());
                }
            }
        }
        self.conn.update(Duration::ZERO)?;
        Ok(u32::try_from(due.len()).unwrap_or(u32::MAX))
    }

    /// Leaves the match: tells the server, then stops the integrated one.
    pub fn leave(mut self) {
        self.conn.disconnect();
        let _ = self.conn.update(Duration::ZERO);
        tracing::info!(instance = self.instance, "session closed");
    }
}

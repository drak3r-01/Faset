//! The server: connections, handshake, command buffers, tick, the hosted
//! match and snapshots.

use std::collections::{BTreeMap, VecDeque};
use std::time::Duration;

use faset_core::glam::Vec3;
use faset_core::{NetId, Tick, DT, TICK_RATE};
use faset_minigame_api::{HostEffect, HostFrame, Match, MatchState, ModeMessage, Vitals};
use faset_net::{Channel, PeerId, ServerEvent};
use faset_protocol::quant::{dequantize_pitch_command, dequantize_yaw};
use faset_protocol::{Event, InputCommand, Message, PROTOCOL_VERSION};
use faset_sim::components::STANDING_HEIGHT;
use faset_sim::world::Hooks;
use faset_sim::World;

use crate::commands::CommandBuffer;
use crate::config::{ConfigError, ServerConfig};
use crate::lag_compensation::{FireCommand, Hitbox, LagCompensation, ShotOutcome};
use crate::link::ServerTransport;
use crate::services::ModeServices;
use crate::snapshot::{EntityState, InputPacket, Snapshot, SnapshotEncoder};

mod rotation_host;
use rotation_host::RotationState;

/// Health reported for a player when no hosted mode tracks health.
const DEFAULT_HEALTH: u8 = 100;

/// Why a connecting client was turned away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectReason {
    /// The client speaks another protocol version.
    ProtocolVersion {
        /// Version the client announced.
        client: u16,
    },
    /// The client is another build.
    BuildFingerprint {
        /// Fingerprint the client announced.
        client: u64,
    },
    /// The server already has `max_players` players.
    ServerFull,
    /// A message that does not belong at this point of the handshake.
    Protocol,
    /// The handshake was not finished in time.
    Timeout,
}

/// Something that happened to a client, from [`Server::poll_notice`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerNotice {
    /// A client finished the handshake and entered the game.
    Joined(NetId),
    /// A client was refused during the handshake; its connection is closed.
    Rejected(RejectReason),
    /// An in-game client left or timed out.
    Left(NetId),
}

/// What a bot sees when it chooses its command (lot 4.11).
#[derive(Clone, Copy)]
pub struct BotContext<'a> {
    /// The simulation world.
    pub world: &'a World,
    /// The hosted match, if any: teams, scores, phase, vitals.
    pub game: Option<&'a Match>,
    /// The match state replicated this tick, as a client would get it.
    pub match_state: Option<&'a MatchState>,
}

/// An internal virtual client. Its commands go through the same command
/// buffer as a human's (section 7), its messages through the same mode
/// message path.
pub trait Bot: Send {
    /// The command of player `me` for tick `tick`, read at the start of that
    /// tick. The default stands still.
    fn command(&mut self, _world: &World, _me: NetId, tick: Tick) -> InputCommand {
        InputCommand {
            tick,
            buttons: 0,
            move_x: 0,
            move_y: 0,
            yaw: 0,
            pitch: 0,
            weapon_slot: 0,
        }
    }

    /// [`command`](Self::command) with the match around the bot (lot 4.11);
    /// the default ignores the match.
    fn command_in(&mut self, ctx: &BotContext, me: NetId, tick: Tick) -> InputCommand {
        self.command(ctx.world, me, tick)
    }

    /// Mode message bytes the bot sends this tick (class choice), as a
    /// client sends `Event::Mode` (lot 4.11). None by default.
    fn messages(&mut self) -> Vec<Vec<u8>> {
        Vec::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    AwaitHello,
    AwaitReady,
    InGame,
}

#[derive(Debug)]
struct Client {
    player: Option<NetId>,
    phase: Phase,
    since: Tick,
    /// Reference snapshots sent to this client and its acknowledgement.
    snapshots: SnapshotEncoder,
}

/// What the server knows about an active player beyond the sim's position.
#[derive(Debug, Clone)]
struct Player {
    commands: CommandBuffer,
    /// Owning connection, `None` for a bot.
    conn: Option<(usize, PeerId)>,
    last_position: Vec3,
    velocity: Vec3,
    yaw: f32,
    pitch: f32,
    weapon: u8,
}

/// The authoritative game server.
pub struct Server {
    config: ServerConfig,
    links: Vec<ServerTransport>,
    world: World,
    clients: BTreeMap<(usize, PeerId), Client>,
    /// Active (in-game) players in ascending id order.
    players: BTreeMap<NetId, Player>,
    bots: BTreeMap<NetId, Box<dyn Bot>>,
    notices: VecDeque<ServerNotice>,
    next_player: u16,
    snapshot_acc: u32,
    /// One second of hitbox history per player, and the round trips.
    services: ModeServices,
    /// The hosted match (lot 4.11).
    game: Option<Match>,
    /// Bytes of the last match state sent to every client.
    sent_state: Option<Vec<u8>>,
    /// The map rotation (lot 5.1), if configured.
    rotation: Option<RotationState>,
    /// Number of map switches so far.
    map_generation: u32,
}

/// The hitboxes of a player as the sim holds it.
fn hitbox_of(world: &World, id: NetId) -> Option<Hitbox> {
    if let Some(s) = world.controller_state(id) {
        return Some(Hitbox {
            position: s.feet() + Vec3::Y * (STANDING_HEIGHT * 0.5),
            sliding: s.crouched,
        });
    }
    Some(Hitbox::standing(world.position(id)?))
}

/// Sim system 9: records the hitboxes of every living player, labelled with
/// the world tick after the step (the label of the snapshot of that state).
struct HistoryRecorder<'a> {
    lag: &'a mut LagCompensation,
    players: &'a [NetId],
}

impl Hooks for HistoryRecorder<'_> {
    fn record_collision_history(&mut self, world: &World) {
        let hitboxes = self
            .players
            .iter()
            .filter_map(|&id| Some((id, hitbox_of(world, id)?)));
        self.lag.record(world.tick().next(), hitboxes);
    }
}

impl std::fmt::Debug for Server {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Server")
            .field("tick", &self.world.tick())
            .field("links", &self.links.len())
            .field("clients", &self.clients.len())
            .field("players", &self.players.len())
            .field("bots", &self.bots.len())
            .field("game", &self.game)
            .finish()
    }
}

impl Server {
    /// A server listening on every transport in `transports` at once.
    ///
    /// # Errors
    /// An invalid [`ServerConfig`].
    pub fn new(
        config: ServerConfig,
        transports: Vec<ServerTransport>,
    ) -> Result<Self, ConfigError> {
        config.validate()?;
        let mut world =
            World::with_controller(config.seed, config.map.statics.clone(), config.move_params);
        world.set_zones(config.map.zones.clone());
        Ok(Self {
            config,
            links: transports,
            world,
            clients: BTreeMap::new(),
            players: BTreeMap::new(),
            bots: BTreeMap::new(),
            notices: VecDeque::new(),
            next_player: 1,
            snapshot_acc: 0,
            services: ModeServices::default(),
            game: None,
            sent_state: None,
            rotation: None,
            map_generation: 0,
        })
    }

    /// Hosts `game` (lot 4.11): every present player joins it, and from now
    /// on each tick runs it after the world step, applies its respawns and
    /// effects, freezes movement outside `Warmup` and `Playing` (and for
    /// dead players) and replicates its state, events and messages to every
    /// client. Replaces a match already hosted.
    pub fn host_match(&mut self, mut game: Match) {
        for &id in self.players.keys() {
            game.player_joined(&self.world, id);
        }
        self.game = Some(game);
        self.sent_state = None;
    }

    /// The hosted match.
    #[must_use]
    pub fn game(&self) -> Option<&Match> {
        self.game.as_ref()
    }

    /// The configuration.
    #[must_use]
    pub fn config(&self) -> &ServerConfig {
        &self.config
    }

    /// The simulation world.
    #[must_use]
    pub fn world(&self) -> &World {
        &self.world
    }

    /// Ids of the in-game players (humans and bots), ascending.
    pub fn players(&self) -> impl Iterator<Item = NetId> + '_ {
        self.players.keys().copied()
    }

    /// Whether `player` is a bot.
    #[must_use]
    pub fn is_bot(&self, player: NetId) -> bool {
        self.bots.contains_key(&player)
    }

    /// Connections still in the handshake.
    #[must_use]
    pub fn pending_clients(&self) -> usize {
        self.clients
            .values()
            .filter(|c| c.phase != Phase::InGame)
            .count()
    }

    /// Next connection notice, oldest first.
    pub fn poll_notice(&mut self) -> Option<ServerNotice> {
        self.notices.pop_front()
    }

    /// Adds a bot player and returns its id, or `None` when full.
    pub fn add_bot(&mut self, bot: Box<dyn Bot>) -> Option<NetId> {
        if self.players.len() >= self.config.limits.max_players {
            return None;
        }
        let id = self.allocate_player()?;
        self.spawn(id, None);
        self.bots.insert(id, bot);
        Some(id)
    }

    /// Removes an in-game player: closes a human's connection or drops a bot.
    /// Returns whether the player existed.
    pub fn disconnect(&mut self, player: NetId) -> bool {
        let Some(p) = self.players.remove(&player) else {
            return false;
        };
        self.bots.remove(&player);
        if let Some(conn) = p.conn {
            self.clients.remove(&conn);
            self.links[conn.0].listener.disconnect(conn.1);
        }
        self.player_gone(player);
        true
    }

    /// The hitbox history of every player.
    #[must_use]
    pub fn lag_compensation(&self) -> &LagCompensation {
        self.services.lag()
    }

    /// Smoothed round trip of a human player's connection in seconds; 0 for
    /// a bot, `None` for an unknown player.
    #[must_use]
    pub fn player_rtt(&self, player: NetId) -> Option<f64> {
        let p = self.players.get(&player)?;
        Some(
            p.conn
                .map_or(0.0, |(link, peer)| self.links[link].listener.rtt(peer)),
        )
    }

    /// Casts a hitscan shot of `shooter` with lag compensation (see
    /// [`LagCompensation::rewind_and_cast`]), using the shooter's measured
    /// round trip. `None` for an unknown shooter or before the first tick.
    pub fn rewind_and_cast(&mut self, shooter: NetId, fire: &FireCommand) -> Option<ShotOutcome> {
        let rtt = self.player_rtt(shooter)? as f32;
        self.services
            .lag_mut()
            .rewind_and_cast(self.world.statics(), shooter, fire, rtt)
    }

    /// Runs the real-time loop: one [`tick`](Self::tick) per 1/60 s, paced
    /// by a [`TickClock`](crate::TickClock), until `stop` returns true
    /// (checked before every wait).
    pub fn run_until(&mut self, mut stop: impl FnMut(&Self) -> bool) {
        let mut clock = crate::TickClock::new(self.config.tick_rate);
        while !stop(self) {
            for _ in 0..clock.wait() {
                self.tick();
            }
        }
    }

    /// Whether `player` may not move this tick: the match phase freezes
    /// everyone, or the mode says the player is dead.
    fn frozen(&self, player: NetId) -> bool {
        self.rotation
            .as_ref()
            .is_some_and(|r| r.clock.between_maps())
            || self.game.as_ref().is_some_and(|g| {
                !g.movement_allowed() || g.vitals(player).is_some_and(|v| !v.alive)
            })
    }

    /// Advances exactly one tick:
    /// 1. receive: connection changes, handshake messages, commands and
    ///    mode messages;
    /// 2. read one command per player from its buffer (repeat if missing),
    ///    frozen for players who may not move;
    /// 3. step the world (the character controller);
    /// 4. run the hosted match and apply its respawns and effects;
    /// 5. send its events and messages, then snapshots and the match state
    ///    when due (deltas against each client's acknowledged snapshot, see
    ///    [`crate::snapshot::delta`]), then flush.
    pub fn tick(&mut self) {
        let dt = Duration::from_secs_f32(DT);
        for link in 0..self.links.len() {
            if let Err(e) = self.links[link].listener.update(dt) {
                tracing::warn!(link, error = %e, "transport read failed");
            }
            self.receive(link);
        }
        self.expire_handshakes();

        let tick = self.world.tick();
        self.run_bots(tick);
        let mut inputs = Vec::with_capacity(self.players.len());
        let ids: Vec<NetId> = self.players.keys().copied().collect();
        for id in ids {
            let frozen = self.frozen(id);
            let Some(p) = self.players.get_mut(&id) else {
                continue;
            };
            if let Some(cmd) = p.commands.take(tick) {
                p.yaw = dequantize_yaw(cmd.yaw);
                p.pitch = dequantize_pitch_command(cmd.pitch);
                p.weapon = cmd.weapon_slot;
                inputs.push((id, if frozen { cmd.frozen() } else { cmd }));
            }
        }
        let alive: Vec<NetId> = self
            .players
            .keys()
            .copied()
            .filter(|&id| {
                self.game
                    .as_ref()
                    .and_then(|g| g.vitals(id))
                    .is_none_or(|v| v.alive)
            })
            .collect();
        self.world.step_with(
            &inputs,
            &mut HistoryRecorder {
                lag: self.services.lag_mut(),
                players: &alive,
            },
        );
        self.update_velocities();
        let zone_events = self.world.drain_zone_events();
        if !self
            .rotation
            .as_ref()
            .is_some_and(|r| r.clock.waiting_ready())
        {
            self.run_match(&inputs, &zone_events);
        }
        self.run_rotation();

        self.snapshot_acc += u32::from(self.config.snapshot_rate);
        if self.snapshot_acc >= u32::from(TICK_RATE) {
            self.snapshot_acc -= u32::from(TICK_RATE);
            self.broadcast_match_state();
            let conns: Vec<(usize, PeerId)> = self
                .clients
                .iter()
                .filter(|(_, c)| c.phase == Phase::InGame)
                .map(|(k, _)| *k)
                .collect();
            for conn in conns {
                self.send_snapshot(conn);
            }
        }
        for link in &mut self.links {
            if let Err(e) = link.listener.update(Duration::ZERO) {
                tracing::warn!(error = %e, "transport flush failed");
            }
        }
    }

    fn run_bots(&mut self, tick: Tick) {
        if self.bots.is_empty() {
            return;
        }
        let state = self.game.as_ref().map(Match::state);
        let ctx = BotContext {
            world: &self.world,
            game: self.game.as_ref(),
            match_state: state.as_ref(),
        };
        let mut messages = Vec::new();
        for (&id, bot) in &mut self.bots {
            let cmd = bot.command_in(&ctx, id, tick);
            if let Some(p) = self.players.get_mut(&id) {
                p.commands.push(cmd, tick);
            }
            for bytes in bot.messages() {
                messages.push((id, bytes));
            }
        }
        if let Some(game) = self.game.as_mut() {
            let mode = game.mode_id();
            for (id, bytes) in messages {
                game.client_message(id, ModeMessage { mode, bytes });
            }
        }
    }

    fn update_velocities(&mut self) {
        for (&id, p) in &mut self.players {
            if let Some(pos) = self.world.position(id) {
                p.velocity = self
                    .world
                    .controller_state(id)
                    .map_or((pos - p.last_position) / DT, |s| s.vel);
                p.last_position = pos;
            }
        }
    }

    /// Step 4 and the event part of step 5.
    fn run_match(
        &mut self,
        inputs: &[(NetId, InputCommand)],
        zone_events: &[(NetId, faset_sim::ZoneEvent)],
    ) {
        let Some(game) = self.game.as_mut() else {
            return;
        };
        let rtt = self
            .players
            .iter()
            .filter_map(|(&id, p)| {
                let (link, peer) = p.conn?;
                Some((id, self.links[link].listener.rtt(peer) as f32))
            })
            .collect();
        self.services.set_rtt(rtt);
        game.tick_with(
            &self.world,
            &mut HostFrame {
                commands: inputs,
                zone_events,
                host: Some(&mut self.services),
            },
        );
        for (id, at) in game.take_respawns() {
            if self.world.teleport(id, at) {
                if let Some(p) = self.players.get_mut(&id) {
                    p.last_position = at;
                    p.velocity = Vec3::ZERO;
                }
            }
        }
        for effect in game.take_effects() {
            match effect {
                HostEffect::Push { player, delta_v } => {
                    self.world.add_velocity(player, delta_v);
                }
                HostEffect::MoveScale { player, run, jump } => {
                    self.world
                        .set_move_scale(player, faset_sim::MoveScale { run, jump });
                }
            }
        }
        let events = game.drain_events();
        let messages = game.drain_messages();
        for event in events {
            self.broadcast_event(&event.to_protocol());
        }
        for message in messages {
            self.broadcast_event(&Event::Mode(message.encode()));
        }
    }

    /// Sends the match state to every in-game client if it changed since
    /// the last time.
    fn broadcast_match_state(&mut self) {
        let Some(game) = self.game.as_ref() else {
            return;
        };
        let bytes = game.state().encode();
        if self.sent_state.as_ref() == Some(&bytes) {
            return;
        }
        self.broadcast_event(&Event::MatchState(bytes.clone()));
        self.sent_state = Some(bytes);
    }

    fn broadcast_event(&mut self, event: &Event) {
        let msg = Message::Event(event.clone());
        let conns: Vec<(usize, PeerId)> = self
            .clients
            .iter()
            .filter(|(_, c)| c.phase == Phase::InGame)
            .map(|(k, _)| *k)
            .collect();
        for conn in conns {
            self.send(conn, Channel::Event, &msg);
        }
    }

    /// A player left: the match is told.
    fn player_gone(&mut self, player: NetId) {
        if let Some(game) = self.game.as_mut() {
            game.player_left(&self.world, player);
        }
        self.notices.push_back(ServerNotice::Left(player));
    }

    fn receive(&mut self, link: usize) {
        while let Some(event) = self.links[link].listener.poll_event() {
            match event {
                ServerEvent::Connected(peer) => {
                    self.clients.insert(
                        (link, peer),
                        Client {
                            player: None,
                            phase: Phase::AwaitHello,
                            since: self.world.tick(),
                            snapshots: SnapshotEncoder::new(),
                        },
                    );
                }
                ServerEvent::Disconnected(peer) => {
                    if let Some(c) = self.clients.remove(&(link, peer)) {
                        if let (Phase::InGame, Some(id)) = (c.phase, c.player) {
                            self.players.remove(&id);
                            self.player_gone(id);
                        }
                    }
                }
            }
        }
        let conns: Vec<(usize, PeerId)> = self
            .clients
            .range((link, PeerId(0))..=(link, PeerId(u64::MAX)))
            .map(|(k, _)| *k)
            .collect();
        for conn in conns {
            while let Some(bytes) = self.links[link].listener.recv(conn.1, Channel::Event) {
                match Message::decode(&bytes) {
                    Ok(msg) => self.on_message(conn, msg),
                    Err(_) => self.reject(conn, RejectReason::Protocol),
                }
                if !self.clients.contains_key(&conn) {
                    break;
                }
            }
            let Some(client) = self.clients.get_mut(&conn) else {
                continue;
            };
            let Some(player) = client.player else {
                continue;
            };
            let tick = self.world.tick();
            while let Some(bytes) = self.links[link].listener.recv(conn.1, Channel::Input) {
                let Ok(packet) = InputPacket::decode(&bytes) else {
                    continue;
                };
                if let Some(ack) = packet.ack {
                    client.snapshots.acknowledge(ack);
                }
                if let Some(p) = self.players.get_mut(&player) {
                    p.commands.push(packet.command, tick);
                }
            }
        }
    }

    fn on_message(&mut self, conn: (usize, PeerId), msg: Message) {
        let Some(phase) = self.clients.get(&conn).map(|c| c.phase) else {
            return;
        };
        match (phase, msg) {
            (
                Phase::AwaitHello,
                Message::ClientHello {
                    protocol_version,
                    build_fingerprint,
                },
            ) => {
                if protocol_version != PROTOCOL_VERSION {
                    return self.reject(
                        conn,
                        RejectReason::ProtocolVersion {
                            client: protocol_version,
                        },
                    );
                }
                if build_fingerprint != self.config.build_fingerprint {
                    return self.reject(
                        conn,
                        RejectReason::BuildFingerprint {
                            client: build_fingerprint,
                        },
                    );
                }
                let reserved = self.players.len()
                    + self
                        .clients
                        .values()
                        .filter(|c| c.phase == Phase::AwaitReady)
                        .count();
                if reserved >= self.config.limits.max_players {
                    return self.reject(conn, RejectReason::ServerFull);
                }
                let Some(id) = self.allocate_player() else {
                    return self.reject(conn, RejectReason::ServerFull);
                };
                let hello = Message::ServerHello {
                    mode: self
                        .game
                        .as_ref()
                        .map_or(self.config.mode, |g| g.mode_id().0),
                    player_id: id,
                    map_fingerprint: self.config.map.fingerprint,
                    tick_rate: self.config.tick_rate,
                };
                if let Some(c) = self.clients.get_mut(&conn) {
                    c.player = Some(id);
                    c.phase = Phase::AwaitReady;
                    c.since = self.world.tick();
                }
                self.send(conn, Channel::Event, &hello);
            }
            (Phase::AwaitReady, Message::Ready) => {
                let Some(id) = self.clients.get_mut(&conn).and_then(|c| {
                    c.phase = Phase::InGame;
                    c.player
                }) else {
                    return;
                };
                self.spawn(id, Some(conn));
                self.notices.push_back(ServerNotice::Joined(id));
                if let Some(game) = self.game.as_ref() {
                    let state = Message::Event(Event::MatchState(game.state().encode()));
                    self.send(conn, Channel::Event, &state);
                }
                self.send_snapshot(conn);
            }
            (Phase::AwaitReady | Phase::InGame, msg @ Message::MapChunk { .. }) => {
                self.serve_map(conn, &msg);
            }
            (Phase::InGame, Message::Ready) => {
                // The client loaded the map of a `MapChange` (lot 5.1).
                if let Some(r) = self.rotation.as_mut() {
                    r.ready.insert(conn);
                }
            }
            (Phase::InGame, Message::Event(Event::Mode(bytes))) => {
                let player = self.clients.get(&conn).and_then(|c| c.player);
                if let (Some(player), Some(game), Ok(m)) =
                    (player, self.game.as_mut(), ModeMessage::decode(&bytes))
                {
                    game.client_message(player, m);
                }
            }
            (Phase::InGame, Message::Event(_)) => {
                // Chat and other client events are handled by later lots.
            }
            _ => self.reject(conn, RejectReason::Protocol),
        }
    }

    /// Closes a connection that is still in the handshake.
    fn reject(&mut self, conn: (usize, PeerId), reason: RejectReason) {
        tracing::info!(link = conn.0, peer = conn.1 .0, ?reason, "client rejected");
        if let Some(c) = self.clients.remove(&conn) {
            if let (Phase::InGame, Some(id)) = (c.phase, c.player) {
                self.players.remove(&id);
                self.player_gone(id);
            }
        }
        self.links[conn.0].listener.disconnect(conn.1);
        self.notices.push_back(ServerNotice::Rejected(reason));
    }

    fn expire_handshakes(&mut self) {
        let now = self.world.tick().0;
        let limit = self.config.limits.handshake_timeout_ticks;
        let late: Vec<(usize, PeerId)> = self
            .clients
            .iter()
            .filter(|(_, c)| c.phase != Phase::InGame && now.saturating_sub(c.since.0) > limit)
            .map(|(k, _)| *k)
            .collect();
        for conn in late {
            self.reject(conn, RejectReason::Timeout);
        }
    }

    /// A fresh player id. Ids are never reused within a run because the sim
    /// cannot despawn entities yet; a departed player's entity stays inert.
    fn allocate_player(&mut self) -> Option<NetId> {
        let id = self.next_player;
        self.next_player = id.checked_add(1)?;
        Some(NetId(id))
    }

    fn spawn(&mut self, id: NetId, conn: Option<(usize, PeerId)>) {
        let spawns = &self.config.map.spawn_points;
        let at = spawns[usize::from(id.0) % spawns.len()];
        self.world.spawn_player(id, at);
        self.players.insert(
            id,
            Player {
                commands: CommandBuffer::new(self.config.limits.command_buffer_ticks),
                conn,
                last_position: at,
                velocity: Vec3::ZERO,
                yaw: 0.0,
                pitch: 0.0,
                weapon: 0,
            },
        );
        if let Some(game) = self.game.as_mut() {
            game.player_joined(&self.world, id);
        }
    }

    /// Health and alive flag for the snapshot of a player.
    fn vitals(&self, id: NetId) -> (u8, bool) {
        match self.game.as_ref().and_then(|g| g.vitals(id)) {
            Some(Vitals { health, alive, .. }) => {
                (health.round().clamp(0.0, f32::from(u8::MAX)) as u8, alive)
            }
            None => (DEFAULT_HEALTH, true),
        }
    }

    /// Builds the full snapshot for one client.
    fn snapshot_for(&self, player: Option<NetId>) -> Snapshot {
        let entities = self
            .players
            .iter()
            .filter_map(|(&id, p)| {
                let state = self.world.controller_state(id);
                let (health, alive) = self.vitals(id);
                Some(EntityState {
                    id,
                    position: self.world.position(id)?,
                    velocity: p.velocity,
                    yaw: p.yaw,
                    pitch: p.pitch,
                    health,
                    weapon: p.weapon,
                    on_ground: state.is_none_or(|s| s.on_ground),
                    sliding: state.is_some_and(|s| s.slide_timer > 0.0),
                    alive,
                })
            })
            .collect();
        Snapshot {
            tick: self.world.tick(),
            last_command: player
                .and_then(|id| self.players.get(&id))
                .and_then(|p| p.commands.last_processed()),
            entities,
        }
    }

    /// Sends the next snapshot to one client: a delta against the snapshot
    /// it acknowledged, or a full one (see [`SnapshotEncoder`]).
    fn send_snapshot(&mut self, conn: (usize, PeerId)) {
        let player = self.clients.get(&conn).and_then(|c| c.player);
        let snapshot = self.snapshot_for(player);
        let Some(client) = self.clients.get_mut(&conn) else {
            return;
        };
        match client.snapshots.encode(&snapshot) {
            Ok(msg) => self.send(conn, Channel::Snapshot, &msg),
            Err(e) => tracing::error!(error = %e, "snapshot encoding failed"),
        }
    }

    fn send(&mut self, conn: (usize, PeerId), channel: Channel, msg: &Message) {
        let bytes = match msg.encode() {
            Ok(b) => b,
            Err(e) => return tracing::error!(error = %e, "message encoding failed"),
        };
        if let Err(e) = self.links[conn.0].listener.send(conn.1, channel, &bytes) {
            tracing::warn!(link = conn.0, peer = conn.1 .0, error = %e, "send failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Walker;
    impl Bot for Walker {
        fn command(&mut self, _: &World, _: NetId, tick: Tick) -> InputCommand {
            InputCommand {
                tick,
                buttons: 0,
                move_x: 0,
                move_y: 127,
                yaw: 0,
                pitch: 0,
                weapon_slot: 2,
            }
        }
    }

    #[test]
    fn bot_commands_drive_the_world() {
        let mut s = Server::new(ServerConfig::default(), Vec::new()).unwrap();
        let id = s.add_bot(Box::new(Walker)).unwrap();
        let start = s.world().position(id).unwrap();
        for _ in 0..60 {
            s.tick();
        }
        let end = s.world().position(id).unwrap();
        // Yaw 0 faces -Z; 1 s of the character controller at 7 m/s, the
        // first ticks accelerating.
        let run = s.config().move_params.run_speed;
        let moved = start.z - end.z;
        assert!(moved > run - 1.0 && moved < run, "{start} -> {end}");
        let snap = s.snapshot_for(None);
        let e = snap.entity(id).unwrap();
        assert_eq!(e.weapon, 2);
        assert!((e.velocity.z + run).abs() < 0.05, "{}", e.velocity);
        assert!(e.on_ground && e.alive);
        assert_eq!(snap.tick, Tick(60));
    }

    #[test]
    fn every_tick_records_the_hitboxes_under_the_snapshot_label() {
        let mut s = Server::new(ServerConfig::default(), Vec::new()).unwrap();
        let id = s.add_bot(Box::new(Walker)).unwrap();
        for _ in 0..90 {
            s.tick();
        }
        let lag = s.lag_compensation();
        assert_eq!(lag.now(), Some(s.world().tick()));
        assert_eq!(lag.history(id).count(), crate::HISTORY_TICKS);
        let hb = lag.authoritative(id).unwrap();
        assert!((hb.position - s.world().position(id).unwrap()).length() < 1e-4);
        assert_eq!(s.player_rtt(id), Some(0.0));
        // A bot shooting straight up hits nothing and leaves no trace.
        let fire = FireCommand {
            view_tick: Tick(85),
            view_fraction: 0.5,
            direction: Vec3::Y,
            range: 50.0,
        };
        let out = s.rewind_and_cast(id, &fire).unwrap();
        assert!(out.hit.is_none());
        assert_eq!(s.lag_compensation().hitbox(id), Some(hb));
        assert!(s.disconnect(id));
        s.tick();
        assert_eq!(s.lag_compensation().history(id).count(), 0);
    }

    #[test]
    fn bots_respect_max_players() {
        let mut config = ServerConfig::default();
        config.limits.max_players = 1;
        let mut s = Server::new(config, Vec::new()).unwrap();
        assert!(s.add_bot(Box::new(Walker)).is_some());
        assert!(s.add_bot(Box::new(Walker)).is_none());
        let id = s.players().next().unwrap();
        assert!(s.disconnect(id));
        assert_eq!(s.poll_notice(), Some(ServerNotice::Left(id)));
        assert_eq!(s.players().count(), 0);
    }
}

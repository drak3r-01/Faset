//! The server: connections, handshake, command buffers, tick and snapshots.

use std::collections::{BTreeMap, VecDeque};
use std::time::Duration;

use faset_core::glam::Vec3;
use faset_core::{NetId, Tick, DT, TICK_RATE};
use faset_net::{Channel, PeerId, ServerEvent};
use faset_protocol::quant::{dequantize_pitch_command, dequantize_yaw};
use faset_protocol::{InputCommand, Message, PROTOCOL_VERSION};
use faset_sim::World;

use crate::commands::CommandBuffer;
use crate::config::{ConfigError, ServerConfig};
use crate::link::ServerTransport;
use crate::snapshot::{EntityState, Snapshot};

/// Health reported for every player until the sim exposes damage (lots 4.x).
const STUB_HEALTH: u8 = 100;

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

/// An internal virtual client. Its commands go through the same command
/// buffer as a human's (section 7). Only the hook exists in this lot.
pub trait Bot: Send {
    /// The command of player `me` for tick `tick`, read at the start of that
    /// tick.
    fn command(&mut self, world: &World, me: NetId, tick: Tick) -> InputCommand;
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
}

impl std::fmt::Debug for Server {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Server")
            .field("tick", &self.world.tick())
            .field("links", &self.links.len())
            .field("clients", &self.clients.len())
            .field("players", &self.players.len())
            .field("bots", &self.bots.len())
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
        let world = World::with_statics(config.seed, config.map.statics.clone());
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
        })
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
        self.notices.push_back(ServerNotice::Left(player));
        true
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

    /// Advances exactly one tick:
    /// 1. receive: connection changes, handshake messages, commands;
    /// 2. read one command per player from its buffer (repeat if missing);
    /// 3. step the world;
    /// 4. send full snapshots when due, then flush.
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
        for (&id, bot) in &mut self.bots {
            let cmd = bot.command(&self.world, id, tick);
            if let Some(p) = self.players.get_mut(&id) {
                p.commands.push(cmd, tick);
            }
        }
        let mut inputs = Vec::with_capacity(self.players.len());
        for (&id, p) in &mut self.players {
            if let Some(cmd) = p.commands.take(tick) {
                p.yaw = dequantize_yaw(cmd.yaw);
                p.pitch = dequantize_pitch_command(cmd.pitch);
                p.weapon = cmd.weapon_slot;
                inputs.push((id, cmd));
            }
        }
        self.world.step(&inputs);
        for (&id, p) in &mut self.players {
            if let Some(pos) = self.world.position(id) {
                p.velocity = (pos - p.last_position) / DT;
                p.last_position = pos;
            }
        }

        self.snapshot_acc += u32::from(self.config.snapshot_rate);
        if self.snapshot_acc >= u32::from(TICK_RATE) {
            self.snapshot_acc -= u32::from(TICK_RATE);
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
                        },
                    );
                }
                ServerEvent::Disconnected(peer) => {
                    if let Some(c) = self.clients.remove(&(link, peer)) {
                        if let (Phase::InGame, Some(id)) = (c.phase, c.player) {
                            self.players.remove(&id);
                            self.notices.push_back(ServerNotice::Left(id));
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
            let Some(player) = self.clients.get(&conn).and_then(|c| c.player) else {
                continue;
            };
            let tick = self.world.tick();
            while let Some(bytes) = self.links[link].listener.recv(conn.1, Channel::Input) {
                let (Ok(cmd), Some(p)) =
                    (InputCommand::decode(&bytes), self.players.get_mut(&player))
                else {
                    continue;
                };
                p.commands.push(cmd, tick);
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
                    mode: self.config.mode,
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
                self.send_snapshot(conn);
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
                self.notices.push_back(ServerNotice::Left(id));
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
    }

    /// Builds the full snapshot for one client.
    fn snapshot_for(&self, player: Option<NetId>) -> Snapshot {
        let entities = self
            .players
            .iter()
            .filter_map(|(&id, p)| {
                Some(EntityState {
                    id,
                    position: self.world.position(id)?,
                    velocity: p.velocity,
                    yaw: p.yaw,
                    pitch: p.pitch,
                    health: STUB_HEALTH,
                    weapon: p.weapon,
                    on_ground: true,
                    sliding: false,
                    alive: true,
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

    fn send_snapshot(&mut self, conn: (usize, PeerId)) {
        let player = self.clients.get(&conn).and_then(|c| c.player);
        match self.snapshot_for(player).to_message() {
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
        // Yaw 0 faces -Z; 1 s at the 5 m/s stub speed.
        assert!((start.z - end.z - 5.0).abs() < 0.05, "{start} -> {end}");
        let snap = s.snapshot_for(None);
        let e = snap.entity(id).unwrap();
        assert_eq!(e.weapon, 2);
        assert!((e.velocity.z + 5.0).abs() < 0.05);
        assert_eq!(snap.tick, Tick(60));
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

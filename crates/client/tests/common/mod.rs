//! A headless predicting client and a server, joined by conditioned
//! in-memory links and driven on one virtual clock.
//!
//! The client glue lives here because the snapshot and input packet codecs
//! belong to `faset-server`, a dev-dependency only: the library sees decoded
//! [`AuthoritativeState`]s and returns plain commands.

// Each test uses a different part of this helper.
#![allow(dead_code)]

use std::time::Duration;

use faset_client::{
    AuthoritativeState, ClockSync, PredictedState, Prediction, ReconcileOutcome,
    RemoteInterpolation, RemoteState, VisualOffset,
};
use faset_core::glam::Vec3;
use faset_core::{NetId, Tick, DT};
use faset_net::{
    Channel, ClientEndpoint, ConditionedTransport, MemoryAddr, MemoryNetwork, MemoryTransport,
    NetworkConditions,
};
use faset_protocol::{InputCommand, Message, PROTOCOL_VERSION};
use faset_server::{
    build_fingerprint, InputPacket, MapConfig, Server, ServerConfig, SnapshotDecoder,
};
use faset_sim::StaticWorld;

mod fleet;
#[allow(unused_imports)]
pub use fleet::*;

pub type Link = ConditionedTransport<MemoryTransport>;

/// Client frame length: four frames per server tick (240 Hz).
pub const FRAME: Duration = Duration::from_nanos(4_166_667);

/// Fastest walk per tick, metres: the controller runs at 7 m/s and briefly
/// overshoots to about 7.3 m/s when the walk turns (ground acceleration
/// along the new direction before friction catches up).
pub const MAX_STEP: f32 = 7.5 * DT;

/// Movement of one tick: `(move_x, move_y, yaw)`.
pub type Movement = (i8, i8, u16);

/// What one client frame did.
#[derive(Debug, Clone, Copy)]
pub struct FrameLog {
    pub timescale: f64,
    /// Ticks simulated this frame.
    pub ticks: u32,
    /// Displayed position at the end of the frame.
    pub display: Option<Vec3>,
    /// Visual offset after the snapshots, before this frame's ticks.
    pub offset_before_ticks: Vec3,
    /// Visual offset at the end of the frame.
    pub offset: Vec3,
    /// Whether a snapshot corrected the prediction this frame.
    pub corrected: bool,
    pub snapped: bool,
}

pub struct PredictedClient {
    endpoint: ClientEndpoint<Link>,
    hello_sent: bool,
    seed: u64,
    statics: StaticWorld,
    decoder: SnapshotDecoder,
    pub player: Option<NetId>,
    pub clock: ClockSync,
    pub prediction: Option<Prediction>,
    pub outcomes: Vec<ReconcileOutcome>,
    /// Every simulated tick, in order.
    pub ticks: Vec<Tick>,
    pub frames: Vec<FrameLog>,
    /// Snapshots received in game, and those whose state tick the server
    /// had no command of ours for (late or lost).
    pub snapshots: u32,
    pub late: u32,
    /// The other players, interpolated.
    pub remotes: RemoteInterpolation,
    /// Local real time in seconds (start of the current frame).
    pub now: f64,
}

impl PredictedClient {
    pub fn new(link: Link, server: MemoryAddr, seed: u64, statics: StaticWorld) -> Self {
        Self {
            endpoint: ClientEndpoint::new(link, server),
            hello_sent: false,
            seed,
            statics,
            decoder: SnapshotDecoder::new(),
            player: None,
            clock: ClockSync::default(),
            prediction: None,
            outcomes: Vec::new(),
            ticks: Vec::new(),
            frames: Vec::new(),
            snapshots: 0,
            late: 0,
            remotes: RemoteInterpolation::default(),
            now: 0.0,
        }
    }

    pub fn link_mut(&mut self) -> &mut Link {
        self.endpoint.transport_mut()
    }

    pub fn in_game(&self) -> bool {
        self.prediction.is_some()
    }

    fn send_event(&mut self, msg: &Message) {
        let bytes = msg.encode().unwrap();
        self.endpoint.send(Channel::Event, &bytes).unwrap();
    }

    fn on_snapshot(&mut self, bytes: &[u8], log: &mut FrameLog) {
        let Ok(snap) = self.decoder.decode_bytes(bytes) else {
            return;
        };
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
        self.clock.observe_rtt(self.endpoint.rtt());
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
            let mut p = Prediction::new(me, self.seed, self.statics.clone());
            p.start(self.clock.next_tick(), server.state);
            self.prediction = Some(p);
            return;
        }
        let Some(p) = self.prediction.as_mut() else {
            return;
        };
        let first = self.ticks.first().copied();
        if first.is_some_and(|f| snap.tick.0 > f.0 + 1) {
            self.snapshots += 1;
            if snap.last_command != server.state_tick() {
                self.late += 1;
            }
        }
        if let Some(out) = p.reconcile(&server) {
            log.corrected |= out.corrected;
            log.snapped |= out.snapped;
            self.outcomes.push(out);
        }
    }

    /// One client frame of `real` time: receive, reconcile, simulate the
    /// ticks due, send the newest unacknowledged commands, flush.
    pub fn frame(&mut self, real: Duration, input: &mut dyn FnMut(Tick) -> Movement) {
        self.now += real.as_secs_f64();
        if !self.hello_sent {
            self.hello_sent = true;
            self.send_event(&Message::ClientHello {
                protocol_version: PROTOCOL_VERSION,
                build_fingerprint: build_fingerprint(),
            });
        }
        self.endpoint.update(real).unwrap();
        while let Some(bytes) = self.endpoint.recv(Channel::Event) {
            if let Ok(Message::ServerHello { player_id, .. }) = Message::decode(&bytes) {
                self.player = Some(player_id);
                self.send_event(&Message::Ready);
            }
        }
        let mut log = FrameLog {
            timescale: 1.0,
            ticks: 0,
            display: None,
            offset_before_ticks: Vec3::ZERO,
            offset: Vec3::ZERO,
            corrected: false,
            snapped: false,
        };
        while let Some(bytes) = self.endpoint.recv(Channel::Snapshot) {
            self.on_snapshot(&bytes, &mut log);
        }
        log.offset_before_ticks = self.offset();

        let due: Vec<Tick> = self.clock.advance(real).collect();
        log.timescale = self.clock.timescale();
        log.ticks = due.len() as u32;
        if let Some(p) = self.prediction.as_mut() {
            for &tick in &due {
                let (move_x, move_y, yaw) = input(tick);
                p.predict(InputCommand {
                    tick,
                    buttons: 0,
                    move_x,
                    move_y,
                    yaw,
                    pitch: 0,
                    weapon_slot: 1,
                })
                .unwrap();
                self.ticks.push(tick);
            }
            if !due.is_empty() {
                for command in p.unacknowledged() {
                    let packet = InputPacket {
                        command,
                        ack: self.decoder.ack(),
                    };
                    // A full channel only loses this copy; the next packet
                    // repeats the command.
                    let _ = self.endpoint.send(Channel::Input, &packet.encode());
                }
            }
        }
        self.endpoint.update(Duration::ZERO).unwrap();
        log.display = self
            .prediction
            .as_ref()
            .and_then(Prediction::display_position);
        log.offset = self.offset();
        self.frames.push(log);
    }

    pub fn offset(&self) -> Vec3 {
        self.prediction
            .as_ref()
            .map_or(Vec3::ZERO, |p| p.visual_offset().value())
    }
}

/// One server and one predicting client.
pub struct Rig {
    pub server: Server,
    pub client: PredictedClient,
    now: Duration,
    next_server_tick: Duration,
}

impl Rig {
    /// Both directions degraded by `conditions` (independent random
    /// streams).
    pub fn new(conditions: NetworkConditions) -> Self {
        Self::with_map(conditions, ServerConfig::default().map)
    }

    /// [`Rig::new`] on another map (lot 6.10: terrain).
    pub fn with_map(conditions: NetworkConditions, map: MapConfig) -> Self {
        let net = MemoryNetwork::new();
        let server_end = net.endpoint();
        let addr = server_end.addr();
        let config = ServerConfig {
            map,
            ..ServerConfig::default()
        };
        let (seed, statics) = (config.seed, config.map.statics.clone());
        let server_link = ConditionedTransport::new(
            server_end,
            NetworkConditions {
                seed: conditions.seed ^ 0xA5A5_5A5A,
                ..conditions.clone()
            },
        );
        let server = Server::new(config, vec![server_link.into()]).unwrap();
        let client_link = ConditionedTransport::new(net.endpoint(), conditions);
        Self {
            server,
            client: PredictedClient::new(client_link, addr, seed, statics),
            now: Duration::ZERO,
            next_server_tick: Duration::ZERO,
        }
    }

    /// Runs `frames` client frames, ticking the server in real time.
    pub fn run(&mut self, frames: u32, input: &mut dyn FnMut(Tick) -> Movement) {
        let server_dt = Duration::from_secs_f32(DT);
        for _ in 0..frames {
            self.now += FRAME;
            self.client.frame(FRAME, input);
            while self.next_server_tick <= self.now {
                self.server.tick();
                self.next_server_tick += server_dt;
            }
        }
    }

    /// Runs until the client predicts (at most 10 s).
    pub fn join(&mut self) {
        let mut still = |_| (0, 0, 0);
        for _ in 0..2400 {
            self.run(1, &mut still);
            if self.client.in_game() {
                return;
            }
        }
        panic!("the client never entered the game");
    }

    /// Frames for `seconds` of real time.
    pub fn frames(seconds: f32) -> u32 {
        (seconds / DT * 4.0).round() as u32
    }
}

/// A walk that keeps changing direction, facing -Z: forward, right, back,
/// left, still, diagonal and back along the diagonal, 45 ticks each. A cycle
/// returns to its start, so the walk stays in the open middle of the test
/// arena: pressed against its wall, the stub movement of `faset-sim` sticks
/// from the server's exact position but slides from the client's quantized
/// copy, which no reconciliation can hide (reported in lot 1.6).
pub fn walk(tick: Tick) -> Movement {
    match (tick.0 / 45) % 7 {
        0 => (0, 127, 0),
        1 => (127, 0, 0),
        2 => (0, -127, 0),
        3 => (-127, 0, 0),
        4 => (0, 0, 0),
        5 => (90, 90, 0),
        _ => (-90, -90, 0),
    }
}

/// Decay of the visual offset over `n` ticks.
pub fn decay(n: u32) -> f32 {
    VisualOffset::decay_factor().powi(n as i32)
}

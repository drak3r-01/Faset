//! Several predicting clients and server-side bots on one server, for the
//! network integration and endurance tests (lot 1.10).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use faset_core::{NetId, Tick, DT};
use faset_net::{
    ConditionedTransport, Datagram, MemoryAddr, MemoryNetwork, MemoryTransport, NetworkConditions,
    Transport, TransportError,
};
use faset_protocol::InputCommand;
use faset_server::{Bot, Server, ServerConfig};
use faset_sim::{StaticWorld, World};

use super::{Movement, PredictedClient, FRAME};

/// IPv4 + UDP header bytes added to every datagram when measuring bandwidth.
pub const IP_UDP_HEADER: u64 = 28;

/// Counts the bytes and datagrams the wrapped transport sends.
pub struct Counting {
    inner: MemoryTransport,
    bytes: Arc<AtomicU64>,
    packets: Arc<AtomicU64>,
}

impl Transport for Counting {
    type Addr = MemoryAddr;

    fn send_to(&mut self, to: MemoryAddr, packet: &[u8]) -> Result<(), TransportError> {
        self.bytes.fetch_add(packet.len() as u64, Ordering::Relaxed);
        self.packets.fetch_add(1, Ordering::Relaxed);
        self.inner.send_to(to, packet)
    }

    fn poll(&mut self) -> Result<Option<Datagram<MemoryAddr>>, TransportError> {
        self.inner.poll()
    }

    fn update(&mut self, dt: Duration) {
        self.inner.update(dt);
    }
}

/// A bot that runs and turns on its own schedule, so every tick changes
/// positions, velocities and yaw. Past 30 m from the centre it heads back,
/// so it never runs off the 50 m test arena floor and falls.
pub struct Wanderer(pub u32);

impl Bot for Wanderer {
    fn command(&mut self, world: &World, me: NetId, tick: Tick) -> InputCommand {
        let leg = (tick.0 + self.0 * 37) / 90;
        let mut yaw = leg as f32 * 2.1 + self.0 as f32;
        if let Some(p) = world.position(me).filter(|p| p.x.hypot(p.z) > 30.0) {
            // Yaw 0 faces -Z: face the origin.
            yaw = p.x.atan2(p.z);
        }
        InputCommand {
            tick,
            buttons: 0,
            move_x: if leg.is_multiple_of(3) { 64 } else { 0 },
            move_y: 127,
            yaw: faset_protocol::quant::quantize_yaw(yaw),
            pitch: 0,
            weapon_slot: (leg % 4) as u8,
        }
    }
}

/// One server (bots included) and several predicting clients, all on
/// degraded links and one virtual clock.
pub struct Fleet {
    pub server: Server,
    pub clients: Vec<PredictedClient>,
    net: MemoryNetwork,
    server_addr: MemoryAddr,
    conditions: NetworkConditions,
    joined: u64,
    seed: u64,
    statics: StaticWorld,
    /// Bytes and datagrams the server has sent (before loss).
    pub bytes: Arc<AtomicU64>,
    pub packets: Arc<AtomicU64>,
    now: Duration,
    next_server_tick: Duration,
}

impl Fleet {
    /// `bots` server-side bots, then `humans` predicting clients.
    pub fn new(humans: usize, bots: u32, conditions: NetworkConditions) -> Self {
        let net = MemoryNetwork::new();
        let server_end = net.endpoint();
        let server_addr = server_end.addr();
        let config = ServerConfig::default();
        let (seed, statics) = (config.seed, config.map.statics.clone());
        let bytes = Arc::new(AtomicU64::new(0));
        let packets = Arc::new(AtomicU64::new(0));
        let counting = Counting {
            inner: server_end,
            bytes: bytes.clone(),
            packets: packets.clone(),
        };
        let link = ConditionedTransport::new(
            counting,
            NetworkConditions {
                seed: conditions.seed ^ 0xA5A5_5A5A,
                ..conditions.clone()
            },
        );
        let mut server = Server::new(config, vec![link.into()]).unwrap();
        for i in 0..bots {
            server.add_bot(Box::new(Wanderer(i))).unwrap();
        }
        let mut fleet = Self {
            server,
            clients: Vec::new(),
            net,
            server_addr,
            conditions,
            joined: 0,
            seed,
            statics,
            bytes,
            packets,
            now: Duration::ZERO,
            next_server_tick: Duration::ZERO,
        };
        for _ in 0..humans {
            fleet.add_client();
        }
        fleet
    }

    /// A new predicting client with its own random streams.
    pub fn add_client(&mut self) -> usize {
        self.joined += 1;
        let conditions = NetworkConditions {
            seed: self.conditions.seed.wrapping_add(self.joined * 7919),
            ..self.conditions.clone()
        };
        let link = ConditionedTransport::new(self.net.endpoint(), conditions);
        self.clients.push(PredictedClient::new(
            link,
            self.server_addr,
            self.seed,
            self.statics.clone(),
        ));
        self.clients.len() - 1
    }

    /// Replaces client `i` by a fresh one; the old connection times out.
    pub fn reconnect(&mut self, i: usize) {
        let n = self.add_client();
        self.clients.swap(i, n);
        self.clients.pop();
    }

    /// Runs `frames` client frames. `input(client, tick)` is that client's
    /// movement for the tick.
    pub fn run(&mut self, frames: u32, input: &mut dyn FnMut(usize, Tick) -> Movement) {
        let server_dt = Duration::from_secs_f32(DT);
        for _ in 0..frames {
            self.now += FRAME;
            for (i, c) in self.clients.iter_mut().enumerate() {
                c.frame(FRAME, &mut |t| input(i, t));
            }
            while self.next_server_tick <= self.now {
                self.server.tick();
                self.next_server_tick += server_dt;
            }
        }
    }

    /// Runs until every client predicts (at most 10 s).
    pub fn join(&mut self) {
        for _ in 0..2400 {
            self.run(1, &mut |_, _| (0, 0, 0));
            if self.clients.iter().all(PredictedClient::in_game) {
                return;
            }
        }
        panic!("not every client entered the game");
    }
}

/// Resident set size of this process in KiB (Linux only).
pub fn rss_kib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmRSS:"))?;
    line.split_whitespace().nth(1)?.parse().ok()
}

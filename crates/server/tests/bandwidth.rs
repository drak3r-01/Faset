//! Lot 1.5 acceptance: downlink per client at 16 players stays below
//! 100 kbit/s with delta snapshots.

mod common;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use common::HeadlessClient;
use faset_core::{NetId, Tick, TICK_RATE};
use faset_net::{Datagram, MemoryAddr, MemoryNetwork, MemoryTransport, Transport, TransportError};
use faset_protocol::quant::quantize_yaw;
use faset_protocol::InputCommand;
use faset_server::{Bot, Server, ServerConfig};
use faset_sim::World;

/// IPv4 + UDP header bytes of every datagram, added to the measurement.
const IP_UDP_HEADER: u64 = 28;

/// Counts every datagram and byte the wrapped transport receives.
struct Counting {
    inner: MemoryTransport,
    bytes: Arc<AtomicU64>,
    packets: Arc<AtomicU64>,
}

impl Transport for Counting {
    type Addr = MemoryAddr;

    fn send_to(&mut self, to: MemoryAddr, packet: &[u8]) -> Result<(), TransportError> {
        self.inner.send_to(to, packet)
    }

    fn poll(&mut self) -> Result<Option<Datagram<MemoryAddr>>, TransportError> {
        let d = self.inner.poll()?;
        if let Some(d) = &d {
            self.bytes.fetch_add(d.data.len() as u64, Ordering::Relaxed);
            self.packets.fetch_add(1, Ordering::Relaxed);
        }
        Ok(d)
    }

    fn update(&mut self, dt: Duration) {
        self.inner.update(dt);
    }
}

/// Runs forward and turns every 1.5 s, each bot on its own schedule, so
/// positions change every tick and velocity, yaw and weapon change often.
struct Wanderer(u32);

impl Bot for Wanderer {
    fn command(&mut self, _: &World, _: NetId, tick: Tick) -> InputCommand {
        let leg = (tick.0 + self.0 * 37) / 90;
        InputCommand {
            tick,
            buttons: 0,
            move_x: if leg.is_multiple_of(3) { 64 } else { 0 },
            move_y: 127,
            yaw: quantize_yaw(leg as f32 * 2.1 + self.0 as f32),
            pitch: 0,
            weapon_slot: (leg % 4) as u8,
        }
    }
}

#[test]
fn downlink_at_sixteen_players_is_below_100_kbit_per_second() {
    let net = MemoryNetwork::new();
    let server_t = net.endpoint();
    let addr = server_t.addr();
    let config = ServerConfig::default();
    assert_eq!(config.snapshot_rate, 30);
    let mut server = Server::new(config, vec![server_t.into()]).unwrap();
    for i in 0..15 {
        server.add_bot(Box::new(Wanderer(i))).unwrap();
    }
    let bytes = Arc::new(AtomicU64::new(0));
    let packets = Arc::new(AtomicU64::new(0));
    let mut c = HeadlessClient::new(
        Counting {
            inner: net.endpoint(),
            bytes: bytes.clone(),
            packets: packets.clone(),
        },
        addr,
    );
    while !c.in_game() {
        c.step(None);
        server.tick();
    }
    assert_eq!(server.players().count(), 16);
    // Settle for one second, then measure ten.
    let walk = Some((0, 127, quantize_yaw(1.0)));
    for _ in 0..TICK_RATE {
        c.step(walk);
        server.tick();
    }
    let start = (
        bytes.load(Ordering::Relaxed),
        packets.load(Ordering::Relaxed),
        c.snapshot_bytes,
        c.fulls,
        c.deltas,
        c.snapshots.len(),
    );
    let seconds: u32 = 10;
    for _ in 0..u32::from(TICK_RATE) * seconds {
        c.step(walk);
        server.tick();
    }
    let secs = f64::from(seconds);
    let wire = bytes.load(Ordering::Relaxed) - start.0;
    let datagrams = packets.load(Ordering::Relaxed) - start.1;
    let with_ip = wire + IP_UDP_HEADER * datagrams;
    let snap_bytes = c.snapshot_bytes - start.2;
    let (fulls, deltas) = (c.fulls - start.3, c.deltas - start.4);
    let full_only: usize = c.snapshots[start.5..]
        .iter()
        .map(|s| s.encode().unwrap().len())
        .sum();
    let kbit = with_ip as f64 * 8.0 / secs / 1000.0;
    let latest = c.latest().unwrap();
    println!(
        "16 players, 30 Hz snapshots, {seconds} s measured: downlink {kbit:.1} kbit/s \
         per client including IP/UDP headers ({:.0} B/s, {:.0} B/s without them); \
         snapshot messages {:.0} B/s, {:.1} B per entity per snapshot; \
         {deltas} deltas, {fulls} full; full snapshots only would be {:.0} B/s",
        with_ip as f64 / secs,
        wire as f64 / secs,
        snap_bytes as f64 / secs,
        snap_bytes as f64 / (fulls + deltas) as f64 / latest.entities.len() as f64,
        full_only as f64 / secs,
    );
    assert_eq!(latest.entities.len(), 16);
    assert_eq!(fulls + deltas, 30 * seconds as usize);
    assert!(deltas >= fulls * 10, "{deltas} deltas, {fulls} full");
    assert!(kbit < 100.0, "downlink {kbit:.1} kbit/s");
    for e in &latest.entities {
        let truth = server.world().position(e.id).unwrap();
        // The latest snapshot lags the server by at most a couple of ticks.
        assert!((e.position - truth).abs().max_element() < 0.5, "{:?}", e.id);
    }
}

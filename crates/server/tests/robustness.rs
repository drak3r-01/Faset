//! Lot 1.10 robustness: random bytes and truncated copies of valid packets
//! reach the server through a transport; it must never panic, decoding
//! returns `Err`, flooding stays bounded and honest clients are unaffected.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::HeadlessClient;
use faset_core::glam::Vec3;
use faset_core::{NetId, Tick, DT};
use faset_net::{
    Channel, ClientEndpoint, Datagram, MemoryAddr, MemoryNetwork, MemoryTransport, Transport,
    TransportError,
};
use faset_protocol::{InputCommand, Message, PROTOCOL_VERSION};
use faset_server::{
    build_fingerprint, EntityState, InputPacket, Server, ServerConfig, Snapshot, SnapshotDecoder,
    SnapshotEncoder,
};
use proptest::prelude::*;

/// Cases of each property test.
const CASES: u32 = 10_000;

fn command(tick: u32) -> InputCommand {
    InputCommand {
        tick: Tick(tick),
        buttons: 0,
        move_x: 10,
        move_y: 100,
        yaw: 1234,
        pitch: 0,
        weapon_slot: 1,
    }
}

fn snapshot(tick: u32, shift: f32) -> Snapshot {
    Snapshot {
        tick: Tick(tick),
        last_command: Some(Tick(tick - 1)),
        entities: (0..6)
            .map(|i| EntityState {
                id: NetId(i),
                position: Vec3::new(i as f32 + shift, 0.9, 2.0),
                velocity: Vec3::new(shift, 0.0, 1.0),
                yaw: 1.0,
                pitch: 0.1,
                health: 100,
                weapon: 1,
                on_ground: true,
                sliding: false,
                alive: true,
            })
            .collect(),
    }
}

/// A full and a delta snapshot message, as bytes.
fn valid_snapshots() -> Vec<Vec<u8>> {
    let mut enc = SnapshotEncoder::new();
    let full = enc.encode(&snapshot(10, 0.0)).unwrap();
    enc.acknowledge(Tick(10));
    let delta = enc.encode(&snapshot(11, 0.3)).unwrap();
    vec![full.encode().unwrap(), delta.encode().unwrap()]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(CASES))]

    /// Every length but 13 and 17 is an error; nothing panics.
    #[test]
    fn input_packet_decoding_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..64)) {
        let r = InputPacket::decode(&bytes);
        if bytes.len() != InputCommand::SIZE && bytes.len() != InputPacket::SIZE {
            prop_assert!(r.is_err());
        }
    }

    /// Truncated valid input packets are errors.
    #[test]
    fn truncated_input_packets_are_errors(tick in any::<u32>(), ack in any::<Option<u32>>(), cut in 0usize..17) {
        let bytes = InputPacket { command: command(tick), ack: ack.map(Tick) }.encode();
        if cut < bytes.len() && cut != InputCommand::SIZE {
            prop_assert!(InputPacket::decode(&bytes[..cut]).is_err());
        }
    }

    /// Random bytes and corrupted or truncated real snapshots never panic
    /// the snapshot decoder, whatever baselines it already holds.
    #[test]
    fn snapshot_decoding_never_panics(
        random in proptest::collection::vec(any::<u8>(), 0..600),
        which in 0usize..2,
        flip in any::<(usize, u8)>(),
        cut in 0usize..400,
    ) {
        let mut dec = SnapshotDecoder::new();
        let _ = dec.decode_bytes(&random);
        let valid = valid_snapshots();
        // Give the delta its baseline, then feed damaged copies.
        dec.decode_bytes(&valid[0]).unwrap();
        let bytes = &valid[which];
        let _ = dec.decode_bytes(&bytes[..cut.min(bytes.len().saturating_sub(1))]);
        let mut corrupted = bytes.clone();
        let at = flip.0 % corrupted.len();
        corrupted[at] ^= flip.1;
        let _ = dec.decode_bytes(&corrupted);
    }
}

#[test]
fn truncated_snapshots_are_errors() {
    let valid = valid_snapshots();
    for bytes in &valid {
        for n in 0..bytes.len() - 4 {
            let mut dec = SnapshotDecoder::new();
            dec.decode_bytes(&valid[0]).unwrap();
            // Too short to hold the entities: an error, not a partial
            // snapshot.
            assert!(dec.decode_bytes(&bytes[..n]).is_err(), "cut at {n}");
        }
    }
}

fn setup() -> (Server, MemoryNetwork, MemoryAddr) {
    let net = MemoryNetwork::new();
    let server_t = net.endpoint();
    let addr = server_t.addr();
    let server = Server::new(ServerConfig::default(), vec![server_t.into()]).unwrap();
    (server, net, addr)
}

/// Deterministic noise.
fn noise(len: usize, mut s: u64) -> Vec<u8> {
    (0..len)
        .map(|_| {
            s = s
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (s >> 33) as u8
        })
        .collect()
}

/// Records the datagrams an honest client sends, to truncate and corrupt.
struct Tap {
    inner: MemoryTransport,
    seen: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl Transport for Tap {
    type Addr = MemoryAddr;
    fn send_to(&mut self, to: MemoryAddr, packet: &[u8]) -> Result<(), TransportError> {
        self.seen.lock().unwrap().push(packet.to_vec());
        self.inner.send_to(to, packet)
    }
    fn poll(&mut self) -> Result<Option<Datagram<MemoryAddr>>, TransportError> {
        self.inner.poll()
    }
    fn update(&mut self, dt: Duration) {
        self.inner.update(dt);
    }
}

fn hello() -> Vec<u8> {
    Message::ClientHello {
        protocol_version: PROTOCOL_VERSION,
        build_fingerprint: build_fingerprint(),
    }
    .encode()
    .unwrap()
}

#[test]
fn ten_thousand_random_and_truncated_datagrams_do_not_hurt_the_server() {
    let (mut server, net, addr) = setup();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut honest = HeadlessClient::new(
        Tap {
            inner: net.endpoint(),
            seen: seen.clone(),
        },
        addr,
    );
    for _ in 0..300 {
        honest.step(Some((0, 100, 0)));
        server.tick();
    }
    assert!(honest.in_game());
    let captured = seen.lock().unwrap().clone();
    assert!(captured.len() > 50, "{} datagrams captured", captured.len());

    let mut attacker = net.endpoint();
    for i in 0..10_000usize {
        let datagram = match i % 4 {
            // Pure noise of every size up to a bit more than one MTU.
            0 => noise(i % 1200, i as u64),
            // Truncated copies of real datagrams.
            1 => {
                let d = &captured[i % captured.len()];
                d[..(i / 7) % d.len().max(1)].to_vec()
            }
            // Real datagrams with a corrupted byte.
            2 => {
                let mut d = captured[i % captured.len()].clone();
                if !d.is_empty() {
                    let at = (i * 31) % d.len();
                    d[at] ^= (i % 255) as u8 + 1;
                }
                d
            }
            // Real datagrams followed by garbage.
            _ => {
                let mut d = captured[i % captured.len()].clone();
                d.extend(noise(i % 64, i as u64));
                d.truncate(1200);
                d
            }
        };
        attacker.send_to(addr, &datagram).unwrap();
        if i % 50 == 49 {
            honest.step(Some((0, 100, 0)));
            server.tick();
        }
    }
    // The honest client is still served and the server is still sane.
    let before = honest.snapshots.len();
    for _ in 0..120 {
        honest.step(Some((0, 100, 0)));
        server.tick();
    }
    assert!(honest.snapshots.len() > before + 30);
    assert!(server.players().any(|p| Some(p) == honest.player));
}

/// An endpoint that completes the handshake by hand.
fn rogue_in_game(
    net: &MemoryNetwork,
    addr: MemoryAddr,
    server: &mut Server,
) -> ClientEndpoint<MemoryTransport> {
    let mut rogue = ClientEndpoint::new(net.endpoint(), addr);
    rogue.send(Channel::Event, &hello()).unwrap();
    for _ in 0..60 {
        rogue.update(Duration::from_secs_f32(DT)).unwrap();
        server.tick();
        while let Some(bytes) = rogue.recv(Channel::Event) {
            if let Ok(Message::ServerHello { .. }) = Message::decode(&bytes) {
                rogue
                    .send(Channel::Event, &Message::Ready.encode().unwrap())
                    .unwrap();
            }
        }
    }
    rogue
}

#[test]
fn garbage_on_event_and_input_channels_is_survived() {
    let (mut server, net, addr) = setup();
    let mut honest = HeadlessClient::new(net.endpoint(), addr);
    let mut rogue = rogue_in_game(&net, addr, &mut server);
    assert_eq!(server.players().count(), 1);
    for i in 0..3_000usize {
        let junk = noise(1 + i % 700, i as u64 + 1);
        let ch = if i % 2 == 0 {
            Channel::Event
        } else {
            Channel::Input
        };
        let _ = rogue.send(ch, &junk);
        rogue.update(Duration::from_secs_f32(DT)).unwrap();
        honest.step(Some((0, 100, 0)));
        server.tick();
    }
    for _ in 0..120 {
        honest.step(Some((0, 100, 0)));
        server.tick();
    }
    assert!(honest.in_game());
    assert!(honest.snapshots.len() > 100, "honest client starved");
    assert!(server.players().any(|p| Some(p) == honest.player));
}

#[test]
fn flooding_inputs_does_not_starve_another_client_or_speed_the_flooder_up() {
    let (mut server, net, addr) = setup();
    let mut honest = HeadlessClient::new(net.endpoint(), addr);
    let mut flooder = rogue_in_game(&net, addr, &mut server);
    for _ in 0..30 {
        honest.step(None);
        server.tick();
    }
    assert_eq!(server.players().count(), 2);
    // 40 forward commands per tick, stamped over the buffered horizon.
    for _ in 0..600u32 {
        let now = server.world().tick().0;
        for k in 0..40u32 {
            let c = InputCommand {
                move_y: 127,
                ..command(now + k)
            };
            let p = InputPacket {
                command: c,
                ack: None,
            };
            let _ = flooder.send(Channel::Input, &p.encode());
        }
        flooder.update(Duration::from_secs_f32(DT)).unwrap();
        honest.step(Some((0, 100, 0)));
        server.tick();
    }
    let before = honest.snapshots.len();
    for _ in 0..120 {
        honest.step(Some((0, 100, 0)));
        server.tick();
    }
    assert!(
        honest.snapshots.len() >= before + 30,
        "honest client starved"
    );
    // One command is applied per tick whatever the flood: at most the
    // controller's run speed over the ~12 s elapsed (horizontally: the
    // flooder runs off the 50 m floor and falls).
    let id = server
        .players()
        .find(|p| Some(*p) != honest.player)
        .unwrap();
    let p = server.world().position(id).unwrap();
    let dist = p.x.hypot(p.z);
    let run = server.config().move_params.run_speed;
    assert!(dist < run * 12.5 + 2.0, "flooder travelled {dist} m");
}

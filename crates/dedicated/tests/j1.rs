//! Milestone J1: two headless clients join a dedicated server over real UDP
//! with 5 % packet loss in both directions, move, and are told when it stops.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dedicated::maps::Catalog;
use dedicated::{Dedicated, Settings};
use faset_core::{NetId, Tick, DT};
use faset_net::{Channel, ClientEndpoint, ConditionedTransport, NetworkConditions, UdpTransport};
use faset_protocol::{InputCommand, Message, PROTOCOL_VERSION};
use faset_server::{build_fingerprint, InputPacket, Snapshot, SnapshotDecoder};

type Link = ConditionedTransport<UdpTransport>;

fn lossy(seed: u64) -> NetworkConditions {
    NetworkConditions {
        loss_percent: 5.0,
        seed,
        ..NetworkConditions::default()
    }
}

struct Client {
    endpoint: ClientEndpoint<Link>,
    hello: bool,
    player: Option<NetId>,
    decoder: SnapshotDecoder,
    latest: Option<Snapshot>,
    first: Option<Snapshot>,
}

impl Client {
    fn new(addr: std::net::SocketAddr, seed: u64) -> Self {
        let udp = UdpTransport::bind("127.0.0.1:0").unwrap();
        Self {
            endpoint: ClientEndpoint::new(ConditionedTransport::new(udp, lossy(seed)), addr),
            hello: false,
            player: None,
            decoder: SnapshotDecoder::new(),
            latest: None,
            first: None,
        }
    }

    fn send(&mut self, channel: Channel, msg: &Message) {
        let _ = self.endpoint.send(channel, &msg.encode().unwrap());
    }

    fn step(&mut self, walk: bool) {
        if !self.hello {
            self.hello = true;
            self.send(
                Channel::Event,
                &Message::ClientHello {
                    protocol_version: PROTOCOL_VERSION,
                    build_fingerprint: build_fingerprint(),
                },
            );
        }
        if let (true, Some(latest)) = (walk, &self.latest) {
            let command = InputCommand {
                tick: Tick(latest.tick.0 + 6),
                buttons: 0,
                move_x: 0,
                move_y: 127,
                yaw: 0,
                pitch: 0,
                weapon_slot: 1,
            };
            let packet = InputPacket {
                command,
                ack: self.decoder.ack(),
            };
            let _ = self.endpoint.send(Channel::Input, &packet.encode());
        }
        self.endpoint.update(Duration::from_secs_f32(DT)).unwrap();
        while let Some(bytes) = self.endpoint.recv(Channel::Event) {
            if let Ok(Message::ServerHello { player_id, .. }) = Message::decode(&bytes) {
                self.player = Some(player_id);
                self.send(Channel::Event, &Message::Ready);
            }
        }
        while let Some(bytes) = self.endpoint.recv(Channel::Snapshot) {
            let msg = Message::decode(&bytes).unwrap();
            if let Ok(s) = self.decoder.decode(&msg) {
                if self.latest.as_ref().is_none_or(|l| s.tick > l.tick) {
                    self.first.get_or_insert_with(|| s.clone());
                    self.latest = Some(s);
                }
            }
        }
    }

    fn z(&self, s: &Option<Snapshot>) -> Option<f32> {
        Some(s.as_ref()?.entity(self.player?)?.position.z)
    }
}

#[test]
fn two_clients_move_under_packet_loss_then_server_stops() {
    let stop = Arc::new(AtomicBool::new(false));
    let (addr_tx, addr_rx) = mpsc::channel();
    let server_stop = Arc::clone(&stop);
    let server = std::thread::spawn(move || {
        let udp = UdpTransport::bind("127.0.0.1:0").unwrap();
        addr_tx.send(udp.local_addr().unwrap()).unwrap();
        let link = ConditionedTransport::new(udp, lossy(99));
        let mut d =
            Dedicated::new(Settings::default(), Catalog::default(), vec![link.into()]).unwrap();
        let (_tx, rx) = mpsc::channel();
        d.run_loop(&server_stop, &rx);
    });
    let addr = addr_rx.recv().unwrap();

    let mut clients = [Client::new(addr, 1), Client::new(addr, 2)];
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut walk_until = None;
    while Instant::now() < deadline {
        let walking = walk_until.is_some();
        for c in &mut clients {
            c.step(walking);
        }
        if walk_until.is_none() && clients.iter().all(|c| c.latest.is_some()) {
            walk_until = Some(Instant::now() + Duration::from_secs(2));
        }
        if walk_until.is_some_and(|t| Instant::now() > t) {
            break;
        }
        std::thread::sleep(Duration::from_millis(16));
    }
    assert!(walk_until.is_some(), "clients did not join");
    for c in &clients {
        let (a, b) = (c.z(&c.first).unwrap(), c.z(&c.latest).unwrap());
        assert!(a - b > 2.0, "player did not move forward: {a} -> {b}");
        // Both players are visible to each other.
        assert_eq!(c.latest.as_ref().unwrap().entities.len(), 2);
    }

    // `ServerEndpoint::disconnect` does not notify the peer: clients notice the
    // shutdown through their own timeout (about 9.8 s), so allow twice that
    // (lot 5.1: 10 s made this test fail under CI load).
    stop.store(true, Ordering::Relaxed);
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline && clients.iter().any(|c| c.endpoint.is_connected()) {
        for c in &mut clients {
            c.step(false);
        }
        std::thread::sleep(Duration::from_millis(16));
    }
    server.join().unwrap();
    assert!(
        clients.iter().all(|c| !c.endpoint.is_connected()),
        "clients were not notified of the shutdown"
    );
}

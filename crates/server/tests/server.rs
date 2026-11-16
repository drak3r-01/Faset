//! Lot 1.4 acceptance: one server, headless clients, one process, no window.

mod common;

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use common::HeadlessClient;
use faset_core::glam::Vec3;
use faset_core::{NetId, Tick};
use faset_net::{MemoryNetwork, UdpTransport};
use faset_protocol::quant::{quantize_yaw, POS_STEP};
use faset_protocol::PROTOCOL_VERSION;
use faset_server::{RejectReason, Server, ServerConfig, ServerNotice};

/// Server positions by tick, to compare snapshots against.
type History = BTreeMap<Tick, BTreeMap<NetId, Vec3>>;

fn record(server: &Server, history: &mut History) {
    let positions = server
        .players()
        .filter_map(|id| Some((id, server.world().position(id)?)))
        .collect();
    history.insert(server.world().tick(), positions);
}

#[test]
fn two_clients_see_each_other_move() {
    let net = MemoryNetwork::new();
    let server_t = net.endpoint();
    let server_addr = server_t.addr();
    // Second client over real UDP: the server listens on both at once.
    let udp = UdpTransport::bind("127.0.0.1:0").unwrap();
    let udp_addr = udp.local_addr().unwrap();
    let mut server =
        Server::new(ServerConfig::default(), vec![server_t.into(), udp.into()]).unwrap();

    let mut a = HeadlessClient::new(net.endpoint(), server_addr);
    let mut b = HeadlessClient::new(UdpTransport::bind("127.0.0.1:0").unwrap(), udp_addr);
    let mut history = History::new();

    // Handshake: both clients reach the game.
    for _ in 0..300 {
        a.step(None);
        b.step(None);
        server.tick();
        record(&server, &mut history);
        if a.in_game() && b.in_game() {
            break;
        }
        // UDP loopback is asynchronous: give the kernel a moment.
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(a.in_game() && b.in_game(), "handshake did not complete");
    let (ida, idb) = (a.player.unwrap(), b.player.unwrap());
    assert_ne!(ida, idb);
    assert_eq!(a.map_fingerprint, Some(server.config().map.fingerprint));
    let mut joined = Vec::new();
    while let Some(n) = server.poll_notice() {
        joined.push(n);
    }
    assert_eq!(joined.len(), 2, "{joined:?}");
    assert!(joined.iter().all(|n| matches!(n, ServerNotice::Joined(_))));

    let start_a = server.world().position(ida).unwrap();
    let start_b = server.world().position(idb).unwrap();
    let both_in_game = server.world().tick();

    // A walks forward (-Z), B strafes right (+X), for one second.
    let forward = Some((0, 127, quantize_yaw(0.0)));
    let strafe = Some((127, 0, quantize_yaw(0.0)));
    for _ in 0..60 {
        a.step(forward);
        b.step(strafe);
        server.tick();
        record(&server, &mut history);
        std::thread::sleep(Duration::from_millis(1));
    }
    // Let the last snapshots arrive.
    for _ in 0..10 {
        a.step(None);
        b.step(None);
        server.tick();
        record(&server, &mut history);
        std::thread::sleep(Duration::from_millis(1));
    }

    let end_a = server.world().position(ida).unwrap();
    let end_b = server.world().position(idb).unwrap();
    assert!(
        start_a.z - end_a.z > 3.0,
        "A did not move: {start_a} -> {end_a}"
    );
    assert!(
        end_b.x - start_b.x > 3.0,
        "B did not move: {start_b} -> {end_b}"
    );

    for (name, snapshots) in [("A", &a.snapshots), ("B", &b.snapshots)] {
        let mut checked = 0;
        for snap in snapshots {
            let truth = &history[&snap.tick];
            let spawns = &server.config().map.spawn_points;
            if snap.tick >= both_in_game {
                assert_eq!(snap.entities.len(), 2, "{name} tick {:?}", snap.tick);
            }
            for e in &snap.entities {
                // A player spawned during this tick is not in the history yet.
                let want = truth
                    .get(&e.id)
                    .copied()
                    .unwrap_or_else(|| spawns[usize::from(e.id.0) % spawns.len()]);
                let err = (e.position - want).abs().max_element();
                assert!(err <= POS_STEP, "{name}: entity {:?} off by {err} m", e.id);
                checked += 1;
            }
            if let Some(ack) = snap.last_command {
                assert!(ack < snap.tick, "ack {ack:?} not before {:?}", snap.tick);
            }
        }
        let latest = snapshots.last().unwrap();
        assert!(latest.entity(ida).is_some() && latest.entity(idb).is_some());
        assert!(
            latest.last_command.is_some(),
            "{name}: no command acknowledged"
        );
        assert!(checked > 40, "{name}: only {checked} entity states checked");
    }
    // Each client sees the other one move.
    let seen_a = b.latest().unwrap().entity(ida).unwrap().position;
    let seen_b = a.latest().unwrap().entity(idb).unwrap().position;
    assert!(start_a.z - seen_a.z > 3.0, "B did not see A move");
    assert!(seen_b.x - start_b.x > 3.0, "A did not see B move");
}

#[test]
fn snapshot_rate_follows_config() {
    for rate in [20u8, 30, 60] {
        let net = MemoryNetwork::new();
        let server_t = net.endpoint();
        let addr = server_t.addr();
        let config = ServerConfig {
            snapshot_rate: rate,
            ..ServerConfig::default()
        };
        let mut server = Server::new(config, vec![server_t.into()]).unwrap();
        let mut c = HeadlessClient::new(net.endpoint(), addr);
        while !c.in_game() {
            c.step(None);
            server.tick();
        }
        let before = c.snapshots.len();
        for _ in 0..120 {
            c.step(None);
            server.tick();
        }
        let got = c.snapshots.len() - before;
        let want = 2 * usize::from(rate);
        assert!(
            got.abs_diff(want) <= 1,
            "rate {rate}: {got} snapshots in 2 s"
        );
    }
}

#[test]
fn version_mismatch_is_rejected() {
    let net = MemoryNetwork::new();
    let server_t = net.endpoint();
    let addr = server_t.addr();
    let mut server = Server::new(ServerConfig::default(), vec![server_t.into()]).unwrap();
    let mut bad = HeadlessClient::with_version(net.endpoint(), addr, PROTOCOL_VERSION + 1);
    let mut good = HeadlessClient::new(net.endpoint(), addr);
    for _ in 0..60 {
        bad.step(None);
        good.step(None);
        server.tick();
    }
    assert_eq!(
        server.poll_notice(),
        Some(ServerNotice::Rejected(RejectReason::ProtocolVersion {
            client: PROTOCOL_VERSION + 1
        }))
    );
    assert!(bad.player.is_none(), "rejected client got a ServerHello");
    assert!(!bad.in_game());
    assert!(good.in_game());
    assert_eq!(server.players().count(), 1);
    // The rejected client keeps sending keep-alives, so the endpoint may see
    // it again as a new, unauthenticated connection; that one never gets a
    // player and is dropped by the handshake timeout.
    assert!(server.pending_clients() <= 1);
    let rest: Vec<_> = std::iter::from_fn(|| server.poll_notice()).collect();
    assert_eq!(rest.len(), 1, "{rest:?}");
    assert!(matches!(rest[0], ServerNotice::Joined(_)));
}

#[test]
fn missing_commands_repeat_the_last_one() {
    let net = MemoryNetwork::new();
    let server_t = net.endpoint();
    let addr = server_t.addr();
    let mut server = Server::new(ServerConfig::default(), vec![server_t.into()]).unwrap();
    let mut c = HeadlessClient::new(net.endpoint(), addr);
    while !c.in_game() {
        c.step(None);
        server.tick();
    }
    let id = c.player.unwrap();
    // One command, then silence: the player keeps walking.
    c.step(Some((0, 127, 0)));
    for _ in 0..61 {
        server.tick();
        c.step(None);
    }
    let moved = server.config().map.spawn_points[usize::from(id.0) % 8].z
        - server.world().position(id).unwrap().z;
    assert!(moved > 4.5, "moved only {moved} m");
    // The acknowledged command stays the one actually received.
    let ack = c.latest().unwrap().last_command.unwrap();
    assert!(ack.0 < c.latest().unwrap().tick.0 - 50);
}

#[test]
fn tick_rate_is_sixty_hertz() {
    let mut server = Server::new(ServerConfig::default(), Vec::new()).unwrap();
    let start = Instant::now();
    let run = Duration::from_secs(2);
    server.run_until(|s| s.world().tick().0 >= 120 || start.elapsed() > run * 2);
    let elapsed = start.elapsed().as_secs_f64();
    let ticks = f64::from(server.world().tick().0);
    // The first tick is due at once, so 120 ticks take 119 periods.
    let rate = (ticks - 1.0) / elapsed;
    assert!(
        (rate - 60.0).abs() < 1.5,
        "measured {rate:.2} Hz over {elapsed:.3} s"
    );
}

#[test]
fn silent_client_times_out_in_handshake() {
    let net = MemoryNetwork::new();
    let server_t = net.endpoint();
    let addr = server_t.addr();
    let mut config = ServerConfig::default();
    config.limits.handshake_timeout_ticks = 30;
    let mut server = Server::new(config, vec![server_t.into()]).unwrap();
    // Connects (keep-alive on first update) but never sends ClientHello.
    let mut silent = faset_net::ClientEndpoint::new(net.endpoint(), addr);
    for _ in 0..40 {
        silent.update(Duration::from_millis(16)).unwrap();
        server.tick();
    }
    assert_eq!(
        server.poll_notice(),
        Some(ServerNotice::Rejected(RejectReason::Timeout))
    );
    assert_eq!(server.pending_clients(), 0);
}

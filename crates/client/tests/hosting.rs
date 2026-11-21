//! Lot 1.9 acceptance: integrated server, LAN opening and discovery.
//!
//! Broadcast is not routable inside the dev container, so discovery requests
//! are injected to the loopback address (`ServerBrowser::with_targets`); the
//! broadcast address list itself is only built by `ServerBrowser::new`.

#[path = "../../server/tests/common/mod.rs"]
mod headless;

use std::net::{Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};

use faset_client::{host_game, HostConfig, ServerBrowser, ENTRY_TTL};
use faset_net::UdpTransport;
use faset_server::ServerConfig;
use headless::HeadlessClient;

fn lan_config(name: &str) -> HostConfig {
    let mut c = HostConfig::new(ServerConfig::default());
    c.name = name.into();
    c.map_name = "test-map".into();
    c.game_port = 0;
    // A free discovery port per test, found by binding then releasing.
    let probe = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    c.discovery_port = probe.local_addr().unwrap().port();
    c
}

fn loopback(port: u16) -> SocketAddr {
    (Ipv4Addr::LOCALHOST, port).into()
}

/// Polls the browser in real time until `done` or `limit` elapses.
fn pump(browser: &mut ServerBrowser, limit: Duration, done: impl Fn(&ServerBrowser) -> bool) {
    let start = Instant::now();
    let mut last = start;
    while start.elapsed() < limit && !done(browser) {
        std::thread::sleep(Duration::from_millis(5));
        let now = Instant::now();
        browser.update(now - last);
        last = now;
    }
}

#[test]
fn host_is_found_within_two_seconds_and_a_second_client_joins() {
    let cfg = lan_config("Test game");
    let discovery_port = cfg.discovery_port;
    let host = host_game(cfg, true).unwrap();
    let lan = host.lan().unwrap();
    assert!(lan.discoverable);
    assert_eq!(host.udp_sockets(), 2);

    // The host is an ordinary client over the memory transport.
    let (mem, server) = host.connect_local();
    let mut local = HeadlessClient::new(mem, server);

    let mut browser = ServerBrowser::with_targets(vec![loopback(discovery_port)]).unwrap();
    pump(&mut browser, Duration::from_secs(2), |b| {
        !b.entries().is_empty()
    });
    let entry = browser.entries().first().expect("not found within 2 s");
    assert_eq!(entry.info.name, "Test game");
    assert_eq!(entry.info.map, "test-map");
    assert_eq!(entry.info.mode, 0);
    assert_eq!(entry.info.game_port, lan.game_addr.port());
    assert_eq!(
        usize::from(entry.info.max_players),
        ServerConfig::default().limits.max_players
    );
    assert_eq!(entry.addr, loopback(lan.game_addr.port()));

    // The second client joins the discovered address over UDP.
    let mut remote = HeadlessClient::new(UdpTransport::bind("127.0.0.1:0").unwrap(), entry.addr);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !(local.in_game() && remote.in_game()) && Instant::now() < deadline {
        local.step(None);
        remote.step(None);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(local.in_game() && remote.in_game(), "join failed");
    assert_ne!(local.player, remote.player);

    // The announced player count follows the game.
    pump(&mut browser, Duration::from_secs(3), |b| {
        b.entries().first().is_some_and(|e| e.info.players == 2)
    });
    assert_eq!(browser.entries()[0].info.players, 2);
    host.stop();
}

#[test]
fn entry_disappears_six_seconds_after_the_server_stops() {
    let cfg = lan_config("Short lived");
    let port = cfg.discovery_port;
    let host = host_game(cfg, true).unwrap();
    let mut browser = ServerBrowser::with_targets(vec![loopback(port)]).unwrap();
    pump(&mut browser, Duration::from_secs(2), |b| {
        !b.entries().is_empty()
    });
    assert_eq!(browser.entries().len(), 1);

    host.stop();
    // Virtual time: still listed just before the limit (the browser keeps
    // asking, nobody answers), gone at six seconds.
    browser.update(Duration::from_millis(5500));
    assert_eq!(browser.entries().len(), 1);
    browser.update(ENTRY_TTL - Duration::from_millis(5500));
    assert!(browser.entries().is_empty());
}

#[test]
fn opening_to_the_lan_later_keeps_the_server_running() {
    let cfg = lan_config("Late open");
    let port = cfg.discovery_port;
    let mut host = host_game(cfg, false).unwrap();
    assert!(host.lan().is_none());
    let (mem, server) = host.connect_local();
    let mut local = HeadlessClient::new(mem, server);
    let mut browser = ServerBrowser::with_targets(vec![loopback(port)]).unwrap();
    pump(&mut browser, Duration::from_millis(300), |_| false);
    assert!(browser.entries().is_empty());

    for _ in 0..200 {
        local.step(None);
        if local.in_game() {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(local.in_game());
    let tick_before = local.latest().unwrap().tick;

    host.open_to_lan().unwrap();
    pump(&mut browser, Duration::from_secs(3), |b| {
        !b.entries().is_empty()
    });
    assert_eq!(browser.entries()[0].info.players, 1);
    // Same server, same local client: it kept ticking.
    for _ in 0..30 {
        local.step(None);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(local.latest().unwrap().tick > tick_before);
}

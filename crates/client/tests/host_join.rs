//! Lot 6.2 acceptance: hosting with bots, joining through the LAN list on
//! loopback, favourites and history.
//!
//! Broadcast is not routable inside the dev container, so the join screen's
//! browser is aimed at the loopback address (`ServerBrowser::with_targets`).
//! The password is not covered: the handshake carries none (see the lot
//! report).

use std::net::{Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};

use faset_client::{host_config, host_game, App, AppState, MapCatalogue, ServerBrowser};
use faset_minigame_api::Registry;
use faset_mode_fps::register_modes;
use faset_mode_race::RaceMode;
use faset_protocol::InputCommand;
use faset_ui::{PlayRequest, UiCommand};

fn registry() -> Registry {
    let mut r = Registry::new();
    register_modes(&mut r).unwrap();
    r.register::<RaceMode>().unwrap();
    r
}

fn catalogue() -> MapCatalogue {
    MapCatalogue::scan(&std::env::temp_dir().join("faset-no-maps"))
}

fn idle(tick: faset_core::Tick) -> InputCommand {
    InputCommand {
        tick,
        buttons: 0,
        move_x: 0,
        move_y: 0,
        yaw: 0,
        pitch: 0,
        weapon_slot: 1,
    }
}

fn pump(app: &mut App, what: &str, done: impl Fn(&App) -> bool) {
    let start = Instant::now();
    while !done(app) {
        assert!(start.elapsed() < Duration::from_secs(20), "timeout: {what}");
        app.update(Duration::from_millis(8), &mut idle);
        std::thread::sleep(Duration::from_millis(4));
    }
}

fn free_udp_port() -> u16 {
    let probe = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    probe.local_addr().unwrap().port()
}

#[test]
fn a_host_with_four_bots_is_joined_through_the_lan_list() {
    let registry = registry();
    let catalogue = catalogue();
    let mut request = PlayRequest {
        mode: "fps.ffa".to_owned(),
        bots: 4,
        max_players: 16,
        lan: true,
        ..PlayRequest::default()
    };
    request.settings = Default::default();
    let names = catalogue.names();
    let mut config = host_config(&request, &registry, catalogue.gameplay(), &names, 7).unwrap();
    assert_eq!(config.bots.len(), 4);
    assert_eq!(config.server.limits.max_players, 16);
    config.game_port = 0;
    config.discovery_port = free_udp_port();
    let discovery_port = config.discovery_port;
    let host = host_game(config, true).unwrap();
    assert!(host.lan().unwrap().discoverable);

    let start = Instant::now();
    while host.player_count() < 4 {
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "bots never joined"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    let mut app = App::new(registry, catalogue);
    let target: SocketAddr = (Ipv4Addr::LOCALHOST, discovery_port).into();
    app.update(Duration::from_millis(8), &mut idle);
    app.handle(vec![UiCommand::Push(AppState::ServerBrowser)]);
    // The join screen opened its own broadcast browser; aim it at loopback.
    app.set_lan_browser(Some(ServerBrowser::with_targets(vec![target]).unwrap()));
    pump(&mut app, "LAN entry", |a| !a.model().lan_servers.is_empty());
    let row = app.model().lan_servers[0].clone();
    assert_eq!(row.map, "example");
    assert!(row.players.0 >= 4 && row.players.1 == 16, "{row:?}");

    app.handle(vec![UiCommand::Connect(row.address.clone())]);
    assert_eq!(app.stack().top(), AppState::Connecting);
    pump(&mut app, "InGame", |a| a.stack().top() == AppState::InGame);
    pump(&mut app, "snapshot with the bots", |a| {
        a.session().is_some_and(|s| s.entity_count() >= 5)
    });
    assert_eq!(app.favorites().history, [row.address]);
}

#[test]
fn favourites_and_history_survive_a_restart() {
    let dir = std::env::temp_dir().join(format!("faset-host-join-{}", std::process::id()));
    let path = dir.join("favorites.ron");
    let mut app = App::new(registry(), catalogue());
    app.use_favorites_file(path.clone());
    app.handle(vec![
        UiCommand::ToggleFavourite("192.168.1.9:7777".into()),
        UiCommand::ToggleFavourite("10.0.0.1:7777".into()),
        UiCommand::ToggleFavourite("10.0.0.1:7777".into()),
        UiCommand::Connect("127.0.0.1:9".into()),
    ]);
    drop(app);

    let mut again = App::new(registry(), catalogue());
    again.use_favorites_file(path);
    assert_eq!(again.model().favourites, ["192.168.1.9:7777"]);
    assert_eq!(again.model().history, ["127.0.0.1:9"]);
    std::fs::remove_dir_all(&dir).unwrap();
}

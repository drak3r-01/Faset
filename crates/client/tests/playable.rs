//! Lot 6.6 acceptance: a match is playable through the application layer.
//!
//! A solo game with bots is entered through the play screen, driven with the
//! player's keys (`GameInput`), and the HUD model, the other players and the
//! shots come out of the live session. A second client joins a hosted
//! rotation over UDP with none of its maps and downloads the one in play.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use faset_client::{host_config, host_game, App, AppState, GameInput, LocalFrame, MapCatalogue};
use faset_core::bindings::{Bindings, Input};
use faset_core::settings::ControlSettings;
use faset_minigame_api::{Phase, Registry};
use faset_mode_fps::register_modes;
use faset_ui::{PlayRequest, UiCommand};

const YARD: &str = r#"Map(
    format_version: 1,
    meta: (name: "Yard", author: "Faset", description: "Lot 6.6 test yard.", modes: ["fps"], players: (2, 12)),
    theme: (ambience: "noon", sky: "clear", weather: [], palette: "industrial"),
    brushes: [
        Box(min: (-60, -2, -60), max: (60, 0, 60), material: "concrete"),
        Box(min: (-6, 0, -1), max: (6, 4, 1), material: "concrete"),
    ],
    spawns: [
        (at: (-12.0, 0.0, -8.0), yaw: 0.0, team: A),
        (at: (12.0, 0.0, 12.0), yaw: 0.0, team: B),
    ],
    pickups: [],
    zones: [],
)
"#;

fn registry() -> Registry {
    let mut r = Registry::new();
    register_modes(&mut r).unwrap();
    r
}

fn empty_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("faset-playable-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Runs the application for real time until `done`.
fn pump(app: &mut App, game: &mut GameInput, what: &str, limit: u64, done: impl Fn(&App) -> bool) {
    let start = Instant::now();
    while !done(app) {
        assert!(
            start.elapsed() < Duration::from_secs(limit),
            "timeout: {what}"
        );
        step(app, game);
    }
}

thread_local! {
    /// When this thread last ran a frame: the application is given the real
    /// time that passed, as the window does, so its clock keeps pace with
    /// the server's.
    static LAST_FRAME: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
}

/// One application frame with the player's keys, about 8 ms after the
/// previous one.
fn step(app: &mut App, game: &mut GameInput) {
    let (bindings, controls) = (Bindings::default(), ControlSettings::default());
    std::thread::sleep(Duration::from_millis(8));
    let now = Instant::now();
    let dt = LAST_FRAME
        .get()
        .map_or(Duration::from_millis(8), |t| now - t);
    LAST_FRAME.set(Some(now));
    app.update(dt, &mut |tick| game.command(tick, &bindings, &controls));
}

fn start_solo(app: &mut App) {
    let request = PlayRequest {
        mode: "fps.ffa".to_owned(),
        bots: 3,
        max_players: 8,
        ..PlayRequest::default()
    };
    app.update(Duration::from_millis(8), &mut |t| {
        GameInput::new().command(t, &Bindings::default(), &ControlSettings::default())
    });
    app.handle(vec![UiCommand::StartMatch(request)]);
}

#[test]
fn a_solo_match_is_played_with_the_players_keys() {
    let dir = empty_dir("solo");
    let mut app = App::new(registry(), MapCatalogue::scan(&dir));
    let mut game = GameInput::new();
    start_solo(&mut app);
    pump(&mut app, &mut game, "InGame", 30, |a| {
        a.stack().top() == AppState::InGame
    });
    assert!(app.take_map().is_some(), "the match's map is shown");

    // The bots are in the replicated state and the HUD model has the
    // weapon in hand with its ammunition.
    pump(&mut app, &mut game, "the bots in the state", 30, |a| {
        a.session().is_some_and(|s| s.entity_count() >= 4)
    });
    let frame = LocalFrame::default();
    let start = Instant::now();
    let model = loop {
        step(&mut app, &mut game);
        let view = app.session_mut().unwrap().view_mut();
        if let Some(m) = view.hud_model(&frame, 0.016).filter(|m| m.ammo.is_some()) {
            break m;
        }
        assert!(start.elapsed() < Duration::from_secs(30), "no HUD model");
    };
    let (mag, reserve) = model.ammo.expect("the weapon in hand has ammunition");
    assert!(mag > 0 && reserve > 0);
    assert!(!model.weapon.is_empty());
    assert!(model.alive);
    let session = app.session_mut().unwrap();
    let players = session.view_mut().players();
    assert!(players.len() >= 4, "{players:?}");

    // Hold forward: the player moves (the countdown may freeze them first).
    let (bindings, controls) = (Bindings::default(), ControlSettings::default());
    let before = app.session().unwrap().display_position().unwrap();
    game.press(Input::key("KeyW"), true, &bindings, &controls);
    pump(
        &mut app,
        &mut game,
        "the match to allow movement",
        60,
        |a| {
            a.session().is_some_and(|s| {
                s.view().movement_allowed()
                    && s.display_position()
                        .is_some_and(|p| p.distance(before) > 2.0)
            })
        },
    );
    game.press(Input::key("KeyW"), false, &bindings, &controls);

    // Fire during play: the magazine goes down and the view sees the shots.
    pump(&mut app, &mut game, "Playing", 60, |a| {
        a.session()
            .is_some_and(|s| s.view().phase() == Some(Phase::Playing))
    });
    game.press(Input::Mouse(0), true, &bindings, &controls);
    let mut shots = 0;
    let start = Instant::now();
    while shots == 0 {
        step(&mut app, &mut game);
        let view = app.session_mut().unwrap().view_mut();
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "no shot seen: phase {:?}, player {:?}, all {:?}",
            view.phase(),
            view.local_player(),
            view.players()
        );
        let _ = view.hud_model(&frame, 0.016);
        shots += view.take_shots();
    }
    game.press(Input::Mouse(0), false, &bindings, &controls);
}

#[test]
fn a_remote_client_downloads_the_map_in_play() {
    // The host plays the yard from a rotation; the client has no maps.
    let host_dir = empty_dir("host");
    std::fs::write(host_dir.join("yard.ron"), YARD).unwrap();
    let catalogue = MapCatalogue::scan(&host_dir);
    let names = catalogue.names();
    let yard = names.iter().position(|n| n == "yard").unwrap();
    let request = PlayRequest {
        mode: "fps.ffa".to_owned(),
        map: yard,
        rotation: true,
        bots: 2,
        max_players: 8,
        lan: true,
        ..PlayRequest::default()
    };
    let mut config = host_config(&request, &registry(), catalogue.gameplay(), &names, 3).unwrap();
    config.game_port = 0;
    config.discovery_port = {
        let probe = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        probe.local_addr().unwrap().port()
    };
    let host = host_game(config, true).unwrap();
    let addr: SocketAddr = host.lan().unwrap().game_addr;
    let addr: SocketAddr = (std::net::Ipv4Addr::LOCALHOST, addr.port()).into();

    let mut app = App::new(registry(), MapCatalogue::scan(&empty_dir("client")));
    let mut game = GameInput::new();
    app.update(Duration::from_millis(8), &mut |t| {
        GameInput::new().command(t, &Bindings::default(), &ControlSettings::default())
    });
    app.handle(vec![UiCommand::Connect(addr.to_string())]);
    pump(&mut app, &mut game, "InGame over UDP", 30, |a| {
        a.stack().top() == AppState::InGame
    });
    let map = app
        .take_map()
        .expect("the downloaded map is handed to the renderer");
    assert!(
        !map.chunks.is_empty(),
        "the client shows the server's map, not nothing"
    );
    pump(&mut app, &mut game, "the match state", 30, |a| {
        a.session().is_some_and(|s| s.view().phase().is_some())
    });
    drop(host);
}

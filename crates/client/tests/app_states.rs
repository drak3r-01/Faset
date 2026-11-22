//! The application state machine (lot 6.1): every state is reachable and
//! left, states pushed over the match never destroy it, and the menus list
//! exactly the registered modes.

use std::time::{Duration, Instant};

use faset_client::{App, AppState, MapCatalogue, Transition};
use faset_minigame_api::Registry;
use faset_mode_fps::FpsMode;
use faset_mode_race::RaceMode;
use faset_protocol::InputCommand;
use faset_ui::{MenuModel, PlayRequest, UiCommand};

fn registry() -> Registry {
    let mut r = Registry::new();
    r.register::<FpsMode>().unwrap();
    r.register::<RaceMode>().unwrap();
    r
}

fn app() -> App {
    let dir = std::env::temp_dir().join("faset-no-maps");
    App::new(registry(), MapCatalogue::scan(&dir))
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

fn step(app: &mut App) {
    app.update(Duration::from_millis(8), &mut idle);
}

/// Updates until `done` holds (at most 20 s of real time).
fn run_until(app: &mut App, what: &str, done: impl Fn(&App) -> bool) {
    let start = Instant::now();
    while !done(app) {
        assert!(start.elapsed() < Duration::from_secs(20), "timeout: {what}");
        step(app);
        std::thread::sleep(Duration::from_millis(4));
    }
}

fn start_request(app: &App) -> PlayRequest {
    let mut request = app.model().form.clone();
    request.map = 0;
    request
}

#[test]
fn every_state_is_driven_and_the_machine_returns_to_main_menu() {
    let mut app = app();
    let mut seen = vec![app.stack().top()];
    assert_eq!(seen[0], AppState::Boot);
    step(&mut app);
    assert_eq!(app.stack().top(), AppState::MainMenu);
    seen.push(AppState::MainMenu);

    // Screens reached from the menu and left with Back.
    for state in [
        AppState::ServerBrowser,
        AppState::HostSetup,
        AppState::Loadout,
        AppState::Settings,
        AppState::Editor,
    ] {
        app.handle(vec![UiCommand::Push(state)]);
        assert_eq!(app.stack().top(), state);
        seen.push(state);
        app.handle(vec![UiCommand::Pop]);
        assert_eq!(app.stack().top(), AppState::MainMenu);
    }

    // Connecting: a remote address nobody listens on, cancelled.
    app.handle(vec![
        UiCommand::Push(AppState::ServerBrowser),
        UiCommand::Connect("127.0.0.1:9".to_owned()),
    ]);
    assert_eq!(app.stack().top(), AppState::Connecting);
    seen.push(AppState::Connecting);
    assert!(app.session().is_some());
    app.handle(vec![UiCommand::Pop]);
    assert!(app.session().is_none(), "cancelling drops the session");
    app.handle(vec![UiCommand::Pop]);
    assert_eq!(app.stack().top(), AppState::MainMenu);

    // A solo match: Loading, then InGame, pause on top, back to the menu.
    app.handle(vec![UiCommand::StartMatch(start_request(&app))]);
    assert_eq!(app.stack().top(), AppState::Loading);
    seen.push(AppState::Loading);
    run_until(&mut app, "InGame", |a| a.stack().top() == AppState::InGame);
    seen.push(AppState::InGame);
    assert_eq!(app.stack().states(), [AppState::InGame]);
    app.pause().unwrap();
    assert_eq!(app.stack().top(), AppState::Pause);
    seen.push(AppState::Pause);
    app.handle(vec![UiCommand::LeaveMatch]);
    assert_eq!(app.stack().states(), [AppState::MainMenu]);
    assert!(app.session().is_none());

    for state in AppState::ALL {
        assert!(seen.contains(&state), "{state:?} was never driven");
    }
}

#[test]
fn settings_over_the_match_keep_the_same_world() {
    let mut app = app();
    step(&mut app);
    app.handle(vec![UiCommand::StartMatch(start_request(&app))]);
    run_until(&mut app, "InGame", |a| a.stack().top() == AppState::InGame);
    run_until(&mut app, "first entities", |a| {
        a.session().is_some_and(|s| s.entity_count() > 0)
    });
    let (id, entities) = {
        let s = app.session().unwrap();
        (s.instance_id(), s.entity_count())
    };
    app.stack_mut().drain_events();

    app.pause().unwrap();
    app.handle(vec![UiCommand::Push(AppState::Settings)]);
    assert_eq!(
        app.stack().states(),
        [AppState::InGame, AppState::Pause, AppState::Settings]
    );
    // The match keeps running under the menus.
    for _ in 0..50 {
        step(&mut app);
        std::thread::sleep(Duration::from_millis(4));
    }
    app.handle(vec![UiCommand::Pop, UiCommand::Pop]);
    assert_eq!(app.stack().states(), [AppState::InGame]);
    let s = app.session().expect("the session survived");
    assert_eq!(s.instance_id(), id, "same world instance");
    assert_eq!(s.entity_count(), entities, "same entity count");
    assert!(s.in_game());
}

#[test]
fn leaving_and_starting_again_gives_a_new_world() {
    let mut app = app();
    step(&mut app);
    app.handle(vec![UiCommand::StartMatch(start_request(&app))]);
    run_until(&mut app, "InGame", |a| a.stack().top() == AppState::InGame);
    let first = app.session().unwrap().instance_id();
    app.handle(vec![UiCommand::LeaveMatch]);
    app.handle(vec![UiCommand::StartMatch(start_request(&app))]);
    run_until(&mut app, "InGame", |a| a.stack().top() == AppState::InGame);
    assert_ne!(app.session().unwrap().instance_id(), first);
}

#[test]
fn quit_command_is_reported() {
    let mut app = app();
    step(&mut app);
    assert!(!app.quit_requested());
    app.handle(vec![UiCommand::Quit]);
    assert!(app.quit_requested());
}

#[test]
fn transitions_are_reported_to_the_stack_owner() {
    let mut stack = faset_client::StateStack::new(AppState::InGame);
    stack.push(AppState::Pause).unwrap();
    stack.push(AppState::Settings).unwrap();
    stack.pop().unwrap();
    stack.pop().unwrap();
    assert_eq!(
        stack.drain_events(),
        [
            Transition::Pushed(AppState::Pause),
            Transition::Pushed(AppState::Settings),
            Transition::Popped(AppState::Settings),
            Transition::Popped(AppState::Pause),
        ]
    );
}

#[test]
fn menu_lists_every_registered_mode_and_only_those() {
    let reg = registry();
    let model = MenuModel::new(&reg, vec!["example".into()]);
    let listed: Vec<&str> = model.modes.iter().map(|m| m.id).collect();
    let registered: Vec<&str> = reg.iter().map(|e| e.descriptor.id).collect();
    assert_eq!(listed, registered);
    assert_eq!(listed.len(), 2);
    assert_eq!(model.form.mode, listed[0]);

    let mut only_race = Registry::new();
    only_race.register::<RaceMode>().unwrap();
    let model = MenuModel::new(&only_race, vec![]);
    assert_eq!(model.modes.len(), 1);
    assert_eq!(model.modes[0].id, "race");
    assert!(MenuModel::new(&Registry::new(), vec![]).modes.is_empty());
}

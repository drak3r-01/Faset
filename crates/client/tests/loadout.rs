//! Lot 6.3 through the application: the loadout screen's command is checked
//! and saved to `profile.ron`, goes to a running match, and a crafted loadout
//! the server refuses comes back as a replacement.

use std::time::{Duration, Instant};

use faset_client::{App, AppState, MapCatalogue, Profile};
use faset_minigame_api::Registry;
use faset_mode_fps::class::Catalog;
use faset_mode_fps::{register_modes, Loadout};
use faset_protocol::InputCommand;
use faset_ui::UiCommand;

fn app() -> App {
    let mut registry = Registry::new();
    register_modes(&mut registry).unwrap();
    let dir = std::env::temp_dir().join("faset-no-maps");
    App::new(registry, MapCatalogue::scan(&dir))
}

fn idle(tick: faset_core::Tick) -> InputCommand {
    InputCommand {
        tick,
        buttons: 0,
        move_x: 0,
        move_y: 0,
        yaw: 0,
        pitch: 0,
        weapon_slot: 0,
    }
}

fn run_until(app: &mut App, what: &str, done: impl Fn(&mut App) -> bool) {
    let start = Instant::now();
    while !done(app) {
        assert!(start.elapsed() < Duration::from_secs(20), "timeout: {what}");
        app.update(Duration::from_millis(8), &mut idle);
        std::thread::sleep(Duration::from_millis(4));
    }
}

#[test]
fn saved_loadouts_are_checked_and_persisted() {
    let dir = std::env::temp_dir().join(format!("faset-loadout-app-{}", std::process::id()));
    let path = dir.join("profile.ron");
    let mut app = app();
    app.use_profile_file(path.clone());
    let mut selection = app.model().loadout.selection.clone();
    assert_eq!(selection.class, "trooper");
    assert!(app.model().loadout.is_ready());

    // An over-budget loadout is refused and nothing is written.
    selection.class = "prankster".into();
    selection.primary = "precision_rifle".into();
    selection.secondary = Some("sidearm".into());
    selection.melee = Some("baguette".into());
    selection.gadget = Some("toaster".into());
    assert!(app.model().loadout.choices.budget < 11);
    app.handle(vec![UiCommand::SaveLoadout(selection.clone())]);
    assert_eq!(app.profile().current().loadout, Loadout::trooper());
    assert!(app.take_status().unwrap().contains("budget"));
    assert!(!path.exists());

    // A valid one is kept, written, and read back by a new application.
    selection.primary = "smg".into();
    selection.body_colour = 4;
    app.handle(vec![UiCommand::SaveLoadout(selection)]);
    assert_eq!(app.profile().current().loadout.class, "prankster");
    assert!(path.exists());
    let mut again = self::app();
    again.use_profile_file(path.clone());
    assert_eq!(again.profile(), app.profile());
    assert_eq!(again.model().loadout.selection.primary, "smg");
    assert_eq!(again.model().loadout.selection.body_colour, 4);
    assert_eq!(Profile::load(&path).unwrap().current().appearance.body, 4);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_server_replaces_a_crafted_loadout_and_the_client_hears_it() {
    let mut app = app();
    app.handle(vec![UiCommand::StartMatch(app.model().form.clone())]);
    run_until(&mut app, "InGame", |a| a.stack().top() == AppState::InGame);
    // Over budget, built by hand to get past the client's own check.
    let l = Loadout {
        class: "prankster".into(),
        primary: "precision_rifle".into(),
        secondary: Some("sidearm".into()),
        melee: Some("baguette".into()),
        gadget: Some("toaster".into()),
    };
    assert!(l.budget_cost() > 10);
    let msg = l.to_msg(&Catalog::builtin()).unwrap();
    assert!(app.session_mut().unwrap().send_mode_message(msg.encode()));
    run_until(&mut app, "replacement", |a| {
        a.take_status()
            .is_some_and(|s| s.contains("refused your class"))
    });
    app.handle(vec![UiCommand::LeaveMatch]);
}

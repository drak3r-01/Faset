//! Acceptance tests of lot 5.4: one fixture per validation rule through the
//! editor panel, the click on an issue, autosave on a simulated clock,
//! export and import, and the immediate test with a headless client.

#[path = "../../server/tests/common/mod.rs"]
mod headless;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use faset_core::glam::{Vec2, Vec3};
use faset_editor::{
    CameraMode, Editor, FileError, FileService, Severity, TestSession, ValidationPanel,
    FOCUS_DISTANCE,
};
use faset_mapfmt::{
    fingerprint, parse, to_ron, Brush, MapFile, Pickup, PickupKind, Spawn, Team, Zone, ZoneKind,
};
use headless::HeadlessClient;

/// Spawn lines of one team: `n` spawns on the line x = `x`, 2 m apart.
fn spawns(team: &str, x: f32, n: usize) -> String {
    (0..n)
        .map(|i| {
            format!(
                "(at: ({x}, 0.0, {}), yaw: 0.0, team: {team}),",
                i as f32 * 2.0 - 7.0
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A small symmetric, valid arena for 16 players.
fn base_text() -> String {
    format!(
        r#"Map(
    format_version: 1,
    meta: (name: "Arena", modes: ["fps"], players: (2, 16)),
    theme: (ambience: "sunset", palette: "industrial"),
    brushes: [
        Box(min: (-60, -2, -40), max: (60, 0, 40), material: "concrete"),
        Box(min: (-4, 0, -12), max: (4, 8, -8), material: "metal"),
    ],
    spawns: [
{}
{}
    ],
    pickups: [
        (kind: Health, at: (0.0, 0.5, 0.0), respawn_s: 30),
        (kind: Armor, at: (0.0, 0.5, 6.0), respawn_s: 30),
    ],
    zones: [
        Box(min: (-60, -40, -40), max: (60, -20, 40), kind: KillVolume),
        Box(min: (-2, 0, -2), max: (2, 4, 2), kind: Objective(id: "flag")),
    ],
)
"#,
        spawns("A", -15.0, 8),
        spawns("B", 15.0, 8),
    )
}

fn base() -> MapFile {
    parse(base_text().as_bytes()).unwrap()
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("faset-editor-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn panel_for(map: &MapFile) -> ValidationPanel {
    let mut panel = ValidationPanel::new();
    panel.refresh(map);
    panel
}

fn issue_of(panel: &ValidationPanel, rule: u8) -> &faset_editor::ValidationIssue {
    panel
        .issues()
        .iter()
        .find(|i| i.rule == rule && i.severity == Severity::Error)
        .unwrap_or_else(|| panic!("no rule {rule} issue in {:?}", panel.issues()))
}

#[test]
fn the_sample_map_passes() {
    let panel = panel_for(&base());
    assert!(panel.is_ok(), "{:?}", panel.issues());
    assert!(panel.stats().triangles > 0);
    assert!(panel.stats().longest_line_of_sight_m > 20.0);
    let example = parse(include_bytes!("../../../assets/maps/example.ron")).unwrap();
    assert!(panel_for(&example).is_ok());
}

#[test]
fn rules_1_and_2_reject_bad_files() {
    let issues = faset_editor::validate_bytes(b"Map(format_version: 1, bogus: 3)");
    assert_eq!(issues[0].rule, 1);
    let mut text = base_text();
    text.push_str(&" ".repeat(faset_mapfmt::MAX_MAP_BYTES));
    assert_eq!(faset_editor::validate_bytes(text.as_bytes())[0].rule, 2);
    assert!(faset_editor::validate_bytes(base_text().as_bytes()).is_empty());
}

#[test]
fn rule_3_triangle_budget() {
    let mut map = base();
    // Thousands of boxes, 12 triangles each, pass the 150 000 budget. They
    // also exceed the file limit, which is a different rule.
    for i in 0..13_000 {
        let x = (i % 130) * 2 - 130;
        let z = (i / 130) * 2 - 100;
        map.brushes.push(Brush::Box {
            min: [x, 0, z],
            max: [x + 1, 1, z + 1],
            material: "metal".into(),
        });
    }
    let panel = panel_for(&map);
    assert!(panel.has_rule(3), "{:?}", panel.issues());
}

#[test]
fn rule_4_spawn_in_a_wall() {
    let mut map = base();
    map.spawns[3].at = [0.0, 0.0, -5.0];
    let panel = panel_for(&map);
    let issue = issue_of(&panel, 4);
    assert_eq!(issue.element, "spawn[3]");
    assert_eq!(issue.location, Some(Vec3::new(0.0, 0.0, -5.0)));
}

#[test]
fn rule_5_not_enough_spawns() {
    let mut map = base();
    map.spawns.truncate(3);
    assert!(panel_for(&map).has_rule(5));
}

#[test]
fn rule_6_unreachable_pickup() {
    let mut map = base();
    map.brushes.push(Brush::Box {
        min: [-40, 0, 20],
        max: [-36, 12, 24],
        material: "metal".into(),
    });
    map.pickups.push(Pickup {
        kind: PickupKind::Ammo,
        at: [-19.0, 6.5, 11.0],
        respawn_s: 20,
    });
    let panel = panel_for(&map);
    assert_eq!(issue_of(&panel, 6).element, "pickup[2]");
}

#[test]
fn rule_7_enemy_spawns_in_sight() {
    let mut map = base();
    map.spawns[8] = Spawn {
        at: [-12.0, 0.0, -7.0],
        yaw: 0.0,
        team: Team::B,
    };
    assert!(panel_for(&map).has_rule(7));
}

#[test]
fn rule_8_kill_volume() {
    let mut map = base();
    map.zones
        .retain(|Zone::Box { kind, .. }| !matches!(kind, ZoneKind::KillVolume));
    let panel = panel_for(&map);
    // A map-wide issue still has a place to go to: the middle of the map.
    assert!(issue_of(&panel, 8).location.is_some());
}

#[test]
fn rule_9_balance() {
    let mut map = base();
    for s in &mut map.spawns {
        if s.team == Team::A {
            s.at[0] = -29.0;
        }
    }
    let panel = panel_for(&map);
    assert!(issue_of(&panel, 9).location.is_some());
}

#[test]
fn validation_follows_edits_after_a_quiet_moment() {
    let mut editor = Editor::new(base());
    assert!(editor.validation.update(&editor.map, 0.0), "first run");
    assert!(editor.validation.is_ok());
    editor.map.spawns[3].at = [0.0, 0.0, -5.0];
    assert!(!editor.validation.update(&editor.map, 0.1), "debounced");
    assert!(editor.validation.is_stale());
    assert!(editor.validation.update(&editor.map, 0.3));
    assert!(editor.validation.has_rule(4));
    // Back to the good map: the issue goes away.
    editor.map = base();
    editor.validation.update(&editor.map, 0.0);
    assert!(editor.validation.update(&editor.map, 0.3));
    assert!(editor.validation.is_ok());
}

#[test]
fn clicking_an_issue_sets_the_camera_target() {
    let mut map = base();
    map.spawns[3].at = [0.0, 0.0, -5.0];
    let mut editor = Editor::new(map);
    editor.validation.refresh(&editor.map);
    let index = editor
        .validation
        .issues()
        .iter()
        .position(|i| i.rule == 4)
        .unwrap();
    let at = editor.click_issue(index).unwrap();
    assert_eq!(at, Vec3::new(0.0, 0.0, -5.0));
    assert!(editor.camera.target().distance(at) < 1e-3);
    assert!((editor.camera.eye.distance(at) - FOCUS_DISTANCE).abs() < 1e-3);
    assert_eq!(editor.validation.selected, Some(index));
    // And the same in the top view.
    editor.camera.toggle_mode();
    assert_eq!(editor.camera.mode, CameraMode::TopOrtho);
    editor.click_issue(index);
    assert!(editor.camera.top_centre.distance(Vec2::new(0.0, -5.0)) < 1e-4);
}

#[test]
fn autosave_fires_after_60_s_and_never_overwrites_the_saved_file() {
    let dir = temp_dir("autosave");
    let mut editor = Editor::new(base());
    editor.set_files(FileService::new(&dir));
    editor.save();
    let saved = editor.files.as_ref().unwrap().save_path(&editor.map);
    let saved_bytes = std::fs::read(&saved).unwrap();

    editor.map.meta.description = "edited after the save".into();
    // 59 simulated seconds: nothing yet.
    for _ in 0..59 {
        editor.update_services(1.0);
    }
    let autosave = editor.files.as_ref().unwrap().autosave_path(&editor.map);
    assert!(!autosave.exists());
    editor.update_services(1.0);
    assert!(autosave.exists());
    assert_ne!(autosave, saved);
    assert_eq!(
        std::fs::read(&saved).unwrap(),
        saved_bytes,
        "save untouched"
    );
    let restored = editor
        .files
        .as_ref()
        .unwrap()
        .load_autosave("Arena")
        .unwrap();
    assert_eq!(restored.meta.description, "edited after the save");

    // Unchanged map: the next period writes nothing.
    std::fs::remove_file(&autosave).unwrap();
    for _ in 0..60 {
        editor.update_services(1.0);
    }
    assert!(!autosave.exists());
    // Changed map: it fires again.
    editor.map.meta.description = "again".into();
    for _ in 0..60 {
        editor.update_services(1.0);
    }
    assert!(autosave.exists());
    assert_eq!(std::fs::read(&saved).unwrap(), saved_bytes);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn an_exported_map_reloads_with_the_same_fingerprint() {
    let dir = temp_dir("export");
    let files = FileService::new(dir.join("data"));
    let map = base();
    let out = dir.join("outside").join("copy.ron");
    files.export(&map, &out).unwrap();
    let reloaded = files.load(&out).unwrap();
    assert_eq!(fingerprint(&reloaded), fingerprint(&map));
    assert_eq!(std::fs::read_to_string(&out).unwrap(), to_ron(&map));

    // Import copies into maps/ without replacing what is there.
    let first = files.import(&out).unwrap();
    let second = files.import(&out).unwrap();
    assert_ne!(first.path, second.path);
    assert_eq!(fingerprint(&second.map), fingerprint(&map));
    assert!(first.path.starts_with(files.maps_dir()));

    // A bad file is refused with its rule.
    let bad = dir.join("bad.ron");
    std::fs::write(&bad, "Map(format_version: 99)").unwrap();
    assert_eq!(files.import(&bad).unwrap_err().rule(), Some(1));
    let big = dir.join("big.ron");
    std::fs::write(&big, " ".repeat(faset_mapfmt::MAX_MAP_BYTES + 1)).unwrap();
    let err = files.import(&big).unwrap_err();
    assert!(matches!(err, FileError::Map(_)));
    assert_eq!(err.rule(), Some(2));
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A map exported into the `maps/` folder is read back from there and played:
/// a client joins the integrated server and receives snapshots with the
/// player at the camera.
#[test]
fn the_test_button_runs_an_exported_map_with_a_client() {
    let dir = temp_dir("test-session");
    let files = FileService::new(&dir);
    let maps = files.maps_dir();
    files.export(&base(), &maps.join("arena.ron")).unwrap();
    let map = files.load(&maps.join("arena.ron")).unwrap();

    let mut editor = Editor::new(map);
    editor.camera.eye = Vec3::new(-15.0, 1.5, 0.0);
    editor.start_test();
    let session: TestSession = editor.take_test().expect("a test session");
    assert_eq!(session.spawn(), Vec3::new(-15.0, 1.5, 0.0));

    let (transport, server) = session.connect_local();
    let mut client = HeadlessClient::new(transport, server);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !client.in_game() && Instant::now() < deadline {
        client.step(None);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(client.in_game(), "no snapshot received");
    assert_eq!(client.map_fingerprint, Some(session.fingerprint()));
    let me = client
        .latest()
        .unwrap()
        .entity(client.player.unwrap())
        .unwrap();
    assert!(me.position.x < -10.0, "{:?}", me.position);
    assert_eq!(session.players(), 1);
    session.stop();
    std::fs::remove_dir_all(&dir).unwrap();
}

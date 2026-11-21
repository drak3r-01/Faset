//! One fixture per rule: each faulty map is rejected with its own rule id, a
//! correct map passes.

use faset_mapfmt::{MapFile, Spawn, Team};
use map_tool::{validate, validate_bytes, Settings};

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
    faset_mapfmt::parse(base_text().as_bytes()).unwrap()
}

fn rules_of(map: &MapFile) -> Vec<u8> {
    let mut r: Vec<u8> = validate(map).errors.iter().map(|e| e.rule).collect();
    r.dedup();
    r
}

#[test]
fn correct_map_passes() {
    let report = validate(&base());
    assert!(report.is_ok(), "{:?}", report.errors);
    assert!(report.map_id.is_some());
    assert!(report.stats.triangles > 0);
    assert!(report.stats.playable_surface_m2 > 1000.0);
    assert!(report.stats.longest_line_of_sight_m > 20.0);
}

#[test]
fn example_map_passes() {
    let text = include_bytes!("../../../assets/maps/example.ron");
    let report = validate_bytes(text);
    assert!(report.is_ok(), "{:?}", report.errors);
}

#[test]
fn rule_1_schema_and_version() {
    let r = validate_bytes(b"Map(format_version: 1, bogus: 3)");
    assert_eq!(r.errors.len(), 1);
    assert_eq!(r.errors[0].rule, 1);
    assert!(r.map_id.is_none());
    let text = base_text().replace("format_version: 1", "format_version: 99");
    assert_eq!(validate_bytes(text.as_bytes()).errors[0].rule, 1);
}

#[test]
fn rule_2_file_size() {
    let mut text = base_text();
    text.push_str(&" ".repeat(faset_mapfmt::MAX_MAP_BYTES));
    let r = validate_bytes(text.as_bytes());
    assert_eq!(r.errors[0].rule, 2);
}

#[test]
fn rule_3_triangle_budget() {
    let settings = Settings {
        max_triangles: 10,
        ..Settings::default()
    };
    let r = map_tool::validate_with(&base(), &settings);
    assert!(r.has_rule(3), "{:?}", r.errors);
}

#[test]
fn rule_4_spawn_in_a_wall() {
    let mut map = base();
    map.spawns[3].at = [0.0, 0.0, -5.0];
    let r = validate(&map);
    let e = r.errors.iter().find(|e| e.rule == 4).expect("rule 4");
    assert_eq!(e.element, "spawn[3]");
    assert!(e.message.contains("inside a solid"), "{}", e.message);
}

#[test]
fn rule_4_spawn_without_floor_or_headroom() {
    let mut map = base();
    map.spawns[0].at = [-15.0, 0.4, 0.0];
    map.spawns[1].at = [-15.0, 0.0, 60.0]; // off the floor
    let r = validate(&map);
    let msgs: Vec<_> = r
        .errors
        .iter()
        .filter(|e| e.rule == 4)
        .map(|e| (e.element.as_str(), e.message.as_str()))
        .collect();
    assert!(msgs
        .iter()
        .any(|(e, m)| *e == "spawn[0]" && m.contains("floats")));
    assert!(msgs
        .iter()
        .any(|(e, m)| *e == "spawn[1]" && m.contains("no floor")));
    // A low ceiling above a spawn.
    let mut map = base();
    map.brushes.push(faset_mapfmt::Brush::Box {
        min: [-34, 4, -4],
        max: [-26, 6, 4],
        material: "metal".into(),
    });
    map.spawns[4].at = [-15.0, 0.0, 1.0];
    let r = validate(&map);
    assert!(r
        .errors
        .iter()
        .any(|e| e.rule == 4 && e.message.contains("free height")));
}

#[test]
fn rule_5_not_enough_spawns() {
    let mut map = base();
    map.spawns.retain(|s| s.team != Team::B || s.at[2] < -3.0);
    assert_eq!(rules_of(&map).first(), Some(&5));
    let mut map = base();
    map.spawns.truncate(3);
    assert!(validate(&map).has_rule(5));
}

#[test]
fn rule_6_unreachable_spawn_and_pickup() {
    let mut map = base();
    // A ring of walls around the last spawn of team B.
    map.spawns[15].at = [25.0, 0.0, 15.0];
    for (min, max) in [
        ([46, 0, 26], [54, 6, 28]),
        ([46, 0, 32], [54, 6, 34]),
        ([46, 0, 28], [48, 6, 32]),
        ([52, 0, 28], [54, 6, 32]),
    ] {
        map.brushes.push(faset_mapfmt::Brush::Box {
            min,
            max,
            material: "metal".into(),
        });
    }
    map.pickups[1].at = [25.0, 0.5, 15.0];
    // A pickup high on a pillar that cannot be climbed.
    map.brushes.push(faset_mapfmt::Brush::Box {
        min: [-40, 0, 20],
        max: [-36, 12, 24],
        material: "metal".into(),
    });
    map.pickups.push(faset_mapfmt::Pickup {
        kind: faset_mapfmt::PickupKind::Ammo,
        at: [-19.0, 6.5, 11.0],
        respawn_s: 20,
    });
    let r = validate(&map);
    assert!(r
        .errors
        .iter()
        .any(|e| e.rule == 6 && e.element == "spawn[15]"));
    assert!(r
        .errors
        .iter()
        .any(|e| e.rule == 6 && e.element == "pickup[1]"));
    assert!(r
        .errors
        .iter()
        .any(|e| e.rule == 6 && e.element == "pickup[2]"));
}

#[test]
fn rule_7_enemy_spawns_in_sight() {
    let mut map = base();
    map.spawns[8] = Spawn {
        at: [-12.0, 0.0, -7.0],
        yaw: 0.0,
        team: Team::B,
    };
    let r = validate(&map);
    let e = r.errors.iter().find(|e| e.rule == 7).expect("rule 7");
    assert!(e.element.starts_with("spawn["));
    assert!(e.message.contains("line of sight"));
    // A wall between them hides the spawns.
    map.brushes.push(faset_mapfmt::Brush::Box {
        min: [-27, 0, -16],
        max: [-26, 8, -12],
        material: "metal".into(),
    });
    map.spawns[8].at = [-12.0, 0.0, -7.0];
    map.spawns[0].at = [-15.0, 0.0, -7.0];
    let walled = validate(&map);
    assert!(!walled
        .errors
        .iter()
        .any(|e| e.rule == 7 && e.element == "spawn[0]"));
}

#[test]
fn rule_8_kill_volume() {
    let mut map = base();
    map.zones.retain(|z| {
        !matches!(
            z,
            faset_mapfmt::Zone::Box {
                kind: faset_mapfmt::ZoneKind::KillVolume,
                ..
            }
        )
    });
    let r = validate(&map);
    assert!(r
        .errors
        .iter()
        .any(|e| e.rule == 8 && e.message.contains("no KillVolume")));
    // A volume covering only half of the floor.
    let mut map = base();
    for z in &mut map.zones {
        let faset_mapfmt::Zone::Box { max, kind, .. } = z;
        if matches!(kind, faset_mapfmt::ZoneKind::KillVolume) {
            max[0] = 0;
        }
    }
    let r = validate(&map);
    let e = r.errors.iter().find(|e| e.rule == 8).expect("rule 8");
    assert!(e.message.contains("not covered"), "{}", e.message);
}

#[test]
fn rule_9_balance() {
    let mut map = base();
    // Team A is twice as far from the objective and pickups as team B.
    for s in &mut map.spawns {
        if s.team == Team::A {
            s.at[0] = -29.0;
        }
    }
    let r = validate(&map);
    let e = r.errors.iter().find(|e| e.rule == 9).expect("rule 9");
    assert!(e.message.contains("team A"), "{}", e.message);
}

#[test]
fn report_is_json_with_map_id() {
    let r = validate(&base());
    let v: serde_json::Value = serde_json::from_str(&r.to_json()).unwrap();
    assert_eq!(v["report_version"], 1);
    assert_eq!(v["map_id"].as_str().unwrap().len(), 64);
    assert!(v["errors"].is_array() && v["warnings"].is_array() && v["stats"].is_object());
}

//! Acceptance tests of the map format against `assets/maps/example.ron`.

use faset_mapfmt::{
    expand_prefabs, expand_symmetry, fingerprint, parse, to_ron, Brush, MapError, Symmetry, Team,
    FORMAT_VERSION, MAX_MAP_BYTES,
};

const EXAMPLE: &[u8] = include_bytes!("../../../assets/maps/example.ron");

#[test]
fn example_round_trips_byte_identically() {
    let map = parse(EXAMPLE).unwrap();
    let text = to_ron(&map);
    if text.as_bytes() != EXAMPLE {
        // Print the canonical form so the asset can be regenerated.
        eprintln!("---- canonical ----\n{text}---- end ----");
        panic!("to_ron(parse(example)) differs from example.ron");
    }
    assert_eq!(parse(text.as_bytes()).unwrap(), map);
}

#[test]
fn example_covers_every_brush_kind_and_symmetry() {
    let map = parse(EXAMPLE).unwrap();
    assert_ne!(map.symmetry, Symmetry::None);
    let all = expand_prefabs(&map).brushes;
    let has = |f: fn(&Brush) -> bool| all.iter().any(f);
    assert!(has(|b| matches!(b, Brush::Box { .. })));
    assert!(has(|b| matches!(b, Brush::Wedge { .. })));
    assert!(has(|b| matches!(b, Brush::Cylinder { .. })));
    assert!(has(|b| matches!(b, Brush::Prism { .. })));
    assert!(has(|b| matches!(b, Brush::Stairs { .. })));
}

#[test]
fn fingerprint_is_stable_and_sensitive() {
    let map = parse(EXAMPLE).unwrap();
    let a = fingerprint(&map);
    assert_eq!(a, fingerprint(&parse(EXAMPLE).unwrap()));
    let mut moved = map.clone();
    if let Brush::Box { min, .. } = &mut moved.brushes[1] {
        min[0] += 1;
    } else {
        panic!("second brush of the example is a box");
    }
    assert_ne!(a, fingerprint(&moved));
}

#[test]
fn future_version_is_rejected() {
    let text = String::from_utf8(EXAMPLE.to_vec()).unwrap().replacen(
        "format_version: 1",
        &format!("format_version: {}", FORMAT_VERSION + 1),
        1,
    );
    assert!(matches!(
        parse(text.as_bytes()),
        Err(MapError::UnknownVersion { found }) if found == FORMAT_VERSION + 1
    ));
}

#[test]
fn unknown_field_is_rejected() {
    let text = String::from_utf8(EXAMPLE.to_vec()).unwrap().replacen(
        "    symmetry:",
        "    colour: 3,\n    symmetry:",
        1,
    );
    let err = parse(text.as_bytes()).unwrap_err();
    assert!(matches!(err, MapError::Parse(_)), "{err}");
    assert!(err.to_string().contains("colour"), "{err}");
}

#[test]
fn oversized_file_is_rejected() {
    let mut text = EXAMPLE.to_vec();
    text.resize(MAX_MAP_BYTES + 1, b' ');
    assert!(matches!(
        parse(&text),
        Err(MapError::TooLarge { size }) if size == MAX_MAP_BYTES + 1
    ));
    // Exactly at the limit is still read.
    text.truncate(MAX_MAP_BYTES);
    parse(&text).unwrap();
}

#[test]
fn degenerate_brush_is_invalid() {
    let text = String::from_utf8(EXAMPLE.to_vec()).unwrap().replacen(
        "max: (10, 3, -3), material: \"container_red\"",
        "max: (4, 3, -3), material: \"container_red\"",
        1,
    );
    assert!(matches!(parse(text.as_bytes()), Err(MapError::Invalid(_))));
}

#[test]
fn symmetry_doubles_and_swaps_teams() {
    let map = parse(EXAMPLE).unwrap();
    let flat = expand_prefabs(&map);
    let full = expand_symmetry(&map);
    assert_eq!(full.symmetry, Symmetry::None);
    assert!(full.prefabs.is_empty());
    assert_eq!(full.brushes.len(), 2 * flat.brushes.len());
    assert_eq!(full.props.len(), 2 * flat.props.len());
    assert_eq!(full.spawns.len(), 2 * flat.spawns.len());
    assert_eq!(full.pickups.len(), 2 * flat.pickups.len());
    assert_eq!(full.zones.len(), 2 * flat.zones.len());
    assert_eq!(full.lights.len(), 2 * flat.lights.len());
    let n = flat.spawns.len();
    for (orig, copy) in flat.spawns.iter().zip(&full.spawns[n..]) {
        assert_eq!(copy.team, orig.team.other());
        assert_eq!(copy.at, [-orig.at[0], orig.at[1], -orig.at[2]]);
        assert_eq!(copy.yaw, (orig.yaw + 180.0).rem_euclid(360.0));
    }
    assert!(full.spawns.iter().any(|s| s.team == Team::B));
    // The expanded map is itself a valid, round-tripping map.
    let text = to_ron(&full);
    assert_eq!(parse(text.as_bytes()).unwrap(), full);
}

#[test]
fn prefab_placements_are_expanded() {
    let map = parse(EXAMPLE).unwrap();
    let flat = expand_prefabs(&map);
    let placed: usize = map
        .prefabs
        .iter()
        .map(|p| p.place.len() * p.brushes.len())
        .sum();
    assert!(placed > 0);
    assert_eq!(flat.brushes.len(), map.brushes.len() + placed);
    assert!(flat.prefabs.is_empty());
    // First placement of the stack is a pure offset of (-30, 0, -20).
    let Brush::Box { min, max, .. } = &flat.brushes[map.brushes.len()] else {
        panic!("first prefab brush is a box");
    };
    assert_eq!((*min, *max), ([-30, 0, -20], [-18, 5, -15]));
    // Second placement is a quarter turn: x extent becomes z extent.
    let Brush::Box { min, max, .. } = &flat.brushes[map.brushes.len() + 2] else {
        panic!("first prefab brush is a box");
    };
    assert_eq!((*min, *max), ([-20, 0, -2], [-15, 5, 10]));
}

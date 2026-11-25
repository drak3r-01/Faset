//! Acceptance tests of the map format against `assets/testdata/example.ron`.

use faset_mapfmt::vegetation::scatter;
use faset_mapfmt::{
    expand_prefabs, expand_symmetry, fingerprint, parse, to_ron, Brush, DensityRule, FarField,
    MapError, Plant, Species, Symmetry, Team, Terrain, Vegetation, World, FORMAT_VERSION,
    MAX_MAP_BYTES,
};

const EXAMPLE: &[u8] = include_bytes!("../../../assets/testdata/example.ron");

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
        &format!("format_version: {FORMAT_VERSION}"),
        &format!("format_version: {}", FORMAT_VERSION + 1),
        1,
    );
    assert!(matches!(
        parse(text.as_bytes()),
        Err(MapError::UnknownVersion { found }) if found == FORMAT_VERSION + 1
    ));
}

#[test]
fn version_one_and_two_files_still_load() {
    for old in [1, 2] {
        let text = String::from_utf8(EXAMPLE.to_vec()).unwrap().replacen(
            &format!("format_version: {FORMAT_VERSION}"),
            &format!("format_version: {old}"),
            1,
        );
        let map = parse(text.as_bytes()).unwrap();
        assert_eq!(map.format_version, FORMAT_VERSION);
        assert_eq!(map, parse(EXAMPLE).unwrap());
        assert!(map.terrain.is_none());
        assert!(map.vegetation.is_none());
    }
}

#[test]
fn vegetation_round_trips_mirrors_and_is_validated() {
    let mut map = parse(EXAMPLE).unwrap();
    let veg = Vegetation {
        seed: 11,
        rules: vec![DensityRule {
            on: "concrete".into(),
            species: Species::Grass,
            density: 20.0,
        }],
        plants: vec![Plant {
            species: Species::Oak,
            at: [3.0, 0.0, -4.0],
            yaw: 30.0,
            scale: 1.2,
        }],
        symmetry: Symmetry::None,
    };
    map.vegetation = Some(veg.clone());
    let text = to_ron(&map);
    assert!(
        text.contains("vegetation:") && text.contains("seed: 11"),
        "{text}"
    );
    let back = parse(text.as_bytes()).unwrap();
    assert_eq!(back.vegetation.as_ref(), Some(&veg));
    assert_eq!(to_ron(&back), text);
    // Hand-placed plants are mirrored like props; the expansion records the
    // symmetry the rules scatter with and still round-trips.
    let full = expand_symmetry(&back);
    let v = full.vegetation.as_ref().unwrap();
    assert_eq!(v.plants.len(), 2);
    assert_eq!(v.plants[1].at, [-3.0, 0.0, 4.0]);
    assert_eq!(v.symmetry, back.symmetry);
    assert_eq!(parse(to_ron(&full).as_bytes()).unwrap(), full);
    assert_eq!(scatter(&full), scatter(&back));
    assert!(scatter(&back).len() > 2);

    let mut bad = map;
    bad.vegetation.as_mut().unwrap().rules[0].density = 1000.0;
    assert!(matches!(
        parse(to_ron(&bad).as_bytes()),
        Err(MapError::Invalid(_))
    ));
}

#[test]
fn terrain_round_trips_and_is_validated() {
    let mut map = parse(EXAMPLE).unwrap();
    let mut t = Terrain::flat([-20, -20], 4, [11, 6], 0);
    for (k, h) in t.heights.iter_mut().enumerate() {
        *h = (k as i16 * 37) % 400 - 120;
    }
    t.paint = (0..t.heights.len()).map(|k| (k % 6) as u8).collect();
    map.terrain = Some(t.clone());
    let text = to_ron(&map);
    assert!(text.contains("terrain:"), "{text}");
    let back = parse(text.as_bytes()).unwrap();
    assert_eq!(back.terrain.as_ref(), Some(&t));
    assert_eq!(to_ron(&back), text);
    // Symmetry and prefabs leave the terrain alone.
    assert_eq!(expand_symmetry(&back).terrain, Some(t.clone()));

    // A paint value out of range, or a wrong sample count, is rejected.
    let mut bad = map.clone();
    bad.terrain.as_mut().unwrap().paint[3] = 7;
    assert!(matches!(
        parse(to_ron(&bad).as_bytes()),
        Err(MapError::Invalid(_))
    ));
    let mut bad = map;
    bad.terrain.as_mut().unwrap().size = [12, 6];
    assert!(matches!(
        parse(to_ron(&bad).as_bytes()),
        Err(MapError::Invalid(_))
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

#[test]
fn surroundings_round_trip_and_are_validated() {
    use faset_mapfmt::{Border, Horizon, Surroundings};
    let mut map = parse(EXAMPLE).unwrap();
    assert!(map.surroundings.is_none());
    let s = Surroundings {
        area: [-20.0, -10.0, 20.0, 10.0],
        ground: 0.5,
        sea_level: Some(-1.0),
        north: Border::Forest,
        east: Border::Cliff,
        south: Border::Sea,
        west: Border::Slope,
        horizon: Horizon::Mountains,
        haze: 0.5,
        seed: 4,
    };
    map.surroundings = Some(s.clone());
    let text = to_ron(&map);
    assert!(text.contains("surroundings:") && text.contains("horizon: Mountains"));
    assert_eq!(
        parse(text.as_bytes()).unwrap().surroundings,
        Some(s.clone())
    );
    // Defaults are left out of the canonical form.
    map.surroundings = Some(Surroundings {
        ground: 0.0,
        sea_level: None,
        horizon: Horizon::Flat,
        haze: 1.0,
        seed: 0,
        ..s
    });
    let text = to_ron(&map);
    assert!(!text.contains("haze") && !text.contains("horizon") && !text.contains("seed"));
    assert!(parse(text.as_bytes()).is_ok());
    // A bad block is rejected at parse time.
    let bad = text.replace(
        "area: (-20.0, -10.0, 20.0, 10.0)",
        "area: (0.0, 0.0, 1.0, 1.0)",
    );
    assert!(matches!(parse(bad.as_bytes()), Err(MapError::Invalid(_))));
}

#[test]
fn version_three_files_still_load() {
    let text = String::from_utf8(EXAMPLE.to_vec()).unwrap().replacen(
        &format!("format_version: {FORMAT_VERSION}"),
        "format_version: 3",
        1,
    );
    let map = parse(text.as_bytes()).unwrap();
    assert_eq!(map.format_version, FORMAT_VERSION);
    assert!(map.surroundings.is_none());
}

#[test]
fn version_four_files_load_unchanged() {
    let text = String::from_utf8(EXAMPLE.to_vec()).unwrap().replacen(
        &format!("format_version: {FORMAT_VERSION}"),
        "format_version: 4",
        1,
    );
    let map = parse(text.as_bytes()).unwrap();
    assert_eq!(map.format_version, FORMAT_VERSION);
    assert!(map.world.is_none());
    assert_eq!(map, parse(EXAMPLE).unwrap());
}

#[test]
fn every_shipped_map_parses() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/maps");
    let mut count = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "ron") {
            let bytes = std::fs::read(&path).unwrap();
            let map = parse(&bytes).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            assert_eq!(parse(to_ron(&map).as_bytes()).unwrap(), map);
            count += 1;
        }
    }
    assert!(count >= 7);
}

#[test]
fn world_round_trips_and_is_validated() {
    let mut map = parse(EXAMPLE).unwrap();
    let (nx, nz) = (17u16, 13u16);
    let heights: Vec<i32> = (0..nz)
        .flat_map(|j| (0..nx).map(move |i| i32::from(i) * 7 - i32::from(j) * 3))
        .collect();
    let mut paint = vec![0u8; usize::from(nx) * usize::from(nz)];
    paint[20..30].fill(3);
    map.world = Some(World {
        extent: [-128.0, -96.0, 128.0, 96.0],
        playable: [-30.0, -20.0, 30.0, 20.0],
        far: FarField {
            cell: 16,
            size: [nx, nz],
            heights,
            paint,
        },
        snow_line: Some(40.0),
    });
    let text = to_ron(&map);
    assert!(text.contains("world: (extent:") || text.contains("world: ("));
    let back = parse(text.as_bytes()).unwrap();
    assert_eq!(back, map);
    assert_eq!(fingerprint(&back), fingerprint(&map));
    // A far field that does not span the extent is rejected at parse time.
    let bad = text.replace("cell: 16", "cell: 15");
    assert!(matches!(parse(bad.as_bytes()), Err(MapError::Invalid(_))));
}

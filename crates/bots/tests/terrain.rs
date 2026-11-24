//! Lot 6.10: the navigation graph on terrain: slopes are walked, a bank
//! steeper than 45 degrees is not climbed, and slopes cost time.

use faset_bots::{EdgeKind, NavGraph, DEFAULT_VMAX};
use faset_core::glam::Vec3;
use faset_mapfmt::{MapFile, Terrain};
use faset_sim::controller::MoveParams;

/// 60 x 60 m: a smooth 4 m hill in the middle (about 22 degrees at most)
/// and a 6 m mesa with 71 degree sides around (20, 20).
fn terrain() -> Terrain {
    let mut t = Terrain::flat([-60, -60], 4, [31, 31], 0);
    for j in 0..31 {
        for i in 0..31 {
            let p = t.sample_pos(i, j);
            let hill = 4.0 * (-(p.x * p.x + p.z * p.z) / 72.0).exp();
            let mesa = if (16.0..=24.0).contains(&p.x) && (16.0..=24.0).contains(&p.z) {
                6.0
            } else {
                0.0
            };
            let k = t.index(i, j);
            t.heights[k] = ((hill + mesa) * 100.0).round() as i16;
        }
    }
    t
}

fn map() -> MapFile {
    let mut map = faset_mapfmt::parse(
        b"Map(format_version: 2, meta: (name: \"hill\", modes: [\"fps\"], players: (1, 2)), \
          theme: (ambience: \"day\", palette: \"nature\"), \
          spawns: [(at: (-24.0, 0.0, 0.0), yaw: 0.0, team: A), (at: (24.0, 0.0, 0.0), yaw: 0.0, team: B)])",
    )
    .unwrap();
    map.terrain = Some(terrain());
    map
}

#[test]
fn slopes_are_walked_and_steep_banks_are_not() {
    let t = terrain();
    let g = NavGraph::build(&map(), &MoveParams::default());
    assert!(g.spawn_connectivity().all_connected());

    // The hill top has nodes and is reached on foot from a spawn.
    let top = g
        .nearest_node(Vec3::new(0.0, 4.0, 0.0))
        .expect("a node on the hill");
    assert!((g.node(top).pos.y - 4.0).abs() < 0.2, "{}", g.node(top).pos);
    let ground = g.reachable_from(g.spawns()[0]);
    assert!(ground.contains(top), "the hill top is not reachable");

    // The mesa top has nodes, but its 71 degree sides cannot be walked up:
    // from the ground it is out of reach, from its top a drop leads down.
    let mesa = g
        .nearest_node(Vec3::new(20.0, 6.0, 20.0))
        .expect("a node on the mesa");
    assert!(g.node(mesa).pos.y > 5.9, "{}", g.node(mesa).pos);
    assert!(!ground.contains(mesa), "the mesa was climbed");
    assert!(g.reachable_from_node(mesa).contains(top));

    // Every walk edge between terrain nodes stays within the 45 degree limit
    // and costs more than the flat walking time on a slope.
    let run = MoveParams::default().run_speed;
    let mut sloped = 0;
    for a in 0..g.node_count() as u32 {
        for e in g.edges_from(a).iter().filter(|e| e.kind == EdgeKind::Walk) {
            let (pa, pb) = (g.node(a).pos, g.node(e.to).pos);
            let flat = Vec3::new(pb.x - pa.x, 0.0, pb.z - pa.z).length();
            let rise = (pb.y - pa.y).abs();
            assert!(rise <= flat + 0.46, "{pa} -> {pb}");
            if rise > 0.1 * flat && rise > 0.05 {
                sloped += 1;
                assert!(
                    e.time > pa.distance(pb) / run,
                    "{pa} -> {pb}: no slope cost"
                );
            }
        }
    }
    assert!(sloped > 100, "{sloped}");

    // A path across the map exists and every waypoint lies on the surface.
    let path = g
        .find_path(
            Vec3::new(-24.0, 0.0, 0.0),
            Vec3::new(24.0, 0.0, -10.0),
            DEFAULT_VMAX,
        )
        .expect("a path across the map");
    for w in &path.waypoints {
        let h = t.height_at(w.pos.x, w.pos.z).unwrap();
        assert!((w.pos.y - h).abs() < 0.1, "{} off the surface ({h})", w.pos);
    }
}

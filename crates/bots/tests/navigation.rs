//! Navigation graph acceptance tests (lot 4.9).

use std::time::Instant;

use faset_bots::{EdgeKind, NavGraph, Node, DEFAULT_VMAX};
use faset_core::glam::Vec3;
use faset_mapfmt::ZoneKind;
use faset_sim::controller::MoveParams;
use faset_sim::zones::ZoneVolume;
use faset_sim::{StaticWorld, Volume, Zones};
use parry3d::math::{Pose, Vector};
use parry3d::shape::SharedShape;

fn params() -> MoveParams {
    MoveParams::default()
}

/// Axis-aligned slab with its top face at `top`, spanning `x` and `z`.
fn slab(x: (f32, f32), z: (f32, f32), top: f32) -> Volume {
    let (cx, cz) = ((x.0 + x.1) * 0.5, (z.0 + z.1) * 0.5);
    let (hx, hz) = ((x.1 - x.0) * 0.5, (z.1 - z.0) * 0.5);
    Volume::cuboid(
        Vec3::new(cx, (top - 1.0) * 0.5, cz),
        Vec3::new(hx, (top + 1.0) * 0.5, hz),
    )
}

fn build(volumes: Vec<Volume>, zones: Vec<ZoneVolume>, spawns: Vec<Vec3>) -> NavGraph {
    NavGraph::build_world(
        StaticWorld::new(volumes),
        &Zones::new(zones),
        spawns,
        &params(),
    )
}

fn nodes_where(g: &NavGraph, f: impl Fn(&Node) -> bool) -> Vec<u32> {
    (0..g.node_count() as u32)
        .filter(|&i| f(g.node(i)))
        .collect()
}

/// Whether some edge of `kind` links a node satisfying `from` to one
/// satisfying `to`.
fn has_edge(
    g: &NavGraph,
    kind: EdgeKind,
    from: impl Fn(&Node) -> bool,
    to: impl Fn(&Node) -> bool,
) -> bool {
    nodes_where(g, from).into_iter().any(|a| {
        g.edges_from(a)
            .iter()
            .any(|e| e.kind == kind && to(g.node(e.to)))
    })
}

#[test]
fn example_map_accessibility() {
    let text = include_str!("../../../assets/maps/example.ron");
    let map = faset_mapfmt::parse(text.as_bytes()).unwrap();
    let t = Instant::now();
    let g = NavGraph::build(&map, &params());
    let build_time = t.elapsed();
    let spawn_count = g.spawns().len();
    assert!(spawn_count >= 2, "the example map has spawns");
    let conn = g.spawn_connectivity();
    let set = g.reachable_from(g.spawns()[0]);
    let mut kinds = [0usize; 5];
    for i in 0..g.node_count() as u32 {
        for e in g.edges_from(i) {
            kinds[e.kind as usize] += 1;
        }
    }
    println!(
        "example map: {} nodes, {} edges (walk {}, drop {}, jump {}, pad {}, teleport {}), \
         {} spawns, reachable cell fraction {:.4} ({} of {}), graph build time {:?}",
        g.node_count(),
        g.edge_count(),
        kinds[0],
        kinds[1],
        kinds[2],
        kinds[3],
        kinds[4],
        spawn_count,
        set.fraction(),
        set.len(),
        g.node_count(),
        build_time,
    );
    assert!(conn.all_connected(), "{conn:?}");
    assert!(set.fraction() > 0.0);
}

#[test]
fn step_of_045_is_linked_and_046_is_not() {
    for (height, linked) in [(0.45, true), (0.46, false)] {
        let g = build(
            vec![
                slab((-10.0, 10.0), (-10.0, 10.0), 0.0),
                slab((2.0, 8.0), (-4.0, 4.0), height),
            ],
            vec![],
            vec![],
        );
        let low = |n: &Node| n.pos.y.abs() < 0.01;
        let high = |n: &Node| (n.pos.y - height).abs() < 0.01;
        assert!(!nodes_where(&g, high).is_empty());
        let walk = has_edge(&g, EdgeKind::Walk, low, high);
        assert_eq!(walk, linked, "step of {height} m");
        // The step down is the same link.
        assert_eq!(has_edge(&g, EdgeKind::Walk, high, low), linked);
    }
}

/// A ramp rising along +x over 4 m, with a solid base.
fn ramp(normal_y: f32) -> Volume {
    let run = 4.0;
    let rise = run * (1.0 - normal_y * normal_y).sqrt() / normal_y;
    let mut pts = Vec::new();
    for z in [-3.0, 3.0] {
        pts.push(Vector::new(0.0, -1.0, z));
        pts.push(Vector::new(run, -1.0, z));
        pts.push(Vector::new(0.0, 0.0, z));
        pts.push(Vector::new(run, rise, z));
    }
    Volume {
        shape: SharedShape::convex_hull(&pts).expect("a ramp hull"),
        pose: Pose::IDENTITY,
    }
}

#[test]
fn slope_of_ny_070_is_rejected_and_071_accepted() {
    for (ny, accepted) in [(0.70, false), (0.71, true)] {
        let g = build(vec![ramp(ny)], vec![], vec![]);
        let on_ramp = nodes_where(&g, |n| (n.normal.y - ny).abs() < 0.01);
        assert_eq!(!on_ramp.is_empty(), accepted, "n_y {ny}");
        if accepted {
            // The ramp is walked from its foot to its top.
            let top = Vec3::new(3.6, 3.9, 0.0);
            let foot = Vec3::new(0.3, 0.1, 0.0);
            assert!(g.find_path(foot, top, DEFAULT_VMAX).is_some());
        }
    }
}

#[test]
fn jump_edge_only_where_the_simulated_landing_is_walkable() {
    // Platform A, then B across a 3 m gap (reachable), C across 8 m (not).
    let a = slab((-10.0, -2.0), (-4.0, 4.0), 0.0);
    let b = slab((1.0, 9.0), (-4.0, 4.0), 0.0);
    let c = slab((17.0, 25.0), (-4.0, 4.0), 0.0);
    let g = build(vec![a, b, c], vec![], vec![]);
    let on_a = |n: &Node| n.pos.x < -2.0;
    let on_b = |n: &Node| n.pos.x > 1.0 && n.pos.x < 9.0;
    let on_c = |n: &Node| n.pos.x > 17.0;
    assert!(has_edge(&g, EdgeKind::Jump, on_a, on_b), "3 m gap");
    assert!(has_edge(&g, EdgeKind::Jump, on_b, on_a), "and back");
    assert!(!has_edge(&g, EdgeKind::Jump, on_b, on_c), "8 m gap");
    assert!(!has_edge(&g, EdgeKind::Jump, on_a, on_c));
    assert!(!has_edge(&g, EdgeKind::Drop, on_a, on_b), "same height");
}

#[test]
fn ledges_give_drop_edges_and_one_way_connectivity() {
    // A 3 m high platform on a floor: walking off drops, climbing is out of
    // reach of a 1.06 m jump.
    let g = build(
        vec![
            slab((-10.0, 10.0), (-10.0, 10.0), 0.0),
            slab((-2.0, 2.0), (-2.0, 2.0), 3.0),
        ],
        vec![],
        vec![Vec3::new(0.0, 3.0, 0.0), Vec3::new(8.0, 0.0, 8.0)],
    );
    let top = |n: &Node| n.pos.y > 2.9;
    let floor = |n: &Node| n.pos.y.abs() < 0.01;
    assert!(has_edge(&g, EdgeKind::Drop, top, floor));
    assert!(!has_edge(&g, EdgeKind::Walk, top, floor));
    assert!(!has_edge(&g, EdgeKind::Jump, floor, top));
    let conn = g.spawn_connectivity();
    assert!(conn.unplaced.is_empty());
    assert_eq!(conn.unreachable, vec![(1, 0)], "no way back up");
    assert!(!conn.all_connected());
}

#[test]
fn pads_and_teleporters_get_their_own_edges() {
    let zone = |min: Vec3, max: Vec3, kind| ZoneVolume { min, max, kind };
    let g = build(
        vec![
            slab((-10.0, -2.0), (-4.0, 4.0), 0.0),
            slab((4.0, 12.0), (-4.0, 4.0), 0.0),
            slab((40.0, 48.0), (-4.0, 4.0), 0.0),
        ],
        vec![
            zone(
                Vec3::new(-4.0, 0.0, -0.5),
                Vec3::new(-3.0, 1.0, 0.5),
                ZoneKind::JumpPad {
                    velocity: [10.0, 10.0, 0.0],
                },
            ),
            zone(
                Vec3::new(-8.0, 0.0, -0.5),
                Vec3::new(-7.0, 1.0, 0.5),
                ZoneKind::Teleporter {
                    target: [44.0, 0.0, 0.0],
                    yaw: 0.0,
                },
            ),
        ],
        vec![],
    );
    let on_a = |n: &Node| n.pos.x < -2.0;
    let on_b = |n: &Node| n.pos.x > 4.0 && n.pos.x < 12.0;
    let on_c = |n: &Node| n.pos.x > 40.0;
    assert!(has_edge(&g, EdgeKind::Pad, on_a, on_b));
    assert!(has_edge(&g, EdgeKind::Teleport, on_a, on_c));
    // A path from A to C uses the teleporter.
    let p = g
        .find_path(
            Vec3::new(-9.0, 0.0, 0.0),
            Vec3::new(44.0, 0.0, 0.0),
            DEFAULT_VMAX,
        )
        .expect("a path through the teleporter");
    assert!(p.waypoints.iter().any(|w| w.kind == EdgeKind::Teleport));
    // Pad nodes only have the pad edge out.
    for i in nodes_where(&g, |n| n.trigger.is_some()) {
        assert!(g.edges_from(i).iter().all(|e| e.kind != EdgeKind::Walk));
    }
}

#[test]
fn kill_volumes_remove_nodes_and_edges_into_them() {
    let zone = ZoneVolume {
        min: Vec3::new(-1.0, -1.0, -5.0),
        max: Vec3::new(1.0, 1.0, 5.0),
        kind: ZoneKind::KillVolume,
    };
    let g = build(
        vec![slab((-10.0, 10.0), (-5.0, 5.0), 0.0)],
        vec![zone],
        vec![],
    );
    assert!(nodes_where(&g, |n| n.pos.x.abs() < 1.0).is_empty());
    assert!(!nodes_where(&g, |n| n.pos.x > 1.5).is_empty());
}

#[test]
fn paths_go_around_walls_and_run_straight_in_the_open() {
    let floor = slab((-20.0, 20.0), (-20.0, 20.0), 0.0);
    let wall = Volume::cuboid(Vec3::new(0.0, 1.5, 0.0), Vec3::new(0.5, 1.5, 8.0));
    let open = build(vec![floor.clone()], vec![], vec![]);
    let from = Vec3::new(-5.0, 0.0, 0.0);
    let to = Vec3::new(5.0, 0.0, 0.0);
    let p = open.find_path(from, to, DEFAULT_VMAX).unwrap();
    assert_eq!(p.waypoints.len(), 2, "smoothed to the straight line");
    assert!(
        (p.time - 10.0 / params().run_speed).abs() < 0.3,
        "{}",
        p.time
    );

    let walled = build(vec![floor, wall], vec![], vec![]);
    let p = walled.find_path(from, to, DEFAULT_VMAX).unwrap();
    assert!(p.waypoints.len() >= 3, "{:?}", p.waypoints);
    assert!(p.waypoints.iter().all(|w| w.kind == EdgeKind::Walk));
    let detour = p.waypoints.iter().any(|w| w.pos.z.abs() > 8.0);
    assert!(detour, "the path passes the end of the wall");
    assert!(p.time > 10.0 / params().run_speed);
}

#[test]
fn building_twice_gives_the_same_graph() {
    let text = include_str!("../../../assets/maps/example.ron");
    let map = faset_mapfmt::parse(text.as_bytes()).unwrap();
    let (a, b) = (
        NavGraph::build(&map, &params()),
        NavGraph::build(&map, &params()),
    );
    assert_eq!(a.node_count(), b.node_count());
    assert_eq!(a.edge_count(), b.edge_count());
    for i in 0..a.node_count() as u32 {
        assert_eq!(a.node(i), b.node(i));
        assert_eq!(a.edges_from(i), b.edges_from(i));
    }
}

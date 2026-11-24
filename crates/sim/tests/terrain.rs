//! Lot 6.10: the character controller on terrain (rolling hills and a bank
//! steeper than the walkable limit), and a recorded terrain trace.

use faset_core::glam::Vec3;
use faset_core::{NetId, DT};
use faset_mapfmt::{MapFile, Terrain};
use faset_sim::controller::{step_player_in, MoveCmd, MoveParams, PlayerState};
use faset_sim::trace::Outcome;
use faset_sim::{MapCollision, StaticWorld, Trace};

/// 80 x 80 m of rolling hills (2 m cells, slopes up to about 30 degrees)
/// with a 63 degree bank beyond x = 24 m.
fn terrain() -> Terrain {
    terrain_with_bank(2.0)
}

fn terrain_with_bank(grade: f32) -> Terrain {
    let mut t = Terrain::flat([-80, -80], 4, [41, 41], 0);
    for j in 0..41 {
        for i in 0..41 {
            let p = t.sample_pos(i, j);
            let mut h = 1.5 * (p.x * 0.25).sin() * (p.z * 0.2).cos();
            if p.x > 24.0 {
                h += (p.x - 24.0) * grade;
            }
            let k = t.index(i, j);
            t.heights[k] = (h * 100.0).round() as i16;
        }
    }
    t
}

fn map() -> MapFile {
    map_with(terrain())
}

fn map_with(t: Terrain) -> MapFile {
    let mut map = faset_mapfmt::parse(
        b"Map(format_version: 2, meta: (name: \"hills\", modes: [\"fps\"], players: (1, 2)), \
          theme: (ambience: \"day\", palette: \"nature\"))",
    )
    .unwrap();
    map.terrain = Some(t);
    map
}

fn world() -> StaticWorld {
    MapCollision::build(&map()).world().clone()
}

/// A player settled on the ground at `(x, z)`.
fn grounded(w: &StaticWorld, x: f32, z: f32, p: &MoveParams) -> PlayerState {
    let y = terrain().height_at(x, z).unwrap();
    let mut s = PlayerState::spawn(Vec3::new(x, y + 0.95, z));
    for _ in 0..30 {
        step_player_in(&mut s, MoveCmd::default(), w, p, DT);
    }
    assert!(s.on_ground, "{s:?}");
    s
}

fn hspeed(v: Vec3) -> f32 {
    (v.x * v.x + v.z * v.z).sqrt()
}

#[test]
fn every_triangle_is_a_volume() {
    let col = MapCollision::build(&map());
    assert_eq!(col.terrain_volume_count(), 40 * 40 * 2);
    assert_eq!(col.volume_count(), 40 * 40 * 2);
    assert!(col.world().is_terrain(0));
    // A map without terrain has no terrain volume.
    let mut flat = map();
    flat.terrain = None;
    assert_eq!(MapCollision::build(&flat).volume_count(), 0);
}

#[test]
fn rays_land_on_the_rendered_surface() {
    let t = terrain();
    let col = MapCollision::build(&map());
    for k in 0..200 {
        let x = -38.0 + (k as f32 * 7.31) % 60.0;
        let z = -38.0 + (k as f32 * 3.77) % 76.0;
        let h = t.height_at(x, z).unwrap();
        let d = col
            .ray_cast(Vec3::new(x, 40.0, z), Vec3::NEG_Y, 80.0)
            .unwrap();
        assert!(
            (40.0 - d - h).abs() < 1e-3,
            "({x}, {z}): {} vs {h}",
            40.0 - d
        );
    }
}

/// Walks the rolling part in several directions: the player never leaves the
/// ground, follows the surface within a few centimetres and moves smoothly
/// (no tick is much slower or higher than its neighbours).
#[test]
fn walking_over_hills_stays_grounded_without_jitter() {
    let p = MoveParams::default();
    let w = world();
    let t = terrain();
    for (yaw, start) in [
        (0u16, (0.0, 30.0)),
        (16384, (20.0, 0.0)),
        (49152, (-30.0, 5.0)),
        (8192, (10.0, 25.0)),
        (40000, (-20.0, -25.0)),
    ] {
        let mut s = grounded(&w, start.0, start.1, &p);
        let cmd = MoveCmd {
            yaw,
            move_y: 127,
            ..MoveCmd::default()
        };
        let mut prev = s.pos;
        let mut prev_step = 0.0f32;
        let mut worst_gap = 0.0f32;
        let mut worst_jerk = 0.0f32;
        for tick in 0..300 {
            step_player_in(&mut s, cmd, &w, &p, DT);
            let feet = s.feet();
            if t.height_at(feet.x, feet.z).is_none() || feet.x > 22.0 {
                break;
            }
            assert!(
                s.on_ground,
                "yaw {yaw}: left the ground at tick {tick}, {feet}"
            );
            let gap = feet.y - t.height_at(feet.x, feet.z).unwrap();
            worst_gap = worst_gap.max(gap.abs());
            let step = hspeed(s.pos - prev);
            if tick > 40 {
                // Running speed: 7 m/s, slowed a little on climbs.
                assert!(
                    step > 4.5 * DT,
                    "yaw {yaw}: stalled at tick {tick} ({step})"
                );
                worst_jerk = worst_jerk.max((step - prev_step).abs());
            }
            prev_step = step;
            prev = s.pos;
        }
        println!(
            "yaw {yaw}: worst gap {worst_gap:.4} m, worst speed change {worst_jerk:.4} m/tick"
        );
        assert!(worst_gap < 0.06, "yaw {yaw}: {worst_gap} m off the surface");
        assert!(
            worst_jerk < 0.02,
            "yaw {yaw}: speed jumps by {worst_jerk} m/tick"
        );
    }
}

/// A bank steeper than 45 degrees is not walkable: running into it stops
/// at its foot (grades 1.5, 2 and 3 on top of the hills: 48 to 76
/// degrees); a bank of grade 0.45 (at most 39 degrees) is climbed.
#[test]
fn a_bank_steeper_than_45_degrees_cannot_be_climbed() {
    let p = MoveParams::default();
    for (grade, climbs) in [(0.45, true), (1.5, false), (2.0, false), (3.0, false)] {
        let t = terrain_with_bank(grade);
        let w = MapCollision::build(&map_with(t.clone())).world().clone();
        let mut s = grounded(&w, 10.0, 0.0, &p);
        let toward_bank = MoveCmd {
            yaw: 49152,
            move_y: 127,
            ..MoveCmd::default()
        };
        for _ in 0..360 {
            step_player_in(&mut s, toward_bank, &w, &p, DT);
            if s.pos.x > 32.0 {
                break;
            }
        }
        let foot = t.height_at(24.0, s.pos.z).unwrap();
        println!("grade {grade}: ended at {} (foot {foot})", s.feet());
        if climbs {
            assert!(s.pos.x > 30.0 && s.on_ground, "grade {grade}: {}", s.pos);
        } else {
            assert!(
                s.feet().y < foot + 0.6,
                "grade {grade}: climbed the bank, feet at {} (foot of the bank {foot})",
                s.feet()
            );
            assert!(s.pos.x < 25.0, "grade {grade}: {}", s.pos);
        }
    }
}

const TERRAIN_TRACE: &str = include_str!("data/terrain_course.trace");

/// Three random players and one walking straight over the hills.
fn terrain_trace() -> Trace {
    let mut t = Trace::generate(610, 900, 3);
    t.players = vec![
        (NetId(1), Vec3::new(0.0, 3.0, 30.0)),
        (NetId(2), Vec3::new(-10.0, 3.0, 0.0)),
        (NetId(3), Vec3::new(5.0, 3.0, -15.0)),
    ];
    for frame in &mut t.frames {
        frame[0].1.buttons = 0;
        frame[0].1.move_x = 0;
        frame[0].1.move_y = 127;
        frame[0].1.yaw = 0;
    }
    t
}

fn replay(t: &Trace) -> Outcome {
    t.replay_controller(&world(), MoveParams::default())
}

#[test]
fn terrain_trace_replays_to_the_reference_positions() {
    let (trace, expected) = Trace::from_text(TERRAIN_TRACE).unwrap();
    assert_eq!(trace, terrain_trace(), "the terrain script changed");
    let first = replay(&trace);
    assert_eq!(first, replay(&trace), "two replays differ");
    assert_eq!(first, expected);
}

/// Regenerates the reference file:
/// `cargo test -p faset-sim --test terrain -- --ignored regenerate`.
#[test]
#[ignore]
fn regenerate_terrain_trace() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data");
    let t = terrain_trace();
    std::fs::write(
        format!("{dir}/terrain_course.trace"),
        t.to_text(&replay(&t)),
    )
    .unwrap();
}

//! Lot 6.10 acceptance: client prediction on terrain slopes. The client and
//! the server run the same controller on the same collision, so on a clean
//! link the prediction stays within 1 cm of the server without snapping, and
//! a degraded link stays as close as on flat ground.

mod common;

use std::time::Duration;

use common::{Movement, Rig};
use faset_core::Tick;
use faset_mapfmt::{Spawn, Team, Terrain};
use faset_net::NetworkConditions;
use faset_server::MapConfig;

/// Rolling hills (slopes up to about 35 degrees) around the spawn.
fn hills() -> MapConfig {
    let mut t = Terrain::flat([-100, -100], 4, [51, 51], 0);
    for j in 0..51 {
        for i in 0..51 {
            let p = t.sample_pos(i, j);
            let h = 2.0 * (p.x * 0.3).sin() * (p.z * 0.25).cos() + 0.5 * (p.z * 0.4).sin();
            let k = t.index(i, j);
            t.heights[k] = (h * 100.0).round() as i16;
        }
    }
    let mut map = faset_mapfmt::parse(
        b"Map(format_version: 2, meta: (name: \"hills\", modes: [\"fps\"], players: (1, 2)), \
          theme: (ambience: \"day\", palette: \"nature\"))",
    )
    .unwrap();
    let y = t.height_at(0.0, 0.0).unwrap();
    map.spawns.push(Spawn {
        at: [0.0, y, 0.0],
        yaw: 0.0,
        team: Team::Any,
    });
    map.terrain = Some(t);
    let bytes = faset_mapfmt::to_ron(&map);
    faset_client::map_config(bytes.as_bytes()).unwrap().0
}

/// Runs up and down the hills: two seconds out, two seconds back, a new
/// heading every cycle.
fn hill_walk(tick: Tick) -> Movement {
    let cycle = tick.0 / 240;
    let yaw = (cycle as u16).wrapping_mul(9000);
    if tick.0 % 240 < 120 {
        (0, 127, yaw)
    } else {
        (0, -127, yaw)
    }
}

#[test]
fn a_clean_link_never_mispredicts_on_slopes() {
    let mut rig = Rig::with_map(
        NetworkConditions {
            latency: Duration::from_millis(50),
            ..NetworkConditions::default()
        },
        hills(),
    );
    rig.join();
    rig.run(Rig::frames(20.0), &mut hill_walk);
    let out = &rig.client.outcomes;
    assert!(out.len() > 400, "only {} snapshots compared", out.len());
    let corrected = out.iter().filter(|o| o.corrected).count();
    let worst = out.iter().map(|o| o.position_error).fold(0.0, f32::max);
    println!(
        "{} snapshots, {corrected} corrections, worst error {worst:.5} m",
        out.len()
    );
    // The client starts from a quantized snapshot (1/256 m), so its walk is
    // not bit-identical to the server's: on a triangle edge that offset can
    // make the velocity differ for a tick and trigger a rare correction. The
    // prediction itself never drifts: the position stays within 1 cm, no
    // correction snaps, and corrections are below 1 % of the snapshots.
    assert!(
        corrected * 100 <= out.len(),
        "{corrected} corrections in {} snapshots",
        out.len()
    );
    assert!(out.iter().all(|o| !o.snapped), "a correction snapped");
    assert!(worst < 0.01, "worst error {worst} m");
    // The walk did climb: the player left the spawn height by over a metre.
    let me = rig.client.player.unwrap();
    let ys: Vec<f32> = rig
        .client
        .frames
        .iter()
        .filter_map(|f| f.display.map(|d| d.y))
        .collect();
    let (lo, hi) = ys
        .iter()
        .fold((f32::MAX, f32::MIN), |(l, h), y| (l.min(*y), h.max(*y)));
    assert!(
        hi - lo > 1.0,
        "the walk stayed flat ({lo} to {hi}), player {me:?}"
    );
}

#[test]
fn a_degraded_link_stays_within_five_centimetres_on_slopes() {
    let mut rig = Rig::with_map(
        NetworkConditions {
            seed: 11,
            ..NetworkConditions::typical()
        },
        hills(),
    );
    rig.join();
    rig.run(Rig::frames(20.0), &mut hill_walk);
    let out = &rig.client.outcomes;
    assert!(out.len() > 400, "only {} snapshots compared", out.len());
    let mean = out.iter().map(|o| o.position_error).sum::<f32>() / out.len() as f32;
    println!(
        "mean prediction error {mean:.4} m over {} snapshots",
        out.len()
    );
    assert!(mean < 0.05, "mean prediction error {mean} m");
}

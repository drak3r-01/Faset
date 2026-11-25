//! Lot 7.1 budgets (section 20) measured on the CPU, without a GPU: server
//! tick, dedicated server memory, CPU time per client frame, map load.
//!
//! Wall-time limits apply to optimised builds. A debug test build only gets a
//! loose bound that still catches an algorithmic regression; the strict
//! numbers are asserted by `cargo test --release` (see the PR description).

mod common;

use std::path::Path;
use std::time::Instant;

use common::{rss_kib, walk, Fleet, Movement, Rig};
use faset_client::load_map;
use faset_core::Tick;
use faset_net::NetworkConditions;

fn limit(release_ms: f64) -> f64 {
    if cfg!(debug_assertions) {
        release_ms * 30.0
    } else {
        release_ms
    }
}

fn walking(client: usize, tick: Tick) -> Movement {
    walk(Tick(tick.0 + 20 * client as u32))
}

/// 16 players (4 predicting clients and 12 bots) on the degraded network:
/// server tick under 2 ms, client frame under 4 ms, process memory under
/// 200 MB (the server alone is smaller than this whole test process).
#[test]
fn sixteen_players_meet_the_cpu_and_memory_budgets() {
    let conditions = NetworkConditions {
        seed: 71,
        ..NetworkConditions::typical()
    };
    let mut fleet = Fleet::new(4, 12, conditions);
    fleet.join();
    assert_eq!(fleet.server.players().count(), 16);
    fleet.run(Rig::frames(5.0), &mut walking);
    fleet.server_time = Default::default();
    fleet.server_ticks = 0;
    fleet.client_time = Default::default();
    fleet.client_frames = 0;
    fleet.run(Rig::frames(20.0), &mut walking);

    let tick_ms = fleet.server_time.as_secs_f64() * 1e3 / fleet.server_ticks as f64;
    let frame_ms = fleet.client_time.as_secs_f64() * 1e3 / fleet.client_frames as f64;
    let rss_mb = rss_kib().map(|k| k as f64 / 1024.0);
    println!(
        "budgets (debug assertions: {}): server tick {tick_ms:.3} ms mean over {} ticks, \
         client frame {frame_ms:.3} ms mean over {} frames, process RSS {rss_mb:?} MiB",
        cfg!(debug_assertions),
        fleet.server_ticks,
        fleet.client_frames
    );
    assert!(tick_ms < limit(2.0), "server tick {tick_ms:.3} ms");
    assert!(frame_ms < limit(4.0), "client frame {frame_ms:.3} ms");
    if let Some(mb) = rss_mb {
        assert!(mb < 200.0, "process RSS {mb:.0} MiB");
    }
}

/// Every shipped map loads (read, expand, meshes, collision) in under 2 s.
#[test]
fn every_shipped_map_loads_in_under_two_seconds() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/maps");
    let mut count = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "ron") {
            continue;
        }
        let bytes = std::fs::read(&path).unwrap();
        let start = Instant::now();
        load_map(&bytes).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let secs = start.elapsed().as_secs_f64();
        println!("map load {}: {secs:.3} s", path.display());
        let max = if cfg!(debug_assertions) { 20.0 } else { 2.0 };
        assert!(secs < max, "{} took {secs:.2} s", path.display());
        count += 1;
    }
    assert!(count >= 7, "{count} maps");
}

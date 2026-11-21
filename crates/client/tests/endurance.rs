//! Lot 1.10 endurance: 16 players (4 predicting clients and 12 bots) for ten
//! minutes of simulated time on a degraded network, with client churn. No
//! panic, and memory stable after warm-up. Ignored by default:
//! `cargo nextest run --run-ignored all -E 'test(endurance)'`.

mod common;

use common::{rss_kib, walk, Fleet, Movement, Rig};
use faset_core::Tick;
use faset_net::NetworkConditions;

/// Simulated seconds.
const DURATION: u32 = 600;
/// Seconds before the memory baseline is taken.
const WARM_UP: u32 = 60;
/// Largest accepted growth over the baseline, KiB (decision: 4 MiB; measured about 0.25 MiB).
const MAX_GROWTH_KIB: u64 = 4 * 1024;

fn walking(client: usize, tick: Tick) -> Movement {
    walk(Tick(tick.0 + 20 * client as u32))
}

#[test]
#[ignore = "ten simulated minutes; run with --run-ignored all"]
fn endurance_sixteen_players_ten_minutes() {
    let conditions = NetworkConditions {
        seed: 99,
        ..NetworkConditions::typical()
    };
    let mut fleet = Fleet::new(4, 12, conditions);
    fleet.join();

    let mut samples: Vec<(u32, u64)> = Vec::new();
    let mut baseline = None;
    let started = std::time::Instant::now();
    for second in 1..=DURATION {
        fleet.run(Rig::frames(1.0), &mut walking);
        // The helper clients log every frame; a real client does not, so
        // keep those logs from masking (or faking) a leak.
        for c in &mut fleet.clients {
            c.frames.clear();
            c.outcomes.clear();
            c.ticks.clear();
        }
        // One player leaves and a new one joins every 90 s.
        if second % 90 == 0 && second + 20 < DURATION {
            fleet.reconnect((second / 90) as usize % 4);
        }
        assert!(
            fleet.server.players().count() <= 20,
            "{} players at {second} s",
            fleet.server.players().count()
        );
        if second % 30 == 0 {
            if let Some(rss) = rss_kib() {
                samples.push((second, rss));
                if second == WARM_UP {
                    baseline = Some(rss);
                }
            }
        }
    }
    // Let the last churned connection time out, then check the roster.
    fleet.run(Rig::frames(12.0), &mut walking);
    assert_eq!(fleet.server.players().count(), 16);
    assert!(fleet.clients.iter().all(|c| c.in_game()));

    println!(
        "endurance: {DURATION} s simulated in {:.0} s wall clock; RSS (s, KiB): {samples:?}",
        started.elapsed().as_secs_f64()
    );
    if let Some(base) = baseline {
        let max = samples.iter().map(|s| s.1).max().unwrap();
        let last = samples.last().unwrap().1;
        println!(
            "baseline {base} KiB at {WARM_UP} s, peak {max} KiB, final {last} KiB (growth {} KiB)",
            last as i64 - base as i64
        );
        assert!(
            max <= base + MAX_GROWTH_KIB && last <= base + MAX_GROWTH_KIB,
            "memory grew from {base} KiB to {last} KiB (peak {max})"
        );
    }
}

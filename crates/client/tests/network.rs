//! Lot 1.10 network integration: one server with 16 players (4 predicting
//! headless clients and 12 bots) in one process, every link at 100 ms latency,
//! 20 ms jitter and 5 % loss. Section 20 criteria: mean prediction error
//! below 5 cm, downlink below 100 kbit/s per client, no lasting divergence.

mod common;

use std::sync::atomic::Ordering;

use common::{walk, Fleet, Movement, Rig, IP_UDP_HEADER};
use faset_core::Tick;
use faset_net::NetworkConditions;

const HUMANS: usize = 4;
const BOTS: u32 = 12;

/// The shared walk, shifted per client so the clients differ.
fn walking(client: usize, tick: Tick) -> Movement {
    walk(Tick(tick.0 + 20 * client as u32))
}

#[test]
fn sixteen_players_on_a_degraded_network_meet_the_criteria() {
    let conditions = NetworkConditions {
        seed: 10,
        ..NetworkConditions::typical()
    };
    let mut fleet = Fleet::new(HUMANS, BOTS, conditions);
    fleet.join();
    assert_eq!(fleet.server.players().count(), 16);

    // Settle for 5 s, then measure 30 s.
    fleet.run(Rig::frames(5.0), &mut walking);
    for c in &mut fleet.clients {
        c.outcomes.clear();
    }
    let start = (
        fleet.bytes.load(Ordering::Relaxed),
        fleet.packets.load(Ordering::Relaxed),
    );
    let seconds = 30.0f32;
    fleet.run(Rig::frames(seconds), &mut walking);
    let wire = fleet.bytes.load(Ordering::Relaxed) - start.0;
    let datagrams = fleet.packets.load(Ordering::Relaxed) - start.1;

    // Criterion 1: mean prediction error below 5 cm.
    let mut sum = 0.0f64;
    let mut n = 0usize;
    for (i, c) in fleet.clients.iter().enumerate() {
        let errors = &c.outcomes;
        assert!(errors.len() > 400, "client {i}: {} snapshots", errors.len());
        let mean = errors
            .iter()
            .map(|o| f64::from(o.position_error))
            .sum::<f64>()
            / errors.len() as f64;
        assert!(mean < 0.05, "client {i}: mean prediction error {mean} m");
        sum += errors
            .iter()
            .map(|o| f64::from(o.position_error))
            .sum::<f64>();
        n += errors.len();
    }
    let mean_error = sum / n as f64;

    // Criterion 2: downlink per client below 100 kbit/s.
    let per_client =
        (wire + IP_UDP_HEADER * datagrams) as f64 * 8.0 / f64::from(seconds) / HUMANS as f64;
    let kbit = per_client / 1000.0;

    // Criterion 3: no lasting divergence. Everyone stops; the network stays
    // degraded; after 10 s each client's prediction matches the server.
    fleet.run(Rig::frames(10.0), &mut |_, _| (0, 0, 0));
    let mut worst = 0.0f32;
    for (i, c) in fleet.clients.iter().enumerate() {
        let id = c.player.unwrap();
        let server = fleet.server.world().position(id).unwrap();
        let predicted = c.prediction.as_ref().unwrap().state().unwrap().position;
        let displayed = c.prediction.as_ref().unwrap().display_position().unwrap();
        let divergence = (predicted - server).length();
        worst = worst.max(divergence).max((displayed - server).length());
        assert!(
            divergence < 0.05 && (displayed - server).length() < 0.05,
            "client {i} diverged by {divergence} m"
        );
    }
    println!(
        "16 players, 100 ms / 20 ms / 5 % loss, {seconds} s measured: mean prediction error \
         {:.4} m over {n} snapshots; downlink {kbit:.1} kbit/s per client including IP/UDP \
         headers; worst divergence after 10 s at rest {worst:.4} m",
        mean_error
    );
    assert!(mean_error < 0.05);
    assert!(kbit < 100.0, "downlink {kbit:.1} kbit/s per client");
}

//! Lot 1.6 acceptance: a headless predicting client against the server
//! through the conditioned transport (100 ms latency, 20 ms jitter, 5 % loss
//! in each direction).

mod common;

use common::{decay, walk, Movement, Rig, MAX_STEP};
use faset_client::{MAX_TIMESCALE, MIN_TIMESCALE};
use faset_core::Tick;
use faset_net::NetworkConditions;

fn degraded(seed: u64) -> NetworkConditions {
    NetworkConditions {
        seed,
        ..NetworkConditions::typical()
    }
}

#[test]
fn mean_prediction_error_stays_below_five_centimetres() {
    let mut rig = Rig::new(degraded(1));
    rig.join();
    rig.run(Rig::frames(20.0), &mut walk);
    let out = &rig.client.outcomes;
    assert!(out.len() > 400, "only {} snapshots compared", out.len());
    let mean = out.iter().map(|o| o.position_error).sum::<f32>() / out.len() as f32;
    let corrected = out.iter().filter(|o| o.corrected).count();
    let worst = out.iter().map(|o| o.position_error).fold(0.0, f32::max);
    println!(
        "compared {} snapshots: mean error {:.4} m, worst {worst:.3} m, {corrected} corrections; \
         rtt {:.3} s, jitter {:.4} s, lead {:.2} (target {:.2}) ticks; late {}/{}",
        out.len(),
        mean,
        rig.client.clock.rtt(),
        rig.client.clock.jitter(),
        rig.client.clock.measured_lead().unwrap(),
        rig.client.clock.target_lead(),
        rig.client.late,
        rig.client.snapshots,
    );
    assert!(mean < 0.05, "mean prediction error {mean} m");
}

#[test]
fn speed_factor_stays_within_five_percent_and_no_tick_is_skipped() {
    let mut rig = Rig::new(degraded(2));
    rig.join();
    rig.run(Rig::frames(20.0), &mut walk);
    let c = &rig.client;
    for f in &c.frames {
        assert!(
            (MIN_TIMESCALE..=MAX_TIMESCALE).contains(&f.timescale),
            "speed factor {}",
            f.timescale
        );
    }
    assert!(c.ticks.len() > 1100, "{} ticks", c.ticks.len());
    for w in c.ticks.windows(2) {
        assert_eq!(w[1], w[0].next(), "tick skipped or repeated");
    }
    // The speed is actually modulated, and the clock does its job: commands
    // arrive ahead of the server and are almost never late.
    assert!(c.frames.iter().any(|f| f.timescale != 1.0));
    let lead = c.clock.measured_lead().unwrap();
    assert!(lead > 1.0, "lead {lead} ticks");
    assert!(
        f64::from(c.late) < 0.01 * f64::from(c.snapshots),
        "{} of {} snapshots had a late or lost command",
        c.late,
        c.snapshots
    );
}

#[test]
fn forced_correction_decays_without_jumps() {
    let mut rig = Rig::new(degraded(3));
    rig.join();
    let forward: Movement = (0, 127, 0);
    rig.run(Rig::frames(2.0), &mut |_| forward);

    // Cut the client's uplink for 12 ticks while it turns left: the server
    // keeps repeating the last forward command it got.
    let turn = rig.client.clock.next_tick();
    let mut input = move |t: Tick| if t < turn { forward } else { (-127, 0, 0) };
    let normal = rig.client.link_mut().conditions().clone();
    rig.client.link_mut().set_conditions(NetworkConditions {
        loss_percent: 100.0,
        ..normal.clone()
    });
    rig.run(48, &mut input);
    rig.client.link_mut().set_conditions(normal);
    let from = rig.client.frames.len();
    rig.run(Rig::frames(3.0), &mut input);

    let frames = &rig.client.frames[from - 48..];
    let biggest = frames.iter().map(|f| f.offset.length()).fold(0.0, f32::max);
    assert!(biggest > 0.3, "no forced correction (offset {biggest} m)");
    assert!(frames.iter().any(|f| f.corrected));
    assert!(frames.iter().all(|f| !f.snapped), "a correction snapped");

    let mut last_correction = 0;
    for (i, w) in frames.windows(2).enumerate() {
        let (a, b) = (w[0], w[1]);
        if b.corrected {
            last_correction = i + 1;
        }
        // The display moves by the walk plus the decayed part of the offset
        // (the correction itself is absorbed by the offset): no teleport.
        let step = (b.display.unwrap() - a.display.unwrap()).length();
        let bound = MAX_STEP * b.ticks as f32
            + b.offset_before_ticks.length() * (1.0 - decay(b.ticks))
            + 1e-3;
        assert!(
            step <= bound,
            "frame {i}: display jumped {step} m (bound {bound})"
        );
        // Between corrections the offset only shrinks, by e^{-Δt/τ} a tick.
        if !b.corrected {
            assert!(b.offset.length() <= a.offset.length() * decay(b.ticks) + 1e-5);
        }
    }
    // A second after the last correction the offset is gone.
    let settled = &frames[last_correction..];
    assert!(settled.len() > 240, "corrections went on until the end");
    assert!(
        settled[240].offset.length() < 1e-3,
        "{}",
        settled[240].offset
    );
}

#[test]
fn quiet_period_converges_to_the_server_state() {
    let mut rig = Rig::new(degraded(4));
    rig.join();
    rig.run(Rig::frames(5.0), &mut walk);
    rig.run(Rig::frames(3.0), &mut |_| (0, 0, 0));

    let c = &rig.client;
    let p = c.prediction.as_ref().unwrap();
    let tolerance = p.tolerance();
    let recent = &c.outcomes[c.outcomes.len() - 30..];
    for o in recent {
        assert!(!o.corrected, "still correcting at {:?}", o.tick);
        assert!(o.position_error <= tolerance.position && o.velocity_error <= tolerance.velocity);
    }
    let me = c.player.unwrap();
    let server = rig.server.world().position(me).unwrap();
    let client = p.state().unwrap();
    assert!(
        (client.position - server).length() <= tolerance.position,
        "client {} server {server}",
        client.position
    );
    assert!(client.velocity.length() <= tolerance.velocity);
    assert!(p.visual_offset().value().length() < 1e-3);
    assert!((p.display_position().unwrap() - server).length() <= tolerance.position);
}

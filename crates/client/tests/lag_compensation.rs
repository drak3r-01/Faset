//! Lot 1.7 acceptance: a client shows a moving bot through the
//! interpolation buffer with 100 ms of round trip (50 ms each way), shoots
//! at the displayed position, and the server's rewound cast registers the
//! hit, where an uncompensated cast misses.

mod common;

use std::f32::consts::{FRAC_PI_2, PI};
use std::time::Duration;

use common::Rig;
use faset_client::{SampleMode, ViewTime};
use faset_core::{NetId, Tick};
use faset_net::NetworkConditions;
use faset_protocol::quant::quantize_yaw;
use faset_protocol::InputCommand;
use faset_server::{Bot, FireCommand, Hitbox};
use faset_sim::World;

/// Server tick at which the target turns from -X to +Z.
const TURN: u32 = 120;

/// Walks -X at the 5 m/s stub speed for two seconds, then +Z: across the
/// line of sight of a shooter standing near the origin.
struct Target;

impl Bot for Target {
    fn command(&mut self, _: &World, _: NetId, tick: Tick) -> InputCommand {
        // Yaw pi/2 faces -X, pi faces +Z.
        let yaw = if tick.0 < TURN { FRAC_PI_2 } else { PI };
        InputCommand {
            tick,
            buttons: 0,
            move_x: 0,
            move_y: 127,
            yaw: quantize_yaw(yaw),
            pitch: 0,
            weapon_slot: 0,
        }
    }
}

#[test]
fn a_shot_at_the_displayed_target_hits_with_100_ms_latency() {
    let mut rig = Rig::new(NetworkConditions {
        latency: Duration::from_millis(50),
        ..NetworkConditions::default()
    });
    let target = rig.server.add_bot(Box::new(Target)).unwrap();
    rig.join();
    let shooter = rig.client.player.unwrap();
    let mut still = |_| (0, 0, 0);
    // Well past the turn: the target moves at constant speed.
    while rig.server.world().tick().0 < TURN + 60 {
        rig.run(1, &mut still);
    }

    // Client side: aim at the centre of the displayed target.
    let c = &rig.client;
    let render = c.remotes.render_time(c.now, c.clock.jitter()).unwrap();
    let seen = c.remotes.sample(target, render).unwrap();
    assert_eq!(seen.mode, SampleMode::Interpolated);
    let me = c.prediction.as_ref().unwrap().display_position().unwrap();
    let eye = Hitbox::standing(me).eye();
    let view = ViewTime::from_ticks(render);
    let fire = FireCommand {
        view_tick: view.tick,
        view_fraction: view.fraction,
        direction: seen.position - eye,
        range: 100.0,
    };

    // The command reaches the server one way later.
    rig.run(Rig::frames(0.05), &mut still);
    let now = rig.server.world().tick();
    let rtt = rig.server.player_rtt(shooter).unwrap();
    let out = rig.server.rewind_and_cast(shooter, &fire).unwrap();
    let age = (f64::from(now.0) - out.rewound_to) / 60.0;
    let truth = rig.server.world().position(target).unwrap();
    println!(
        "server rtt {rtt:.3} s, view {:.2} rewound {:.2} ({age:.3} s back), clamped {}, \
         seen {} now {truth}",
        view.ticks(),
        out.rewound_to,
        out.clamped,
        seen.position,
    );
    let hit = out.hit.expect("the shot at the displayed target must hit");
    assert_eq!(hit.target, target);
    assert!(age > 0.1 && age <= 0.25, "rewound {age} s");

    // Restore: the history's placed hitboxes are the authoritative ones.
    let lag = rig.server.lag_compensation();
    for id in [shooter, target] {
        assert_eq!(lag.hitbox(id), lag.authoritative(id));
        assert_eq!(
            lag.hitbox(id).unwrap().position,
            rig.server.world().position(id).unwrap()
        );
    }

    // Without compensation, the same aim misses the target.
    let unrewound = FireCommand {
        view_tick: now,
        view_fraction: 0.0,
        ..fire
    };
    let out = rig.server.rewind_and_cast(shooter, &unrewound).unwrap();
    assert!(out.hit.is_none(), "{:?}", out.hit);
}

//! Lot 4.2 acceptance: the character controller on small test courses.

use faset_core::glam::Vec3;
use faset_core::{NetId, Tick, DT};
use faset_protocol::quant::quantize_yaw;
use faset_protocol::InputCommand;
use faset_sim::collision::player_capsule;
use faset_sim::controller::{
    step_player_in, MoveCmd, MoveEvents, MoveParams, PlayerState, BUTTON_CROUCH, BUTTON_JUMP,
    CROUCH_EYE_HEIGHT, SKIN,
};
use faset_sim::trace::Outcome;
use faset_sim::{StaticWorld, Trace, Volume};
use parry3d::math::{Pose, Vector};
use parry3d::query;
use parry3d::shape::SharedShape;
use proptest::prelude::*;

const FORWARD: MoveCmd = MoveCmd {
    yaw: 0,
    pitch: 0,
    move_x: 0,
    move_y: 127,
    buttons: 0,
};
const IDLE: MoveCmd = MoveCmd {
    yaw: 0,
    pitch: 0,
    move_x: 0,
    move_y: 0,
    buttons: 0,
};

/// Yaw facing +X.
const FACE_X: u16 = 49152;

fn floor() -> Volume {
    Volume::cuboid(Vec3::new(0.0, -0.5, 0.0), Vec3::new(100.0, 0.5, 100.0))
}

fn boxed(min: Vec3, max: Vec3) -> Volume {
    Volume::cuboid((min + max) * 0.5, (max - min) * 0.5)
}

/// Wedge rising along +X from `x0` at `deg` degrees, `len` long, z in -5..5.
fn ramp(x0: f32, len: f32, deg: f32) -> Volume {
    let h = len * deg.to_radians().tan();
    let x1 = x0 + len;
    let pts: Vec<Vector> = [
        (x0, 0.0, -5.0),
        (x0, 0.0, 5.0),
        (x1, 0.0, -5.0),
        (x1, 0.0, 5.0),
        (x1, h, -5.0),
        (x1, h, 5.0),
    ]
    .iter()
    .map(|&(x, y, z)| Vector::new(x, y, z))
    .collect();
    Volume {
        shape: SharedShape::convex_hull(&pts).unwrap(),
        pose: Pose::IDENTITY,
    }
}

fn world(extra: Vec<Volume>) -> StaticWorld {
    let mut v = vec![floor()];
    v.extend(extra);
    StaticWorld::new(v)
}

/// Capsule centre for feet at `feet` (plus 1 cm).
fn stand(feet: Vec3) -> Vec3 {
    feet + Vec3::Y * 0.91
}

fn step(s: &mut PlayerState, cmd: MoveCmd, w: &StaticWorld, p: &MoveParams) -> MoveEvents {
    step_player_in(s, cmd, w, p, DT)
}

/// A grounded player standing at `feet`.
fn grounded(feet: Vec3, w: &StaticWorld, p: &MoveParams) -> PlayerState {
    let mut s = PlayerState::spawn(stand(feet));
    for _ in 0..10 {
        step(&mut s, IDLE, w, p);
    }
    assert!(s.on_ground, "{s:?}");
    s
}

fn hspeed(v: Vec3) -> f32 {
    (v.x * v.x + v.z * v.z).sqrt()
}

#[test]
fn jump_height_air_time_and_range_match_the_plan() {
    let p = MoveParams {
        auto_hop: false,
        ..MoveParams::default()
    };
    let w = world(vec![]);
    let mut s = grounded(Vec3::ZERO, &w, &p);
    let base = s.feet().y;
    assert!(base > -EPS && base <= 2.0 * SKIN + 1e-4, "feet {base}");
    let jump = MoveCmd {
        buttons: BUTTON_JUMP,
        ..IDLE
    };
    assert!(step(&mut s, jump, &w, &p).jumped);
    let mut top: f32 = s.feet().y;
    let mut ticks = 1;
    while !step(&mut s, IDLE, &w, &p).landed {
        top = top.max(s.feet().y);
        ticks += 1;
        assert!(ticks < 100);
    }
    assert!(
        (top - base - 1.056).abs() < 0.01,
        "jump height {}",
        top - base
    );
    assert!((38..=40).contains(&ticks), "air time {ticks} ticks");

    // Run-speed range.
    let mut s = grounded(Vec3::ZERO, &w, &p);
    for _ in 0..120 {
        step(&mut s, FORWARD, &w, &p);
    }
    assert!((hspeed(s.vel) - 7.0).abs() < 1e-3, "run speed {}", s.vel);
    let start = s.pos;
    assert!(
        step(
            &mut s,
            MoveCmd {
                buttons: BUTTON_JUMP,
                ..FORWARD
            },
            &w,
            &p
        )
        .jumped
    );
    while !step(&mut s, FORWARD, &w, &p).landed {}
    let range = (s.pos - start).length();
    assert!((range - 4.55).abs() < 0.15, "range {range}");
}

#[test]
fn ground_test_rejects_surfaces_steeper_than_45_degrees() {
    let p = MoveParams::default();
    for (deg, walkable) in [(30.0, true), (44.0, true), (46.0, false), (60.0, false)] {
        let w = world(vec![ramp(0.0, 10.0, deg)]);
        // Drop the player above the middle of the ramp.
        let x = 6.0_f32;
        let surface = x * f32::to_radians(deg).tan();
        let mut s = PlayerState::spawn(Vec3::new(x, surface + 2.0, 0.0));
        let mut grounded_ticks = 0;
        for _ in 0..40 {
            step(&mut s, IDLE, &w, &p);
            grounded_ticks += u32::from(s.on_ground);
        }
        if walkable {
            assert!(s.on_ground, "{deg}°: {s:?}");
            let rest = s.pos;
            for _ in 0..60 {
                step(&mut s, IDLE, &w, &p);
                assert!(s.on_ground, "{deg}°");
            }
            assert!(
                (s.pos - rest).length() < 2e-3,
                "{deg}°: crept {}",
                s.pos - rest
            );
        } else {
            assert_eq!(grounded_ticks, 0, "{deg}° counted as ground");
            assert!(s.pos.x < x - 0.5, "{deg}°: did not slide down ({})", s.pos);
        }
    }
}

#[test]
fn steps_up_to_45_cm_are_climbed() {
    let p = MoveParams::default();
    for (h, climbs) in [(0.2, true), (0.4, true), (0.5, false), (0.8, false)] {
        let w = world(vec![boxed(
            Vec3::new(2.0, 0.0, -5.0),
            Vec3::new(30.0, h, 5.0),
        )]);
        let mut s = grounded(Vec3::ZERO, &w, &p);
        let run = MoveCmd {
            yaw: FACE_X,
            ..FORWARD
        };
        for _ in 0..60 {
            step(&mut s, run, &w, &p);
        }
        if climbs {
            assert!(s.pos.x > 3.0, "{h} m step not climbed: {}", s.pos);
            assert!(
                (s.feet().y - h - SKIN).abs() < 2e-3,
                "{h}: feet {}",
                s.feet()
            );
            assert!(s.on_ground);
        } else {
            assert!(s.pos.x < 2.0 - 0.35 + 1e-3, "{h} m wall crossed: {}", s.pos);
            assert!(s.feet().y < 0.01, "{h}: feet {}", s.feet());
        }
    }
}

#[test]
fn walking_down_a_ramp_snaps_to_it() {
    let p = MoveParams::default();
    let deg = 25.0_f32;
    let top = 6.0 * deg.to_radians().tan();
    let w = world(vec![
        ramp(2.0, 6.0, deg),
        boxed(Vec3::new(8.0, 0.0, -5.0), Vec3::new(14.0, top, 5.0)),
    ]);
    let mut s = grounded(Vec3::new(12.0, top, 0.0), &w, &p);
    let down = MoveCmd {
        yaw: 16384, // faces -X
        ..FORWARD
    };
    let mut ticks = 0;
    while s.pos.x > 0.0 {
        let ev = step(&mut s, down, &w, &p);
        assert!(s.on_ground && !ev.landed, "left the ground at {}", s.pos);
        ticks += 1;
        assert!(ticks < 300);
    }
    assert!(s.feet().y < 0.01);
}

/// Falls from 1.5 m; returns the landing tick and the first jump tick.
fn drop_and_press(press: Option<u32>, p: &MoveParams) -> (u32, Option<u32>) {
    let w = world(vec![]);
    let mut s = PlayerState::spawn(stand(Vec3::Y * 1.5));
    let (mut landed, mut jumped) = (None, None);
    for t in 0..120 {
        let cmd = MoveCmd {
            buttons: if press == Some(t) { BUTTON_JUMP } else { 0 },
            ..IDLE
        };
        let ev = step(&mut s, cmd, &w, p);
        if ev.landed && landed.is_none() {
            landed = Some(t);
        }
        if ev.jumped && jumped.is_none() {
            jumped = Some(t);
        }
    }
    (landed.unwrap(), jumped)
}

#[test]
fn jump_buffer_keeps_a_press_for_100_ms() {
    let p = MoveParams {
        auto_hop: false,
        ..MoveParams::default()
    };
    let (land, none) = drop_and_press(None, &p);
    assert_eq!(none, None);
    // Pressed 4 ticks before landing: fires on the first grounded tick,
    // 5 ticks (83 ms) after the press.
    assert_eq!(drop_and_press(Some(land - 4), &p).1, Some(land + 1));
    // 6 ticks (100 ms) after the press is too late.
    assert_eq!(drop_and_press(Some(land - 6), &p).1, None);
}

/// Runs off a 2 m ledge and presses jump `delay` ticks after leaving it.
fn ledge_jump(delay: u32) -> bool {
    let p = MoveParams::default();
    let w = world(vec![boxed(
        Vec3::new(-20.0, 0.0, -5.0),
        Vec3::new(0.0, 2.0, 5.0),
    )]);
    let mut s = grounded(Vec3::new(-3.0, 2.0, 0.0), &w, &p);
    let run = MoveCmd {
        yaw: FACE_X,
        ..FORWARD
    };
    let mut left = None;
    for t in 0..120 {
        let cmd = MoveCmd {
            buttons: if left.is_some_and(|l| t == l + delay) {
                BUTTON_JUMP
            } else {
                0
            },
            ..run
        };
        let was = s.on_ground;
        let ev = step(&mut s, cmd, &w, &p);
        if ev.jumped {
            assert!(s.vel.y > 0.0);
            return true;
        }
        if was && !s.on_ground && left.is_none() {
            left = Some(t);
        }
    }
    assert!(left.is_some());
    false
}

#[test]
fn coyote_time_allows_a_jump_80_ms_after_the_ledge() {
    assert!(ledge_jump(1));
    assert!(ledge_jump(4));
    assert!(!ledge_jump(6));
}

#[test]
fn auto_hop_is_a_switchable_rule() {
    let w = world(vec![]);
    let hold = MoveCmd {
        buttons: BUTTON_JUMP,
        ..IDLE
    };
    for (auto, expected) in [(true, 3), (false, 1)] {
        let p = MoveParams {
            auto_hop: auto,
            ..MoveParams::default()
        };
        let mut s = grounded(Vec3::ZERO, &w, &p);
        let jumps = (0..120)
            .filter(|_| step(&mut s, hold, &w, &p).jumped)
            .count();
        assert!(
            if auto {
                jumps >= expected
            } else {
                jumps == expected
            },
            "auto-hop {auto}: {jumps} jumps"
        );
    }
}

/// Strafe-jumping bot: yaw keeps the strafe direction at the angle where
/// `p = w_air − s` (largest gain). `lateral` false holds forward instead.
fn bhop_command(s: &PlayerState, tick: u32, lateral: bool, p: &MoveParams) -> MoveCmd {
    if tick < 60 {
        return FORWARD;
    }
    let speed = hspeed(s.vel);
    let gain = p.air_accel * p.run_speed * DT;
    let phi = ((p.air_wish_cap - gain) / speed.max(1e-3))
        .clamp(-1.0, 1.0)
        .acos();
    let a = s.vel.z.atan2(s.vel.x);
    MoveCmd {
        yaw: quantize_yaw(-(a + phi)),
        pitch: 0,
        move_x: if lateral { 127 } else { 0 },
        move_y: if lateral { 0 } else { 127 },
        buttons: BUTTON_JUMP,
    }
}

struct Bhop {
    commands: Vec<MoveCmd>,
    /// Horizontal speed at the first jump.
    start_speed: f32,
    /// Highest horizontal speed after `jumps` jumps.
    best: Vec<f32>,
    end: PlayerState,
}

fn bhop(lateral: bool, ticks: u32) -> Bhop {
    let p = MoveParams::default();
    let w = world(vec![]);
    let mut s = grounded(Vec3::ZERO, &w, &p);
    let mut out = Bhop {
        commands: Vec::new(),
        start_speed: 0.0,
        best: Vec::new(),
        end: s,
    };
    for t in 0..ticks {
        let cmd = bhop_command(&s, t, lateral, &p);
        let before = hspeed(s.vel);
        let ev = step(&mut s, cmd, &w, &p);
        out.commands.push(cmd);
        if ev.jumped {
            if out.best.is_empty() {
                out.start_speed = before;
            }
            out.best.push(hspeed(s.vel));
        }
        if let Some(b) = out.best.last_mut() {
            *b = b.max(hspeed(s.vel));
        }
    }
    out.end = s;
    out
}

#[test]
fn strafe_jumping_exceeds_one_and_a_half_run_speed_within_ten_jumps() {
    let run = bhop(true, 60 + 10 * 40);
    assert!((run.start_speed - 7.0).abs() < 0.05, "{}", run.start_speed);
    let first = run.best.iter().position(|&b| b > 10.5);
    println!("speed per jump: {:?}", run.best);
    assert!(first.is_some_and(|j| j < 10), "best speeds {:?}", run.best);
    // About +1.4 m/s on the first jump at run speed (plan figure).
    let gain = run.best[0] - run.start_speed;
    assert!((1.0..1.8).contains(&gain), "first jump gain {gain}");
}

#[test]
fn the_same_script_without_lateral_input_gains_nothing() {
    // Same bot, forward held instead of the strafe key.
    let run = bhop(false, 60 + 10 * 40);
    assert!(run.best.len() >= 10, "{:?}", run.best);
    let top = run.best.iter().copied().fold(0.0, f32::max);
    assert!(
        top <= run.start_speed + 1e-3,
        "gained {} m/s",
        top - run.start_speed
    );
    // No move input at all while hopping: speed is kept, never gained.
    let p = MoveParams::default();
    let w = world(vec![]);
    let mut s = grounded(Vec3::ZERO, &w, &p);
    for _ in 0..60 {
        step(&mut s, FORWARD, &w, &p);
    }
    let start = hspeed(s.vel);
    let hop = MoveCmd {
        buttons: BUTTON_JUMP,
        ..IDLE
    };
    let mut jumps = 0;
    for _ in 0..400 {
        jumps += u32::from(step(&mut s, hop, &w, &p).jumped);
        assert!(hspeed(s.vel) <= start + 1e-4, "{} > {start}", hspeed(s.vel));
    }
    assert!(jumps >= 10);
    assert!(
        (hspeed(s.vel) - start).abs() < 1e-4,
        "speed lost between jumps"
    );
}

#[test]
fn landing_above_15_m_s_brakes_softly() {
    let p = MoveParams::default();
    let w = world(vec![]);
    let mut s = PlayerState::spawn(stand(Vec3::Y * 0.2));
    s.vel = Vec3::new(20.0, 0.0, 0.0);
    loop {
        let before = hspeed(s.vel);
        if step(&mut s, IDLE, &w, &p).landed {
            let expect = before - (before - 15.0) * 0.25;
            assert!((hspeed(s.vel) - expect).abs() < 1e-3, "{}", hspeed(s.vel));
            break;
        }
    }
}

#[test]
fn crouching_shrinks_the_capsule_and_low_ceilings_keep_it() {
    let p = MoveParams::default();
    let w = world(vec![boxed(
        Vec3::new(2.0, 1.3, -5.0),
        Vec3::new(6.0, 3.0, 5.0),
    )]);
    let mut s = grounded(Vec3::ZERO, &w, &p);
    let feet = s.feet();
    let crouch_run = MoveCmd {
        yaw: FACE_X,
        buttons: BUTTON_CROUCH,
        ..FORWARD
    };
    step(&mut s, crouch_run, &w, &p);
    assert!(s.crouched);
    assert!(
        (s.feet().y - feet.y).abs() < 1e-5,
        "feet stay on the ground"
    );
    for _ in 0..60 {
        step(&mut s, crouch_run, &w, &p);
    }
    assert!((s.eye_height - CROUCH_EYE_HEIGHT).abs() < 1e-5);
    assert!(
        s.pos.x > 2.5 && s.pos.x < 5.5,
        "under the ceiling: {}",
        s.pos
    );
    // Crouched ground speed is half the run speed.
    assert!((hspeed(s.vel) - 3.5).abs() < 0.05, "{}", hspeed(s.vel));
    // Releasing crouch under the ceiling keeps the player crouched.
    let stand_up = MoveCmd {
        yaw: FACE_X,
        ..IDLE
    };
    step(&mut s, stand_up, &w, &p);
    assert!(s.crouched);
    // Out in the open the player stands up.
    let run = MoveCmd {
        yaw: FACE_X,
        ..FORWARD
    };
    for _ in 0..90 {
        step(&mut s, run, &w, &p);
    }
    assert!(s.pos.x > 6.5 && !s.crouched, "{s:?}");
    assert!((s.feet().y - feet.y).abs() < 2e-3);
}

#[test]
fn free_fall_is_capped_at_30_m_s() {
    let p = MoveParams::default();
    let w = world(vec![]);
    let mut s = PlayerState::spawn(Vec3::new(0.0, 200.0, 0.0));
    let mut fastest: f32 = 0.0;
    while !step(&mut s, IDLE, &w, &p).landed {
        fastest = fastest.max(s.vel.length());
    }
    assert!((fastest - 30.0).abs() < 1e-3, "{fastest}");
    assert!(s.feet().y > 0.0 && s.feet().y < 0.01);
}

/// Penetration tolerated: parry sweeps near the large boxes of a map are only
/// accurate to about a millimetre, and a sweep grazing a surface the player
/// already touches can report no hit within that tolerance.
const EPS: f32 = 5e-3;

/// Deepest penetration of the player capsule into any volume, metres.
fn penetration(w: &StaticWorld, s: &PlayerState) -> f32 {
    let cap = player_capsule(s.height());
    let pose = Pose::translation(s.pos.x, s.pos.y, s.pos.z);
    w.volumes()
        .iter()
        .map(|v| {
            query::contact(&v.pose, v.shape.as_ref(), &pose, &cap, 0.0)
                .unwrap()
                .map_or(0.0, |c| -c.dist)
        })
        .fold(0.0, f32::max)
}

/// Floor, a 2 cm wall at x = 3 and a 2 cm slab at y = 10.
fn thin_world() -> StaticWorld {
    world(vec![
        boxed(Vec3::new(3.0, 0.0, -100.0), Vec3::new(3.02, 20.0, 100.0)),
        boxed(
            Vec3::new(-100.0, 10.0, -100.0),
            Vec3::new(2.0, 10.02, 100.0),
        ),
    ])
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn no_wall_crossing_at_max_speed(
        angle in -1.4f32..1.4,
        rise in -0.5f32..0.5,
        z in -20.0f32..20.0,
        jump in any::<bool>(),
        above in any::<bool>(),
    ) {
        let p = MoveParams::default();
        let w = thin_world();
        let start = if above { Vec3::new(0.0, 10.95, z) } else { Vec3::new(0.0, 0.95, z) };
        let mut s = PlayerState::spawn(start);
        let dir = Vec3::new(angle.cos(), rise, angle.sin()).normalize();
        s.vel = dir * 30.0;
        let cmd = MoveCmd {
            yaw: quantize_yaw(-angle - std::f32::consts::FRAC_PI_2),
            buttons: if jump { BUTTON_JUMP } else { 0 },
            ..FORWARD
        };
        for _ in 0..90 {
            step(&mut s, cmd, &w, &p);
            prop_assert!(s.pos.x < 3.0 - 0.35 + EPS, "crossed the wall: {}", s.pos);
            if above && s.pos.x < 2.0 - 0.35 {
                prop_assert!(s.feet().y > 10.0, "fell through the slab: {}", s.pos);
            }
        }
    }

    #[test]
    fn speed_never_exceeds_30_m_s(
        vel in (-200.0f32..200.0, -200.0f32..200.0, -200.0f32..200.0),
        cmds in prop::collection::vec((any::<u16>(), any::<i8>(), any::<i8>(), 0u16..4), 1..120),
    ) {
        let p = MoveParams::default();
        let w = world(vec![
            ramp(2.0, 6.0, 30.0),
            boxed(Vec3::new(-6.0, 0.0, -2.0), Vec3::new(-4.0, 0.4, 2.0)),
            boxed(Vec3::new(-20.0, 0.0, 8.0), Vec3::new(20.0, 4.0, 9.0)),
        ]);
        let mut s = PlayerState::spawn(stand(Vec3::Y * 3.0));
        s.vel = Vec3::new(vel.0, vel.1, vel.2);
        for (yaw, mx, my, buttons) in cmds {
            let cmd = MoveCmd { yaw, pitch: 0, move_x: mx, move_y: my, buttons };
            step(&mut s, cmd, &w, &p);
            prop_assert!(s.vel.length() <= 30.0 + 1e-3, "{}", s.vel.length());
            let pen = penetration(&w, &s);
            prop_assert!(pen <= EPS, "{pen} m into a volume at {}", s.pos);
        }
    }
}

// --- Recorded traces ------------------------------------------------------

const COURSE_TRACE: &str = include_str!("data/controller_course.trace");
const BHOP_TRACE: &str = include_str!("data/controller_bhop.trace");

/// Course of the recorded traces: steps, ramps, a wall, a ledge, a ceiling.
fn course() -> StaticWorld {
    world(vec![
        boxed(Vec3::new(4.0, 0.0, -10.0), Vec3::new(8.0, 0.3, 10.0)),
        boxed(Vec3::new(8.0, 0.0, -10.0), Vec3::new(12.0, 0.6, 10.0)),
        ramp(-12.0, 8.0, 30.0),
        ramp(-12.0, 4.0, 55.0),
        boxed(Vec3::new(-4.0, 0.0, -10.0), Vec3::new(0.0, 1.5, -6.0)),
        boxed(Vec3::new(-30.0, 0.0, 20.0), Vec3::new(30.0, 5.0, 21.0)),
        boxed(Vec3::new(12.0, 1.3, -10.0), Vec3::new(16.0, 3.0, 10.0)),
    ])
}

fn command(tick: u32, c: MoveCmd) -> InputCommand {
    InputCommand {
        tick: Tick(tick),
        buttons: c.buttons,
        move_x: c.move_x,
        move_y: c.move_y,
        yaw: c.yaw,
        pitch: c.pitch,
        weapon_slot: 0,
    }
}

/// Three players: one scripted across the course, two random.
fn course_trace() -> Trace {
    let mut random = Trace::generate(42, 1200, 3);
    random.players = vec![
        (NetId(1), stand(Vec3::new(0.0, 0.0, 0.0))),
        (NetId(2), stand(Vec3::new(2.0, 0.0, 5.0))),
        (NetId(3), stand(Vec3::new(-2.0, 0.0, 3.0))),
    ];
    for (t, frame) in random.frames.iter_mut().enumerate() {
        let t = t as u32;
        let script = match (t / 120) % 5 {
            0 => MoveCmd {
                yaw: FACE_X,
                ..FORWARD
            },
            1 => MoveCmd {
                yaw: FACE_X,
                buttons: BUTTON_CROUCH,
                ..FORWARD
            },
            2 => MoveCmd {
                yaw: 16384,
                buttons: if t.is_multiple_of(50) { BUTTON_JUMP } else { 0 },
                ..FORWARD
            },
            3 => MoveCmd {
                yaw: 16384,
                move_x: 90,
                ..FORWARD
            },
            _ => MoveCmd {
                yaw: (t * 300) as u16,
                buttons: BUTTON_JUMP,
                ..FORWARD
            },
        };
        frame[0].1 = command(t, script);
    }
    random
}

/// The strafe-jump bot run, preceded by the ten idle ticks of `grounded`.
fn bhop_trace() -> (Trace, PlayerState) {
    let run = bhop(true, 60 + 10 * 40);
    let idle = std::iter::repeat_n(IDLE, 10);
    let trace = Trace {
        seed: 9,
        players: vec![(NetId(1), stand(Vec3::ZERO))],
        frames: idle
            .chain(run.commands.iter().copied())
            .enumerate()
            .map(|(t, c)| vec![(NetId(1), command(t as u32, c))])
            .collect(),
    };
    (trace, run.end)
}

fn replay_course(t: &Trace) -> Outcome {
    t.replay_controller(&course(), MoveParams::default())
}

fn replay_flat(t: &Trace) -> Outcome {
    t.replay_controller(&world(vec![]), MoveParams::default())
}

#[test]
fn recorded_traces_replay_to_the_reference_positions() {
    let (course, expected) = Trace::from_text(COURSE_TRACE).unwrap();
    assert_eq!(course, course_trace(), "the course script changed");
    let first = replay_course(&course);
    assert_eq!(first, replay_course(&course), "two replays differ");
    assert_eq!(first, expected);

    let (hops, expected) = Trace::from_text(BHOP_TRACE).unwrap();
    let (script, bot_end) = bhop_trace();
    assert_eq!(hops, script, "the bot changed");
    let first = replay_flat(&hops);
    assert_eq!(first, replay_flat(&hops), "two replays differ");
    assert_eq!(first, expected);
    // The world replay is the bot's own controller run, bit for bit.
    assert_eq!(first.positions, vec![(NetId(1), bot_end.pos)]);
}

#[test]
fn world_replay_matches_a_direct_controller_run() {
    let p = MoveParams::default();
    let w = course();
    let t = course_trace();
    let mut states: Vec<PlayerState> = t
        .players
        .iter()
        .map(|(_, at)| PlayerState::spawn(*at))
        .collect();
    for f in &t.frames {
        for (i, (_, c)) in f.iter().enumerate() {
            step(&mut states[i], MoveCmd::from(*c), &w, &p);
        }
    }
    let out = replay_course(&t);
    for ((_, pos), s) in out.positions.iter().zip(&states) {
        assert_eq!(*pos, s.pos);
    }
}

/// Regenerates the reference files:
/// `cargo test -p faset-sim --test controller -- --ignored regenerate`.
#[test]
#[ignore]
fn regenerate_controller_traces() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data");
    let t = course_trace();
    std::fs::write(
        format!("{dir}/controller_course.trace"),
        t.to_text(&replay_course(&t)),
    )
    .unwrap();
    let (t, _) = bhop_trace();
    std::fs::write(
        format!("{dir}/controller_bhop.trace"),
        t.to_text(&replay_flat(&t)),
    )
    .unwrap();
}

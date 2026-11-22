//! Lot 4.3 acceptance: slide, slide-hop, wall jump and zones, each with a
//! recorded input sequence and its reference trace (`tests/traces/*.ron`).

use faset_core::glam::Vec3;
use faset_core::DT;
use faset_mapfmt::ZoneKind;
use faset_sim::controller::{
    step_player_in, MoveCmd, MoveEvents, MoveParams, PlayerState, BUTTON_CROUCH, BUTTON_JUMP,
};
use faset_sim::slide;
use faset_sim::zones::{ZoneTracker, ZoneVolume};
use faset_sim::{StaticWorld, Volume, ZoneEvent, Zones};
use parry3d::math::{Pose, Vector};
use parry3d::shape::SharedShape;
use serde::{Deserialize, Serialize};

/// Position tolerance, metres, and velocity tolerance, m/s.
const TOL: f32 = 1e-3;

/// Yaw facing +X.
const FACE_X: u16 = 49152;
/// Yaw facing -X.
const FACE_NEG_X: u16 = 16384;

const IDLE: MoveCmd = MoveCmd {
    yaw: FACE_X,
    pitch: 0,
    move_x: 0,
    move_y: 0,
    buttons: 0,
};
const RUN: MoveCmd = MoveCmd {
    move_y: 127,
    ..IDLE
};

fn with(c: MoveCmd, buttons: u16) -> MoveCmd {
    MoveCmd { buttons, ..c }
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

/// Capsule centre for feet at `feet` (plus 1 cm).
fn stand(feet: Vec3) -> Vec3 {
    feet + Vec3::Y * 0.91
}

fn hspeed(v: Vec3) -> f32 {
    (v.x * v.x + v.z * v.z).sqrt()
}

const TECHNIQUES: [&str; 7] = [
    "slide_entry",
    "slide_hop",
    "wall_jump",
    "jump_pad",
    "teleporter",
    "kill_volume",
    "low_gravity",
];

struct Scene {
    world: StaticWorld,
    zones: Zones,
    start: Vec3,
}

fn zone(min: Vec3, max: Vec3, kind: ZoneKind) -> Zones {
    Zones::new(vec![ZoneVolume { min, max, kind }])
}

fn scene(name: &str) -> Scene {
    let floor = boxed(
        Vec3::new(-100.0, -1.0, -100.0),
        Vec3::new(100.0, 0.0, 100.0),
    );
    let mut volumes = vec![floor];
    let mut start = stand(Vec3::ZERO);
    let pad = (Vec3::new(3.0, 0.0, -1.0), Vec3::new(4.0, 2.0, 1.0));
    let zones = match name {
        "wall_jump" => {
            volumes.push(boxed(
                Vec3::new(3.0, 0.0, -10.0),
                Vec3::new(4.0, 10.0, 10.0),
            ));
            start = stand(Vec3::new(-2.0, 0.0, 0.0));
            Zones::default()
        }
        "jump_pad" => zone(
            pad.0,
            Vec3::new(4.0, 0.5, 1.0),
            ZoneKind::JumpPad {
                velocity: [4.0, 10.0, 0.0],
            },
        ),
        "teleporter" => zone(
            pad.0,
            pad.1,
            ZoneKind::Teleporter {
                target: [-20.0, 0.0, 10.0],
                yaw: 90.0,
            },
        ),
        "kill_volume" => zone(pad.0, Vec3::new(5.0, 2.0, 1.0), ZoneKind::KillVolume),
        "low_gravity" => zone(
            Vec3::new(-10.0, 0.0, -10.0),
            Vec3::new(10.0, 30.0, 10.0),
            ZoneKind::LowGravity { scale: 0.25 },
        ),
        _ => Zones::default(),
    };
    Scene {
        world: StaticWorld::new(volumes),
        zones,
        start,
    }
}

/// One player in a scene: controller tick, then zones.
struct Sim<'a> {
    scene: &'a Scene,
    params: MoveParams,
    state: PlayerState,
    tracker: ZoneTracker,
}

impl<'a> Sim<'a> {
    fn new(scene: &'a Scene) -> Self {
        Self {
            scene,
            params: MoveParams::default(),
            state: PlayerState::spawn(scene.start),
            tracker: ZoneTracker::default(),
        }
    }

    fn tick(&mut self, cmd: MoveCmd) -> (MoveEvents, Vec<ZoneEvent>) {
        let ev = step_player_in(&mut self.state, cmd, &self.scene.world, &self.params, DT);
        let mut zev = Vec::new();
        self.scene
            .zones
            .apply(&mut self.state, &mut self.tracker, &self.params, &mut zev);
        (ev, zev)
    }
}

fn repeat(out: &mut Vec<MoveCmd>, cmd: MoveCmd, n: usize) {
    out.extend(std::iter::repeat_n(cmd, n));
}

/// The input sequence of a technique.
fn script(name: &str) -> Vec<MoveCmd> {
    let mut c = Vec::new();
    repeat(&mut c, IDLE, 10);
    match name {
        "slide_entry" => {
            repeat(&mut c, RUN, 60);
            repeat(&mut c, with(RUN, BUTTON_CROUCH), 120);
            repeat(&mut c, RUN, 30);
        }
        "slide_hop" => {
            repeat(&mut c, RUN, 60);
            repeat(&mut c, with(RUN, BUTTON_CROUCH), 10);
            repeat(&mut c, with(RUN, BUTTON_CROUCH | BUTTON_JUMP), 240);
        }
        "wall_jump" => {
            // Run, jump toward the wall, press jump on the first airborne
            // tick touching it.
            repeat(&mut c, RUN, 30);
            c.push(with(RUN, BUTTON_JUMP));
            let sc = scene(name);
            let mut sim = Sim::new(&sc);
            for &cmd in &c {
                sim.tick(cmd);
            }
            for _ in 0..60 {
                let touching = !sim.state.on_ground && sim.state.wall_normal != Vec3::ZERO;
                let cmd = if touching {
                    with(RUN, BUTTON_JUMP)
                } else {
                    RUN
                };
                c.push(cmd);
                sim.tick(cmd);
                if touching {
                    break;
                }
            }
            repeat(&mut c, RUN, 60);
        }
        "jump_pad" => repeat(&mut c, RUN, 120),
        "teleporter" => repeat(&mut c, RUN, 90),
        "kill_volume" => repeat(&mut c, RUN, 60),
        "low_gravity" => {
            c.push(with(IDLE, BUTTON_JUMP));
            repeat(&mut c, IDLE, 150);
        }
        _ => panic!("unknown technique {name}"),
    }
    c
}

/// Commands, run-length encoded in the trace files.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
struct Run {
    count: u32,
    yaw: u16,
    move_x: i8,
    move_y: i8,
    buttons: u16,
}

/// A recorded technique: inputs and per-tick position and velocity (after
/// the tick).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct TechniqueTrace {
    start: [f32; 3],
    inputs: Vec<Run>,
    ticks: Vec<([f32; 3], [f32; 3])>,
}

fn encode(cmds: &[MoveCmd]) -> Vec<Run> {
    let mut runs: Vec<Run> = Vec::new();
    for c in cmds {
        let run = Run {
            count: 1,
            yaw: c.yaw,
            move_x: c.move_x,
            move_y: c.move_y,
            buttons: c.buttons,
        };
        match runs.last_mut() {
            Some(r) if Run { count: 1, ..*r } == run => r.count += 1,
            _ => runs.push(run),
        }
    }
    runs
}

fn decode(runs: &[Run]) -> Vec<MoveCmd> {
    runs.iter()
        .flat_map(|r| {
            std::iter::repeat_n(
                MoveCmd {
                    yaw: r.yaw,
                    pitch: 0,
                    move_x: r.move_x,
                    move_y: r.move_y,
                    buttons: r.buttons,
                },
                r.count as usize,
            )
        })
        .collect()
}

fn record(name: &str) -> TechniqueTrace {
    let sc = scene(name);
    let cmds = script(name);
    let mut sim = Sim::new(&sc);
    let ticks = cmds
        .iter()
        .map(|&c| {
            sim.tick(c);
            (sim.state.pos.to_array(), sim.state.vel.to_array())
        })
        .collect();
    TechniqueTrace {
        start: sc.start.to_array(),
        inputs: encode(&cmds),
        ticks,
    }
}

fn trace_path(name: &str) -> String {
    format!("{}/tests/traces/{name}.ron", env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn every_technique_replays_its_reference_trace() {
    for name in TECHNIQUES {
        let text = std::fs::read_to_string(trace_path(name)).unwrap();
        let reference: TechniqueTrace = ron::from_str(&text).unwrap();
        let cmds = decode(&reference.inputs);
        assert_eq!(cmds, script(name), "{name}: the script changed");
        let sc = scene(name);
        assert_eq!(reference.start, sc.start.to_array(), "{name}");
        let mut sim = Sim::new(&sc);
        assert_eq!(cmds.len(), reference.ticks.len(), "{name}");
        for (t, (&c, (pos, vel))) in cmds.iter().zip(&reference.ticks).enumerate() {
            sim.tick(c);
            let dp = (sim.state.pos - Vec3::from_array(*pos)).length();
            let dv = (sim.state.vel - Vec3::from_array(*vel)).length();
            assert!(
                dp <= TOL && dv <= TOL,
                "{name} tick {t}: Δpos {dp} m, Δvel {dv} m/s"
            );
        }
    }
}

/// Runs a technique script, returning per tick the state before, the state
/// after and the events.
fn play(name: &str) -> Vec<(PlayerState, PlayerState, MoveEvents, Vec<ZoneEvent>)> {
    let sc = scene(name);
    let mut sim = Sim::new(&sc);
    script(name)
        .into_iter()
        .map(|c| {
            let before = sim.state;
            let (ev, zev) = sim.tick(c);
            (before, sim.state, ev, zev)
        })
        .collect()
}

#[test]
fn slide_entry_adds_the_impulse_then_ends() {
    let run = play("slide_entry");
    let entries: Vec<usize> = (0..run.len()).filter(|&t| run[t].2.slide_started).collect();
    assert_eq!(entries.len(), 1, "one slide: {entries:?}");
    let t = entries[0];
    let (before, after, ..) = &run[t];
    assert!(hspeed(before.vel) >= 5.0);
    // Impulse minus one tick of slide friction (0.8/s); the crouch tick also
    // grazes the floor (sweep tolerance), hence 1 cm/s.
    let expect = (hspeed(before.vel) + 2.5) * (1.0 - 0.8 * DT);
    assert!(
        (hspeed(after.vel) - expect).abs() < 1e-2,
        "{} -> {} (expected {expect})",
        hspeed(before.vel),
        hspeed(after.vel)
    );
    assert!(after.crouched && (after.height() - 1.0).abs() < 1e-6);
    // Ends after 1.5 s at most (crouch is held for 2 s).
    let ended = (t..run.len())
        .find(|&i| !slide::is_sliding(&run[i].1))
        .unwrap();
    assert!(ended - t <= 91, "slid {} ticks", ended - t);
    assert!(ended - t >= 60, "slid only {} ticks", ended - t);
}

#[test]
fn slide_hop_keeps_its_speed() {
    let run = play("slide_hop");
    let hops: Vec<usize> = (0..run.len())
        .filter(|&t| run[t].2.jumped && slide::is_sliding(&run[t].0))
        .collect();
    assert!(hops.len() >= 4, "slide-hops {hops:?}");
    for &t in &hops {
        let (b, a) = (hspeed(run[t].0.vel), hspeed(run[t].1.vel));
        assert!((a - b).abs() <= 0.01 * b, "tick {t}: {b} -> {a}");
    }
    // Faster than running.
    let top = run.iter().map(|r| hspeed(r.1.vel)).fold(0.0, f32::max);
    assert!(top > 10.0, "top speed {top}");
}

#[test]
fn sliding_down_a_slope_accelerates() {
    let p = MoveParams::default();
    let w = StaticWorld::new(vec![
        boxed(
            Vec3::new(-100.0, -1.0, -100.0),
            Vec3::new(100.0, 0.0, 100.0),
        ),
        ramp(0.0, 20.0, 40.0),
    ]);
    let x = 15.0_f32;
    let mut s = PlayerState::spawn(Vec3::new(x, x * 40f32.to_radians().tan() + 1.0, 0.0));
    let idle = MoveCmd {
        yaw: FACE_NEG_X,
        ..IDLE
    };
    for _ in 0..30 {
        step_player_in(&mut s, idle, &w, &p, DT);
    }
    assert!(s.on_ground);
    s.vel = Vec3::new(-5.5, 0.0, 0.0);
    let crouch = with(idle, BUTTON_CROUCH);
    let ev = step_player_in(&mut s, crouch, &w, &p, DT);
    assert!(ev.slide_started);
    let start = hspeed(s.vel);
    for _ in 0..30 {
        step_player_in(&mut s, crouch, &w, &p, DT);
        assert!(slide::is_sliding(&s), "{s:?}");
    }
    assert!(hspeed(s.vel) > start + 1.0, "{start} -> {}", hspeed(s.vel));
}

#[test]
fn wall_jump_pushes_off_once_per_contact() {
    let run = play("wall_jump");
    let t = run
        .iter()
        .position(|r| r.2.wall_jumped)
        .expect("a wall jump");
    let (_, after, ..) = &run[t];
    assert!((after.vel.y - 0.9 * 6.5).abs() < 1e-4, "{}", after.vel);
    assert!((after.vel.x + 5.0).abs() < 1e-4, "{}", after.vel);

    // Two consecutive presses on one contact give one wall jump; after
    // leaving the wall and touching it again, a press gives another.
    let sc = scene("wall_jump");
    let mut sim = Sim::new(&sc);
    for &c in &script("wall_jump")[..t] {
        sim.tick(c);
    }
    let press = with(RUN, BUTTON_JUMP);
    let mut jumps = 0;
    for c in [press, RUN, press, RUN, RUN] {
        jumps += u32::from(sim.tick(c).0.wall_jumped);
    }
    assert_eq!(jumps, 1);
    let mut again = None;
    for i in 0..40 {
        let touching = !sim.state.on_ground && sim.state.wall_normal != Vec3::ZERO;
        let (ev, _) = sim.tick(if touching { press } else { RUN });
        if touching {
            assert!(ev.wall_jumped, "re-armed after leaving the wall");
            again = Some(i);
            break;
        }
    }
    assert!(again.is_some(), "touched the wall again");
}

#[test]
fn jump_pad_imposes_its_velocity() {
    let run = play("jump_pad");
    let t = run
        .iter()
        .position(|r| r.3.contains(&ZoneEvent::Entered(0)))
        .unwrap();
    assert_eq!(run[t].1.vel, Vec3::new(4.0, 10.0, 0.0));
    let top = run.iter().map(|r| r.1.feet().y).fold(0.0, f32::max);
    assert!(top > 2.0, "{top}");
}

#[test]
fn teleporter_moves_the_player() {
    let run = play("teleporter");
    let t = run
        .iter()
        .position(|r| r.3.contains(&ZoneEvent::Entered(0)))
        .unwrap();
    let feet = run[t].1.feet();
    assert!(
        (feet - Vec3::new(-20.0, 0.0, 10.0)).length() < 0.01,
        "{feet}"
    );
    assert!(
        run[t].1.vel.x < -5.0,
        "speed redirected to -X: {}",
        run[t].1.vel
    );
    assert!(run[t + 1].3.contains(&ZoneEvent::Left(0)));
}

#[test]
fn kill_volume_emits_its_trigger() {
    let run = play("kill_volume");
    let kills = run
        .iter()
        .filter(|r| r.3.contains(&ZoneEvent::Triggered(ZoneKind::KillVolume)))
        .count();
    assert_eq!(kills, 1);
}

#[test]
fn low_gravity_jumps_higher() {
    let run = play("low_gravity");
    assert!(run.iter().all(|r| r.1.gravity_scale == 0.25));
    let top = run.iter().map(|r| r.1.feet().y).fold(0.0, f32::max);
    // 1.056 m at full gravity, four times that at a quarter.
    assert!((top - 4.0 * 1.056).abs() < 0.1, "apex {top}");
}

#[test]
fn world_kill_volume_zeroes_health_and_reports_events() {
    use faset_core::{NetId, Tick};
    use faset_protocol::InputCommand;
    let sc = scene("kill_volume");
    let mut w = faset_sim::World::with_controller(1, sc.world.clone(), MoveParams::default());
    w.set_zones(sc.zones.clone());
    w.spawn_player(NetId(1), sc.start);
    let cmd = InputCommand {
        tick: Tick(0),
        buttons: 0,
        move_x: 0,
        move_y: 127,
        yaw: FACE_X,
        pitch: 0,
        weapon_slot: 0,
    };
    assert_eq!(w.health(NetId(1)), Some(100));
    for _ in 0..60 {
        w.step(&[(NetId(1), cmd)]);
    }
    let events = w.drain_zone_events();
    assert!(events.contains(&(NetId(1), ZoneEvent::Triggered(ZoneKind::KillVolume))));
    assert!(w.drain_zone_events().is_empty());
    assert_eq!(w.health(NetId(1)), Some(0));
}

/// Regenerates the reference files:
/// `cargo test -p faset-sim --test techniques -- --ignored regenerate`.
#[test]
#[ignore]
fn regenerate_technique_traces() {
    let cfg = ron::ser::PrettyConfig::new().depth_limit(2);
    std::fs::create_dir_all(format!("{}/tests/traces", env!("CARGO_MANIFEST_DIR"))).unwrap();
    for name in TECHNIQUES {
        let text = ron::ser::to_string_pretty(&record(name), cfg.clone()).unwrap();
        std::fs::write(trace_path(name), text + "\n").unwrap();
    }
}

//! Quake-style character controller (plan sections 5, 6 and 15).
//!
//! [`step_player`] advances one player by one fixed tick from a quantised
//! [`MoveCmd`]. It is a pure function of its inputs, so the client prediction
//! and the server run the same code and get bit-identical results on the
//! same host.
//!
//! Tick order: read the command and build the wish direction; update crouch,
//! slide and timers; ground test; jump; friction (on ground, no jump);
//! acceleration (ground or air); gravity half-step; move with collision,
//! steps and snapping; gravity half-step; ground test and landing event.
//! Zone triggers come with lot 4.3.
//!
//! Decisions (unspecified by the plan):
//! - On the ground (and on the tick of a jump, before it fires) the vertical
//!   velocity is reset to zero and no gravity is applied: walkable slopes are
//!   followed by velocity clipping, step-up and snap-down, so a player never
//!   creeps down a slope while standing still. Gravity applies in the air.
//! - A tick on which a jump fires uses air acceleration (the player has left
//!   the ground), which keeps bunny hops free of any ground acceleration.
//! - Rising players (vertical speed above zero) do not land, so jumping past
//!   a ledge does not cut the jump.
//! - Timers are in seconds and count as active while above half a tick, so a
//!   100 ms buffer spans exactly 6 ticks and the 80 ms coyote time 4 ticks.
//! - Auto-hop on: holding jump refreshes the jump buffer every tick. Off: only
//!   a press (button down after a tick up) arms it.
//! - Crouching keeps the feet in place on the ground and the centre in place in
//!   the air; standing up needs room for the standing capsule. Crouched ground
//!   speed is scaled by [`MoveParams::crouch_speed_scale`] (0.5).
//! - Eye height moves toward its target at [`MoveParams::eye_speed`] (8 m/s).
//! - Step-up is tried only by grounded players; the stepped move must land on
//!   a walkable surface whose touched point is at most 0.45 m above the feet.
//!   Grounded players treat non-walkable contacts as vertical walls, so the
//!   round capsule bottom never rides up edges or steep slopes.
//! - Ground is judged by the face normal under the contact (a short ray), not
//!   the tilted sweep normal of the round bottom: a capsule resting on a step
//!   edge stands on the step top, and rolls off a ledge once its bottom leaves
//!   the edge.
//! - Parry sweeps near the large boxes of a map are only accurate to a few
//!   millimetres, and one that starts in contact can miss. Flat ground is
//!   therefore measured by an exact ray, and every contact (plus walls along
//!   the motion and the four horizontal axes at the end of the tick) is
//!   settled back to the 1 mm skin with exact rays. Remaining penetration is
//!   bounded by 5 mm in the tests (edges and rotated faces).
//! - Soft brake: above 15 m/s, a landing removes 25 % of the horizontal speed
//!   in excess.
//! - Slide is out of scope: its parameters and `slide_timer` exist (the timer
//!   only counts down), no slide behaviour yet.
//! - Yaw sine and cosine come from fixed polynomials, not the platform libm, so
//!   a Windows client and a Linux server build the same wish direction.

use std::path::Path;

use faset_core::glam::Vec3;
use faset_protocol::quant::ANGLE_STEP;
use faset_protocol::InputCommand;
use serde::{Deserialize, Serialize};

use crate::collision::StaticWorld;
use crate::map_collision::{MapCollision, PLAYER_CROUCH_HEIGHT, PLAYER_HEIGHT, PLAYER_RADIUS};

/// Button bit: jump.
pub const BUTTON_JUMP: u16 = 1 << 0;
/// Button bit: crouch.
pub const BUTTON_CROUCH: u16 = 1 << 1;

/// Standing eye height above the feet, metres.
pub const EYE_HEIGHT: f32 = 1.6;
/// Crouched eye height above the feet, metres.
pub const CROUCH_EYE_HEIGHT: f32 = 0.8;

/// Gap kept between the capsule and any surface after a contact, metres.
pub const SKIN: f32 = 1e-3;
/// Sweep iterations per move.
pub const MAX_ITERATIONS: usize = 4;

/// The default parameter file, equal to [`MoveParams::default`].
pub const DEFAULT_PARAMS_RON: &str = include_str!("../data/move_params.ron");

/// Movement parameters, loaded from RON (`data/move_params.ron`). Missing
/// fields take their default value.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MoveParams {
    /// Run speed `v_run`, m/s.
    pub run_speed: f32,
    /// Ground acceleration `a_g`, 1/s.
    pub ground_accel: f32,
    /// Ground friction `f`, 1/s.
    pub friction: f32,
    /// Stop speed `v_stop`, m/s.
    pub stop_speed: f32,
    /// Air acceleration `a_a`, 1/s.
    pub air_accel: f32,
    /// Air wish speed cap `w_air`, m/s.
    pub air_wish_cap: f32,
    /// Gravity `g`, m/s².
    pub gravity: f32,
    /// Jump speed `v_j`, m/s.
    pub jump_speed: f32,
    /// Slide friction `f_slide`, 1/s (slide comes in a later lot).
    pub slide_friction: f32,
    /// Slide impulse `b`, m/s (slide comes in a later lot).
    pub slide_impulse: f32,
    /// Highest step climbed without jumping, metres.
    pub max_step: f32,
    /// Steepest walkable slope, degrees.
    pub max_slope_deg: f32,
    /// Safety speed cap, m/s.
    pub max_speed: f32,
    /// Jump request buffer, seconds.
    pub jump_buffer: f32,
    /// Coyote time after leaving a ledge, seconds.
    pub coyote_time: f32,
    /// Holding jump re-jumps on landing (server rule).
    pub auto_hop: bool,
    /// Horizontal speed above which landings brake, m/s.
    pub soft_brake_speed: f32,
    /// Fraction of the excess speed removed by each landing.
    pub soft_brake_fraction: f32,
    /// Ground test sweep length, metres.
    pub ground_probe: f32,
    /// Snap-down distance for grounded players, metres.
    pub snap_down: f32,
    /// Ground speed factor while crouched.
    pub crouch_speed_scale: f32,
    /// Eye height smoothing speed, m/s.
    pub eye_speed: f32,
}

impl Default for MoveParams {
    fn default() -> Self {
        Self {
            run_speed: 7.0,
            ground_accel: 10.0,
            friction: 6.0,
            stop_speed: 2.0,
            air_accel: 10.0,
            air_wish_cap: 0.8,
            gravity: 20.0,
            jump_speed: 6.5,
            slide_friction: 0.8,
            slide_impulse: 2.5,
            max_step: 0.45,
            max_slope_deg: 45.0,
            max_speed: 30.0,
            jump_buffer: 0.1,
            coyote_time: 0.08,
            auto_hop: true,
            soft_brake_speed: 15.0,
            soft_brake_fraction: 0.25,
            ground_probe: 0.05,
            snap_down: 0.3,
            crouch_speed_scale: 0.5,
            eye_speed: 8.0,
        }
    }
}

/// A parameter file that cannot be used.
#[derive(Debug, thiserror::Error)]
pub enum ParamsError {
    #[error("cannot read movement parameters: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid movement parameters: {0}")]
    Ron(#[from] ron::error::SpannedError),
}

impl MoveParams {
    /// Parses a RON parameter file.
    ///
    /// # Errors
    /// [`ParamsError::Ron`] on malformed RON or unknown fields.
    pub fn from_ron(text: &str) -> Result<Self, ParamsError> {
        Ok(ron::from_str(text)?)
    }

    /// Reads and parses a RON parameter file.
    ///
    /// # Errors
    /// [`ParamsError`] when the file cannot be read or parsed.
    pub fn load(path: &Path) -> Result<Self, ParamsError> {
        Self::from_ron(&std::fs::read_to_string(path)?)
    }

    /// Smallest normal `y` of a walkable surface (0.7071 for 45°).
    #[must_use]
    pub fn min_ground_normal_y(&self) -> f32 {
        let q = (self.max_slope_deg.clamp(0.0, 90.0) / 360.0 * 65536.0).round() as u16;
        sin_cos_q(q).1
    }
}

/// One tick of quantised input, as both sides simulate it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MoveCmd {
    /// Yaw, step 2π/65536 (0 faces -Z).
    pub yaw: u16,
    /// Pitch, step 2π/65536 (unused by movement).
    pub pitch: i16,
    /// Right axis, -127 to 127.
    pub move_x: i8,
    /// Forward axis, -127 to 127.
    pub move_y: i8,
    /// Button bits ([`BUTTON_JUMP`], [`BUTTON_CROUCH`]).
    pub buttons: u16,
}

impl From<InputCommand> for MoveCmd {
    fn from(c: InputCommand) -> Self {
        Self {
            yaw: c.yaw,
            pitch: c.pitch,
            move_x: c.move_x,
            move_y: c.move_y,
            buttons: c.buttons,
        }
    }
}

impl MoveCmd {
    #[must_use]
    pub fn jump(&self) -> bool {
        self.buttons & BUTTON_JUMP != 0
    }

    #[must_use]
    pub fn crouch(&self) -> bool {
        self.buttons & BUTTON_CROUCH != 0
    }

    /// Unit horizontal forward and right vectors of the yaw.
    #[must_use]
    pub fn basis(&self) -> (Vec3, Vec3) {
        let (s, c) = sin_cos_q(self.yaw);
        (Vec3::new(-s, 0.0, -c), Vec3::new(c, 0.0, -s))
    }

    /// Wish direction (unit, horizontal, or zero) and wish speed before any
    /// cap: `v_run · min(1, |m|)`.
    #[must_use]
    pub fn wish(&self, run_speed: f32) -> (Vec3, f32) {
        let mx = (f32::from(self.move_x) / 127.0).clamp(-1.0, 1.0);
        let mz = (f32::from(self.move_y) / 127.0).clamp(-1.0, 1.0);
        let (fwd, right) = self.basis();
        let m = (mx * mx + mz * mz).sqrt();
        let dir = (fwd * mz + right * mx).normalize_or_zero();
        (dir, run_speed * m.min(1.0))
    }
}

/// Sine and cosine of `q · 2π/65536`, from fixed polynomials (no libm).
#[must_use]
pub fn sin_cos_q(q: u16) -> (f32, f32) {
    let r = f32::from(q & 0x3FFF) * ANGLE_STEP; // [0, π/2)
    let x2 = r * r;
    let s = r
        * (1.0
            + x2 * (-1.0 / 6.0
                + x2 * (1.0 / 120.0
                    + x2 * (-1.0 / 5040.0
                        + x2 * (1.0 / 362_880.0
                            + x2 * (-1.0 / 39_916_800.0 + x2 / 6_227_020_800.0))))));
    let c = 1.0
        + x2 * (-0.5
            + x2 * (1.0 / 24.0
                + x2 * (-1.0 / 720.0
                    + x2 * (1.0 / 40_320.0
                        + x2 * (-1.0 / 3_628_800.0
                            + x2 * (1.0 / 479_001_600.0 - x2 / 87_178_291_200.0))))));
    match q >> 14 {
        0 => (s, c),
        1 => (c, -s),
        2 => (-s, -c),
        _ => (-c, s),
    }
}

/// Simulated state of one player.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayerState {
    /// Capsule centre, metres.
    pub pos: Vec3,
    /// Metres per second.
    pub vel: Vec3,
    pub on_ground: bool,
    pub crouched: bool,
    /// Eye height above the feet, smoothed, metres.
    pub eye_height: f32,
    /// Seconds of slide left (slide comes in a later lot).
    pub slide_timer: f32,
    /// Seconds a jump request stays valid.
    pub jump_buffer: f32,
    /// Seconds a jump is still allowed after leaving a ledge.
    pub coyote_timer: f32,
    /// Jump button state of the previous tick (press detection).
    pub jump_held: bool,
}

impl PlayerState {
    /// Standing still in the air at capsule centre `pos`; the first tick
    /// finds the ground.
    #[must_use]
    pub fn spawn(pos: Vec3) -> Self {
        Self {
            pos,
            vel: Vec3::ZERO,
            on_ground: false,
            crouched: false,
            eye_height: EYE_HEIGHT,
            slide_timer: 0.0,
            jump_buffer: 0.0,
            coyote_timer: 0.0,
            jump_held: false,
        }
    }

    /// Current capsule height.
    #[must_use]
    pub fn height(&self) -> f32 {
        if self.crouched {
            PLAYER_CROUCH_HEIGHT
        } else {
            PLAYER_HEIGHT
        }
    }

    /// Bottom of the capsule.
    #[must_use]
    pub fn feet(&self) -> Vec3 {
        self.pos - Vec3::Y * (self.height() * 0.5)
    }

    /// Eye position.
    #[must_use]
    pub fn eye(&self) -> Vec3 {
        self.feet() + Vec3::Y * self.eye_height
    }

    /// Fields in a fixed order, as bit patterns (state hashing).
    #[must_use]
    pub fn bits(&self) -> [u32; 11] {
        [
            self.pos.x.to_bits(),
            self.pos.y.to_bits(),
            self.pos.z.to_bits(),
            self.vel.x.to_bits(),
            self.vel.y.to_bits(),
            self.vel.z.to_bits(),
            u32::from(self.on_ground)
                | u32::from(self.crouched) << 1
                | u32::from(self.jump_held) << 2,
            self.eye_height.to_bits(),
            self.slide_timer.to_bits(),
            self.jump_buffer.to_bits(),
            self.coyote_timer.to_bits(),
        ]
    }
}

/// What happened during one tick.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MoveEvents {
    /// A jump fired.
    pub jumped: bool,
    /// The player touched the ground after being in the air.
    pub landed: bool,
    /// Downward speed just before the landing, m/s (0 without landing).
    pub impact_speed: f32,
}

/// Advances one player by one tick on a map. See the module docs.
pub fn step_player(
    state: &mut PlayerState,
    cmd: MoveCmd,
    world: &MapCollision,
    params: &MoveParams,
    dt: f32,
) -> MoveEvents {
    step_player_in(state, cmd, world.world(), params, dt)
}

/// [`step_player`] on a bare [`StaticWorld`].
pub fn step_player_in(
    state: &mut PlayerState,
    cmd: MoveCmd,
    world: &StaticWorld,
    p: &MoveParams,
    dt: f32,
) -> MoveEvents {
    let mut ev = MoveEvents::default();
    let active = 0.5 * dt;
    let min_ny = p.min_ground_normal_y();

    // 1. Wish direction.
    let (wish_dir, mut wish_speed) = cmd.wish(p.run_speed);

    // 2. Crouch, slide, timers.
    update_crouch(state, cmd.crouch(), world);
    let eye_target = if state.crouched {
        CROUCH_EYE_HEIGHT
    } else {
        EYE_HEIGHT
    };
    let eye_step = p.eye_speed * dt;
    state.eye_height += (eye_target - state.eye_height).clamp(-eye_step, eye_step);
    state.slide_timer = (state.slide_timer - dt).max(0.0);
    let jump_down = cmd.jump();
    let pressed = jump_down && !state.jump_held;
    state.jump_held = jump_down;
    state.jump_buffer = if pressed || (jump_down && p.auto_hop) {
        p.jump_buffer
    } else {
        (state.jump_buffer - dt).max(0.0)
    };
    state.coyote_timer = (state.coyote_timer - dt).max(0.0);

    // 3. Ground test: the ground may have gone (crouch, edge).
    if state.on_ground && probe_ground(world, state, p.ground_probe, min_ny).is_none() {
        state.on_ground = false;
        state.coyote_timer = p.coyote_time;
    }
    let started_on_ground = state.on_ground;

    // 4. Jump.
    let mut jumped = false;
    if state.on_ground {
        state.vel.y = 0.0;
    }
    if state.jump_buffer > active && (state.on_ground || state.coyote_timer > active) {
        state.vel.y = p.jump_speed;
        state.on_ground = false;
        state.coyote_timer = 0.0;
        state.jump_buffer = 0.0;
        jumped = true;
        ev.jumped = true;
    }

    // 5. Friction.
    if state.on_ground {
        apply_friction(&mut state.vel, p.friction, p.stop_speed, dt);
    }

    // 6. Acceleration.
    if state.on_ground {
        if state.crouched {
            wish_speed *= p.crouch_speed_scale;
        }
        state.vel = accelerate(
            state.vel,
            wish_dir,
            wish_speed,
            wish_speed,
            p.ground_accel,
            dt,
        );
    } else {
        let cap = wish_speed.min(p.air_wish_cap);
        state.vel = accelerate(state.vel, wish_dir, wish_speed, cap, p.air_accel, dt);
    }

    // 7. Gravity half-step.
    let airborne = !state.on_ground;
    if airborne {
        state.vel.y -= 0.5 * p.gravity * dt;
    }
    state.vel = state.vel.clamp_length_max(p.max_speed);
    let fall_speed = (-state.vel.y).max(0.0);

    // 8. Move with collision, steps and snap-down.
    let height = state.height();
    let (pos, vel) = move_with_steps(
        world,
        state.pos,
        state.vel,
        height,
        dt,
        state.on_ground,
        p,
        min_ny,
    );
    state.pos = pos;
    state.vel = vel;
    if started_on_ground && !jumped && probe_ground(world, state, p.ground_probe, min_ny).is_none()
    {
        if let Some(drop) = probe_ground(world, state, p.snap_down, min_ny) {
            state.pos.y -= drop;
        }
    }

    // 9. Gravity half-step.
    if airborne {
        state.vel.y -= 0.5 * p.gravity * dt;
    }
    state.vel = state.vel.clamp_length_max(p.max_speed);

    // 10. Ground test and landing.
    let can_land = !jumped && (started_on_ground || state.vel.y <= 0.0);
    let ground = if can_land {
        probe_ground(world, state, p.ground_probe, min_ny)
    } else {
        None
    };
    if let Some(drop) = ground {
        state.pos.y -= drop;
        if !started_on_ground {
            ev.landed = true;
            ev.impact_speed = fall_speed;
            state.vel.y = state.vel.y.max(0.0);
            soft_brake(&mut state.vel, p);
        }
        state.on_ground = true;
    } else {
        if started_on_ground && !jumped {
            state.coyote_timer = p.coyote_time;
        }
        state.on_ground = false;
    }
    state.pos = settle_axes(world, state.pos, state.height());
    ev
}

/// Crouch when asked; stand up only where the standing capsule fits.
///
/// The room check sweeps the crouched capsule over the space the standing
/// one would add (up on the ground; up and down in the air): sweeps ignore
/// the surface the player stands on, where an overlap test would report the
/// 1 mm contact.
fn update_crouch(state: &mut PlayerState, want: bool, world: &StaticWorld) {
    let shift = PLAYER_HEIGHT - PLAYER_CROUCH_HEIGHT;
    if want && !state.crouched {
        state.crouched = true;
        if state.on_ground {
            state.pos.y -= 0.5 * shift;
        }
    } else if !want && state.crouched {
        let h = PLAYER_CROUCH_HEIGHT;
        let fits = if state.on_ground {
            world.shape_cast(state.pos, Vec3::Y * shift, h).is_none()
        } else {
            world
                .shape_cast(state.pos, Vec3::Y * (0.5 * shift), h)
                .is_none()
                && world
                    .shape_cast(state.pos, Vec3::NEG_Y * (0.5 * shift), h)
                    .is_none()
        };
        if fits {
            state.crouched = false;
            if state.on_ground {
                state.pos.y += 0.5 * shift;
            }
        }
    }
}

/// Normal of the surface touched by a downward contact: the face normal
/// found by a short ray just above the contact point, offset 1 cm toward
/// the volume. A capsule resting on a step edge thus stands on the step top
/// (the sweep normal there is tilted by the round bottom). Falls back to the
/// sweep normal when the ray finds nothing.
fn surface_normal(world: &StaticWorld, centre: Vec3, height: f32, n: Vec3) -> Vec3 {
    if n.y <= 0.0 {
        return n;
    }
    let half = (0.5 * height - PLAYER_RADIUS).max(0.0);
    let contact = centre - Vec3::Y * half - n * PLAYER_RADIUS;
    let inward = Vec3::new(-n.x, 0.0, -n.z).normalize_or_zero() * 0.01;
    match world.ray_cast(contact + inward + Vec3::Y * 0.02, Vec3::NEG_Y, 0.04) {
        Some(h) if h.normal.y > 0.0 => h.normal,
        _ => n,
    }
}

/// Drop for a gap measured by a sweep: the gap minus [`SKIN`], or nothing
/// when that is within [`SKIN`]. Parry sweeps near the large boxes of a map
/// report contacts a few millimetres off; ignoring sub-millimetre drops keeps
/// a resting player from sinking tick after tick.
fn ground_drop(gap: f32) -> f32 {
    let drop = gap - SKIN;
    if drop > SKIN {
        drop
    } else {
        0.0
    }
}

/// Half length of the capsule segment.
fn half_segment(height: f32) -> f32 {
    (0.5 * height - PLAYER_RADIUS).max(0.0)
}

/// Downward move (negative: lift) that rests the player [`SKIN`] above a
/// walkable surface within `dist`, if there is one.
///
/// Decision: an exact ray from the bottom sphere centre measures flat ground
/// (most of a brush map); the capsule sweep, whose contact is only accurate
/// to a few millimetres, covers edges and slopes. A wall in contact cannot
/// hide the floor below it either.
fn probe_ground(world: &StaticWorld, state: &PlayerState, dist: f32, min_ny: f32) -> Option<f32> {
    let height = state.height();
    let bottom = state.pos - Vec3::Y * half_segment(height);
    let sweep = world.shape_cast(state.pos, Vec3::NEG_Y * dist, height);
    let ray = world
        .ray_cast(bottom, Vec3::NEG_Y, PLAYER_RADIUS + dist)
        .filter(|h| h.normal.y >= min_ny);
    if let Some(r) = ray {
        // On flat ground both agree; a slope or an edge met before the
        // point below the centre makes the sweep shorter.
        let gap = r.distance - PLAYER_RADIUS;
        return Some(match sweep {
            Some(s) if s.toi * dist + SKIN < gap => ground_drop(s.toi * dist),
            _ => gap - SKIN,
        });
    }
    let hit = sweep?;
    let at = state.pos - Vec3::Y * (hit.toi * dist);
    let n = surface_normal(world, at, height, hit.normal);
    (n.y >= min_ny).then(|| ground_drop(hit.toi * dist))
}

/// [`settle`] against walls along the four horizontal axes, so no tick ends
/// inside an axis-aligned wall: a sweep that starts in contact can report a
/// wrong normal and pass through.
fn settle_axes(world: &StaticWorld, mut pos: Vec3, height: f32) -> Vec3 {
    for n in [Vec3::X, Vec3::NEG_X, Vec3::Z, Vec3::NEG_Z] {
        pos = settle(world, pos, height, n);
    }
    pos
}

/// Pushes the capsule back to [`SKIN`] from the plane it just touched,
/// measured with an exact ray from the axis point facing the contact. Edges
/// and curved contacts (the ray meets another face) are left as they are.
fn settle(world: &StaticWorld, pos: Vec3, height: f32, n: Vec3) -> Vec3 {
    let half = half_segment(height);
    let axis = if n.y > 0.01 {
        pos - Vec3::Y * half
    } else if n.y < -0.01 {
        pos + Vec3::Y * half
    } else {
        pos
    };
    match world.ray_cast(axis, -n, PLAYER_RADIUS + 0.05) {
        Some(h) if h.normal.dot(n) > 0.5 => {
            let gap = h.distance * h.normal.dot(n) - PLAYER_RADIUS;
            if gap < SKIN {
                pos + h.normal * (SKIN - gap)
            } else {
                pos
            }
        }
        _ => pos,
    }
}

/// `v_h ← v_h · max(s − c f Δt, 0) / s`, with `c = max(s, v_stop)`.
pub fn apply_friction(vel: &mut Vec3, friction: f32, stop_speed: f32, dt: f32) {
    let s = (vel.x * vel.x + vel.z * vel.z).sqrt();
    if s <= 0.0 {
        return;
    }
    let c = s.max(stop_speed);
    let k = (s - c * friction * dt).max(0.0) / s;
    vel.x *= k;
    vel.z *= k;
}

/// Quake acceleration: adds at most `accel · wish_speed · dt` along
/// `wish_dir`, never pushing the projection past `cap`.
#[must_use]
pub fn accelerate(
    vel: Vec3,
    wish_dir: Vec3,
    wish_speed: f32,
    cap: f32,
    accel: f32,
    dt: f32,
) -> Vec3 {
    let delta = cap - vel.dot(wish_dir);
    if delta <= 0.0 {
        return vel;
    }
    vel + wish_dir * (accel * wish_speed * dt).min(delta)
}

fn soft_brake(vel: &mut Vec3, p: &MoveParams) {
    let s = (vel.x * vel.x + vel.z * vel.z).sqrt();
    if s > p.soft_brake_speed {
        let k = (s - (s - p.soft_brake_speed) * p.soft_brake_fraction) / s;
        vel.x *= k;
        vel.z *= k;
    }
}

fn horizontal_len_sq(v: Vec3) -> f32 {
    v.x * v.x + v.z * v.z
}

/// Slide move, then for grounded players a step-up retry (raise, move,
/// lower); keeps the result that went further horizontally.
#[allow(clippy::too_many_arguments)]
fn move_with_steps(
    world: &StaticWorld,
    pos: Vec3,
    vel: Vec3,
    height: f32,
    dt: f32,
    grounded: bool,
    p: &MoveParams,
    min_ny: f32,
) -> (Vec3, Vec3) {
    let upright = grounded.then_some(min_ny);
    let (down_pos, down_vel, blocked) = slide_move(world, pos, vel, height, dt, upright);
    if !grounded || !blocked || p.max_step <= 0.0 {
        return (down_pos, down_vel);
    }
    let up = Vec3::Y * p.max_step;
    let raised = match world.shape_cast(pos, up, height) {
        None => p.max_step,
        Some(h) => (h.toi * p.max_step - SKIN).max(0.0),
    };
    if raised <= SKIN {
        return (down_pos, down_vel);
    }
    let up_pos = pos + Vec3::Y * raised;
    let flat = Vec3::new(vel.x, 0.0, vel.z);
    let (step_pos, step_vel, _) = slide_move(world, up_pos, flat, height, dt, upright);
    // Lower back to the starting level at most.
    let lower = step_pos.y - pos.y;
    let Some(hit) = world.shape_cast(step_pos, Vec3::NEG_Y * lower, height) else {
        return (down_pos, down_vel);
    };
    let at = step_pos - Vec3::Y * (hit.toi * lower);
    if surface_normal(world, at, height, hit.normal).y < min_ny {
        return (down_pos, down_vel);
    }
    let landed = step_pos - Vec3::Y * ground_drop(hit.toi * lower);
    // The round bottom can rest on an edge above the step height (centre
    // short of the edge): the touched point, not the centre, must be within
    // a step of the feet.
    let contact = at - Vec3::Y * half_segment(height) - hit.normal * PLAYER_RADIUS;
    let feet = pos.y - 0.5 * height;
    let rise_ok = contact.y - feet <= p.max_step + 2.0 * SKIN;
    if rise_ok && horizontal_len_sq(landed - pos) > horizontal_len_sq(down_pos - pos) {
        (landed, step_vel)
    } else {
        (down_pos, down_vel)
    }
}

/// Removes the velocity component into the surface: `v − (v·n) n`.
fn clip(v: Vec3, n: Vec3) -> Vec3 {
    v - n * v.dot(n)
}

/// Velocity component allowed into a plane, m/s (float noise).
const INTO_PLANE: f32 = -1e-4;

/// Velocity that slides along every touched plane: one plane clip that does
/// not enter the others, else the crease of two planes, else zero.
fn resolve(vel: Vec3, planes: &[Vec3]) -> Vec3 {
    for (i, &n) in planes.iter().enumerate() {
        let v = clip(vel, n);
        let ok = planes
            .iter()
            .enumerate()
            .all(|(j, &m)| j == i || v.dot(m) >= INTO_PLANE);
        if ok {
            return v;
        }
    }
    if let [a, b] = planes {
        if let Some(d) = a.cross(*b).try_normalize() {
            return d * vel.dot(d);
        }
    }
    Vec3::ZERO
}

/// Sweeps along `vel · dt` in up to [`MAX_ITERATIONS`] parts, sliding on each
/// contact. Returns the end position, the velocity and whether anything was
/// hit.
///
/// `upright` (grounded players: the walkable limit) turns non-walkable
/// contacts into vertical walls, so a grounded player never rides up a steep
/// slope or a step edge: steps are climbed by the step-up retry only.
fn slide_move(
    world: &StaticWorld,
    mut pos: Vec3,
    mut vel: Vec3,
    height: f32,
    dt: f32,
    upright: Option<f32>,
) -> (Vec3, Vec3, bool) {
    let start = pos;
    let primal = vel;
    let mut planes = [Vec3::ZERO; 3];
    let mut n_planes = 0;
    let mut time_left = dt;
    let mut blocked = false;
    for _ in 0..MAX_ITERATIONS {
        let delta = vel * time_left;
        let len = delta.length();
        if len < 1e-6 {
            break;
        }
        let Some(hit) = world.shape_cast(pos, delta, height) else {
            pos += delta;
            break;
        };
        blocked = true;
        // Advance to the impact minus the skin, then restore the skin
        // against the touched plane (exact ray, sweeps are mm-accurate).
        let travel = (hit.toi * len - SKIN).max(0.0);
        pos += delta * (travel / len);
        time_left *= 1.0 - travel / len;
        pos = settle(world, pos, height, hit.normal);
        let mut n = hit.normal;
        if let Some(min_ny) = upright {
            if n.y > 0.0 && n.y < min_ny {
                n = Vec3::new(n.x, 0.0, n.z).try_normalize().unwrap_or(n);
            }
        }
        if let Some(&same) = planes[..n_planes].iter().find(|m| m.dot(n) > 0.999) {
            vel = clip(vel, same);
        } else if n_planes < planes.len() {
            planes[n_planes] = n;
            n_planes += 1;
            vel = resolve(vel, &planes[..n_planes]);
        } else {
            vel = Vec3::ZERO;
        }
        if vel.dot(primal) <= 0.0 {
            vel = Vec3::ZERO;
            break;
        }
    }
    // Near a contact (within the sweep tolerance) parry may report no hit or
    // a wrong normal: restore the skin against each touched plane and
    // against a wall ahead of the horizontal motion.
    for &n in &planes[..n_planes] {
        pos = settle(world, pos, height, n);
    }
    let moved = pos - start;
    if let Some(dir) = Vec3::new(moved.x, 0.0, moved.z).try_normalize() {
        pos = settle(world, pos, height, -dir);
    }
    (pos, vel, blocked)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_file_equals_defaults() {
        assert_eq!(
            MoveParams::from_ron(DEFAULT_PARAMS_RON).unwrap(),
            MoveParams::default()
        );
        let partial = MoveParams::from_ron("(gravity: 10.0)").unwrap();
        assert_eq!(partial.gravity, 10.0);
        assert_eq!(partial.run_speed, 7.0);
        assert!(MoveParams::from_ron("(gravty: 10.0)").is_err());
    }

    #[test]
    fn walkable_limit_is_45_degrees() {
        let ny = MoveParams::default().min_ground_normal_y();
        assert!((ny - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6, "{ny}");
        assert!(ny >= 0.707);
    }

    #[test]
    fn polynomial_sin_cos_matches_std() {
        for q in (0..=u16::MAX).step_by(7) {
            let (s, c) = sin_cos_q(q);
            let a = f32::from(q) * ANGLE_STEP;
            assert!(
                (s - a.sin()).abs() < 2e-6 && (c - a.cos()).abs() < 2e-6,
                "{q}"
            );
        }
    }

    #[test]
    fn wish_follows_yaw_and_caps_diagonals() {
        let cmd = MoveCmd {
            move_y: 127,
            ..MoveCmd::default()
        };
        let (d, w) = cmd.wish(7.0);
        assert!((d - Vec3::NEG_Z).length() < 1e-6 && (w - 7.0).abs() < 1e-6);
        let diag = MoveCmd {
            move_x: 127,
            move_y: 127,
            yaw: 16384, // quarter turn left: forward is -X
            ..MoveCmd::default()
        };
        let (d, w) = diag.wish(7.0);
        assert!(
            (d - Vec3::new(-1.0, 0.0, -1.0).normalize()).length() < 1e-5,
            "{d}"
        );
        assert!((w - 7.0).abs() < 1e-6);
        let half = MoveCmd {
            move_x: -64,
            ..MoveCmd::default()
        };
        assert!((half.wish(7.0).1 - 7.0 * 64.0 / 127.0).abs() < 1e-5);
    }

    #[test]
    fn air_acceleration_only_bounds_the_projection() {
        // Lateral wish at run speed: the projection is 0, so the full step
        // s = a w Δt goes sideways although w_air is 0.8 m/s.
        let v = Vec3::new(0.0, 0.0, -7.0);
        let s: f32 = 10.0 * 7.0 / 60.0;
        let out = accelerate(v, Vec3::X, 7.0, 0.8, 10.0, 1.0 / 60.0);
        assert!((out.x - 0.8).abs() < 1e-6, "Δ = 0.8 caps the step");
        let out = accelerate(
            v,
            Vec3::new(-0.05, 0.0, 1.0).normalize(),
            7.0,
            0.8,
            10.0,
            1.0 / 60.0,
        );
        assert!(out.length() < v.length());
        // Optimal angle: p = w_air − s gives |v'|² = |v|² + 2 s w_air − s².
        let cos = (0.8 - s) / 7.0;
        let dir = Vec3::new((1.0 - cos * cos).sqrt(), 0.0, -cos);
        let out = accelerate(v, dir, 7.0, 0.8, 10.0, 1.0 / 60.0);
        let expect = 49.0 + 2.0 * s * 0.8 - s * s;
        assert!((out.length_squared() - expect).abs() < 1e-3);
    }

    #[test]
    fn friction_stops_slow_players_quickly() {
        let mut v = Vec3::new(1.0, 0.0, 0.0);
        apply_friction(&mut v, 6.0, 2.0, 1.0 / 60.0);
        assert!((v.x - 0.8).abs() < 1e-6, "c = v_stop below the stop speed");
        let mut v = Vec3::new(0.1, 0.0, 0.0);
        apply_friction(&mut v, 6.0, 2.0, 1.0 / 60.0);
        assert_eq!(v.x, 0.0);
    }
}

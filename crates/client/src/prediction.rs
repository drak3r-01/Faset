//! Local prediction, reconciliation against snapshots and visual smoothing.
//!
//! # Which tick a snapshot confirms
//!
//! A snapshot of tick `S` carries the server state after the step of tick
//! `k = S - 1` and the tick of the last command the server actually applied.
//! When the client's commands arrive in time that command is the one of `k`,
//! so [`Prediction::reconcile`] compares the server state with the predicted
//! state of `k`, as the plan says. When the server had to repeat an older
//! command (the one for `k` was late or lost), the snapshot still describes
//! tick `k`, so it is compared at `k` too (decision): that is exactly the case
//! where prediction and server disagree and a correction is due.
//!
//! # Movement (lot 4.11)
//!
//! The local player moves with the character controller of `faset-sim`
//! ([`faset_sim::World::with_controller`]), the same code and parameters as
//! the server, on the same map collision and zones. Each history entry
//! keeps the whole controller state and zone membership after its tick, the
//! command as sent, whether the player was frozen (match phase, death) and
//! the run and jump multipliers of that tick.
//!
//! # Correction
//!
//! Past the [`Tolerance`] (1 cm position, 5 cm/s velocity, or a different
//! weapon) the state of `k` is restored and commands `k + 1` to the newest
//! tick are replayed with the same `World::step` code. The restored state is
//! the full predicted controller state of `k` (crouch, timers, jump buffer,
//! wall contact, zone membership) with the position, the velocity and the
//! ground flag the snapshot carries (decision: the snapshot holds no more of
//! the controller state, and the snapshot velocity is the controller's own
//! since lot 4.11, so the restored state differs from the server's only by
//! the quantization: at most 1 mm per axis and 1.6 cm/s per axis, below the
//! tolerance, hence no correction loop). The display does not move: the
//! difference between the old and new prediction goes into the
//! [`VisualOffset`], which decays by `e^{-Δt/τ}` every tick
//! (τ = [`SMOOTHING_TAU`]) and snaps to zero when it exceeds
//! [`SNAP_DISTANCE`].
//!
//! Errors use Euclidean distances (decision). Snapshot quantization rounds
//! each axis by at most 1/512 m and 1/32 m/s: at most 3.4 mm in position
//! and 5.4 cm/s in velocity on three axes, which may trigger a harmless
//! replay while the player moves vertically.

use faset_core::glam::Vec3;
use faset_core::{NetId, Tick};
use faset_protocol::quant::{dequantize_pitch_command, dequantize_yaw};
use faset_protocol::InputCommand;
use faset_sim::controller::PlayerState;
use faset_sim::{MoveParams, MoveScale, StaticWorld, World, ZoneTracker, Zones};

/// Ticks of commands and predicted states kept for reconciliation.
pub const HISTORY_TICKS: usize = 128;
/// Time constant τ of the visual offset decay, seconds.
pub const SMOOTHING_TAU: f32 = 0.1;
/// A visual offset longer than this (metres) is dropped at once.
pub const SNAP_DISTANCE: f32 = 2.0;
/// Commands carried by each input packet: the newest unacknowledged ones
/// (decision: the newest command plus the two before it, so every command is
/// sent three times).
pub const COMMANDS_PER_PACKET: usize = 3;

/// Offsets shorter than this (metres) are cleared rather than decayed
/// forever.
const OFFSET_EPSILON: f32 = 1e-5;

/// The local player's state after one tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PredictedState {
    /// Capsule centre, metres.
    pub position: Vec3,
    /// Metres per second: the controller's velocity, as the server sends it.
    pub velocity: Vec3,
    /// Radians.
    pub yaw: f32,
    /// Radians.
    pub pitch: f32,
    /// Weapon slot, 0 to 15.
    pub weapon: u8,
    /// Standing on the ground (lot 4.11).
    pub on_ground: bool,
}

impl PredictedState {
    /// Standing still at `position`, facing -Z with the first weapon, not
    /// yet on the ground (the first tick finds it).
    #[must_use]
    pub fn at_rest(position: Vec3) -> Self {
        Self {
            position,
            velocity: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            weapon: 0,
            on_ground: false,
        }
    }
}

/// What one snapshot says about the local player.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AuthoritativeState {
    /// Snapshot tick: the world tick after the step it describes.
    pub tick: Tick,
    /// Tick of the last command of this client the server applied.
    pub last_command: Option<Tick>,
    /// The server's state of the local player.
    pub state: PredictedState,
}

impl AuthoritativeState {
    /// The tick whose result the state is (`tick - 1`), if any.
    #[must_use]
    pub fn state_tick(&self) -> Option<Tick> {
        self.tick.0.checked_sub(1).map(Tick)
    }
}

/// Thresholds above which a prediction is corrected.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tolerance {
    /// Metres.
    pub position: f32,
    /// Metres per second.
    pub velocity: f32,
}

impl Default for Tolerance {
    /// The plan's 1 cm and 5 cm/s.
    fn default() -> Self {
        Self {
            position: 0.01,
            velocity: 0.05,
        }
    }
}

/// One tick of history: the command and the state it produced.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryEntry {
    pub tick: Tick,
    /// The command as sent to the server.
    pub command: InputCommand,
    /// Predicted (or, after a correction, server) state after the command.
    pub state: PredictedState,
    /// The whole controller state after the command.
    pub controller: PlayerState,
    /// Zones the player was in after the command.
    pub zones: ZoneTracker,
    /// The command was simulated frozen (see [`InputCommand::frozen`]).
    pub frozen: bool,
    /// Run and jump multipliers of the tick.
    pub scale: MoveScale,
}

/// Circular buffer of the last [`HISTORY_TICKS`] ticks.
#[derive(Debug, Clone)]
pub struct PredictionHistory {
    slots: Vec<Option<HistoryEntry>>,
    newest: Option<Tick>,
    len: usize,
}

impl Default for PredictionHistory {
    fn default() -> Self {
        Self::new()
    }
}

impl PredictionHistory {
    /// An empty history.
    #[must_use]
    pub fn new() -> Self {
        Self {
            slots: vec![None; HISTORY_TICKS],
            newest: None,
            len: 0,
        }
    }

    fn slot(tick: Tick) -> usize {
        tick.0 as usize % HISTORY_TICKS
    }

    /// Appends the entry of the tick after the newest one (any tick when
    /// empty), overwriting the entry [`HISTORY_TICKS`] ticks older.
    ///
    /// # Errors
    /// [`PredictionError::UnexpectedTick`] when the tick does not follow the
    /// newest one.
    pub fn push(&mut self, entry: HistoryEntry) -> Result<(), PredictionError> {
        if let Some(n) = self.newest {
            if entry.tick != n.next() {
                return Err(PredictionError::UnexpectedTick {
                    expected: n.next(),
                    got: entry.tick,
                });
            }
        }
        let tick = entry.tick;
        self.slots[Self::slot(tick)] = Some(entry);
        self.newest = Some(tick);
        self.len = (self.len + 1).min(HISTORY_TICKS);
        Ok(())
    }

    /// The entry of `tick`, if still held.
    #[must_use]
    pub fn get(&self, tick: Tick) -> Option<&HistoryEntry> {
        self.slots[Self::slot(tick)]
            .as_ref()
            .filter(|e| e.tick == tick)
    }

    fn get_mut(&mut self, tick: Tick) -> Option<&mut HistoryEntry> {
        self.slots[Self::slot(tick)]
            .as_mut()
            .filter(|e| e.tick == tick)
    }

    /// The newest entry.
    #[must_use]
    pub fn newest(&self) -> Option<&HistoryEntry> {
        self.get(self.newest?)
    }

    /// Entries held, at most [`HISTORY_TICKS`].
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether nothing is held.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Entries from the oldest to the newest.
    pub fn iter(&self) -> impl Iterator<Item = &HistoryEntry> + '_ {
        let newest = self.newest.map_or(0, |t| t.0);
        let len = self.len as u32;
        (0..len).filter_map(move |i| self.get(Tick(newest.wrapping_sub(len - 1 - i))))
    }

    /// Forgets everything.
    pub fn clear(&mut self) {
        self.slots.fill(None);
        self.newest = None;
        self.len = 0;
    }
}

/// Display-only offset that hides corrections: the displayed position is
/// the predicted one plus this offset.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct VisualOffset {
    value: Vec3,
}

impl VisualOffset {
    /// The current offset, metres.
    #[must_use]
    pub fn value(&self) -> Vec3 {
        self.value
    }

    /// Per-tick decay factor `e^{-Δt/τ}` (about 0.846).
    #[must_use]
    pub fn decay_factor() -> f32 {
        (-faset_core::DT / SMOOTHING_TAU).exp()
    }

    /// One tick of decay.
    pub fn decay(&mut self) {
        self.value *= Self::decay_factor();
        if self.value.length() < OFFSET_EPSILON {
            self.value = Vec3::ZERO;
        }
    }

    /// Adds the jump of a correction (old prediction minus new one), so the
    /// display stays put. Returns `true` when the total exceeds
    /// [`SNAP_DISTANCE`] and the offset was dropped instead (the display
    /// jumps to the new prediction).
    pub fn absorb(&mut self, jump: Vec3) -> bool {
        let total = self.value + jump;
        if total.length() > SNAP_DISTANCE {
            self.value = Vec3::ZERO;
            true
        } else {
            self.value = total;
            false
        }
    }

    /// Clears the offset.
    pub fn reset(&mut self) {
        self.value = Vec3::ZERO;
    }
}

/// Misuse of a [`Prediction`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PredictionError {
    /// [`Prediction::start`] has not been called.
    #[error("prediction has not started")]
    NotStarted,
    /// Ticks must be predicted one after the other.
    #[error("expected the command of tick {expected:?}, got {got:?}")]
    UnexpectedTick {
        /// The tick that would follow.
        expected: Tick,
        /// The tick given.
        got: Tick,
    },
}

/// Result of comparing one snapshot with the history.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReconcileOutcome {
    /// Tick compared (`snapshot tick - 1`).
    pub tick: Tick,
    /// Distance between the server and predicted positions, metres.
    pub position_error: f32,
    /// Distance between the server and predicted velocities, m/s.
    pub velocity_error: f32,
    /// Whether the history was corrected and replayed.
    pub corrected: bool,
    /// Ticks replayed after the corrected one.
    pub replayed: u32,
    /// Whether the visual offset was dropped (correction over
    /// [`SNAP_DISTANCE`]).
    pub snapped: bool,
}

/// Prediction of the local player: history, reconciliation and smoothing.
pub struct Prediction {
    me: NetId,
    seed: u64,
    statics: StaticWorld,
    params: MoveParams,
    zones: Zones,
    /// Holds only the local player, in the state of the newest tick.
    world: World,
    history: PredictionHistory,
    /// State after the newest predicted tick (the start state before any).
    current: Option<PredictedState>,
    /// Tick the next [`predict`](Self::predict) expects.
    next: Option<Tick>,
    offset: VisualOffset,
    acked: Option<Tick>,
    reconciled: Option<Tick>,
    tolerance: Tolerance,
    frozen: bool,
    scale: MoveScale,
}

impl std::fmt::Debug for Prediction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Prediction")
            .field("me", &self.me)
            .field("next", &self.next)
            .field("current", &self.current)
            .field("history", &self.history.len())
            .field("offset", &self.offset)
            .field("acked", &self.acked)
            .field("frozen", &self.frozen)
            .finish_non_exhaustive()
    }
}

/// The predicted state of the player in `world` after `command`.
fn read_state(world: &World, me: NetId, command: &InputCommand) -> (PredictedState, PlayerState) {
    let s = world
        .controller_state(me)
        .unwrap_or_else(|| PlayerState::spawn(world.position(me).unwrap_or(Vec3::ZERO)));
    (
        PredictedState {
            position: s.pos,
            velocity: s.vel,
            yaw: dequantize_yaw(command.yaw),
            pitch: dequantize_pitch_command(command.pitch),
            weapon: command.weapon_slot.min(15),
            on_ground: s.on_ground,
        },
        s,
    )
}

/// One tick of the local player: the command as simulated (frozen or not)
/// with the tick's multipliers.
fn simulate(
    world: &mut World,
    me: NetId,
    command: &InputCommand,
    frozen: bool,
    scale: MoveScale,
) -> (PredictedState, PlayerState, ZoneTracker) {
    world.set_move_scale(me, scale);
    let applied = if frozen { command.frozen() } else { *command };
    world.step(&[(me, applied)]);
    world.drain_zone_events();
    let (state, controller) = read_state(world, me, command);
    (state, controller, world.zone_tracker(me).unwrap_or_default())
}

impl Prediction {
    /// Prediction of player `me` on the map `statics` with the server's
    /// world `seed`, the default controller parameters and no zones. Call
    /// [`start`](Self::start) before predicting.
    #[must_use]
    pub fn new(me: NetId, seed: u64, statics: StaticWorld) -> Self {
        let params = MoveParams::default();
        let world = World::with_controller(seed, statics.clone(), params);
        Self {
            me,
            seed,
            statics,
            params,
            zones: Zones::default(),
            world,
            history: PredictionHistory::new(),
            current: None,
            next: None,
            offset: VisualOffset::default(),
            acked: None,
            reconciled: None,
            tolerance: Tolerance::default(),
            frozen: false,
            scale: MoveScale::ONE,
        }
    }

    /// Replaces the controller parameters (the server's
    /// `ServerConfig::move_params`). Call before [`start`](Self::start).
    #[must_use]
    pub fn with_params(mut self, params: MoveParams) -> Self {
        self.params = params;
        self
    }

    /// Sets the map's gameplay zones (the server's `MapConfig::zones`). Call
    /// before [`start`](Self::start).
    #[must_use]
    pub fn with_zones(mut self, zones: Zones) -> Self {
        self.zones = zones;
        self
    }

    /// Replaces the correction thresholds.
    #[must_use]
    pub fn with_tolerance(mut self, tolerance: Tolerance) -> Self {
        self.tolerance = tolerance;
        self
    }

    /// Whether the next predicted ticks are frozen (the match phase or the
    /// player's death stops movement on the server too).
    pub fn set_frozen(&mut self, frozen: bool) {
        self.frozen = frozen;
    }

    /// Run and jump multipliers of the next predicted ticks (class speed,
    /// timed bonuses, as the replicated mode state shows them).
    pub fn set_move_scale(&mut self, scale: MoveScale) {
        self.scale = scale;
    }

    /// (Re)starts prediction from `state`, the first command being the one
    /// of tick `first`. Clears the history.
    pub fn start(&mut self, first: Tick, state: PredictedState) {
        self.world = World::with_controller(self.seed, self.statics.clone(), self.params);
        self.world.set_zones(self.zones.clone());
        self.world.spawn_player(self.me, state.position);
        let mut s = PlayerState::spawn(state.position);
        s.vel = state.velocity;
        s.on_ground = state.on_ground;
        self.world.set_controller_state(self.me, s);
        self.history.clear();
        self.current = Some(state);
        self.next = Some(first);
        self.offset.reset();
        self.acked = None;
        self.reconciled = None;
    }

    /// Simulates the command of the next tick at once and records it.
    /// Decays the visual offset by one tick first.
    ///
    /// # Errors
    /// [`PredictionError::NotStarted`], or
    /// [`PredictionError::UnexpectedTick`] unless `command.tick` follows the
    /// previous one.
    pub fn predict(&mut self, command: InputCommand) -> Result<PredictedState, PredictionError> {
        let (Some(next), Some(_)) = (self.next, self.current) else {
            return Err(PredictionError::NotStarted);
        };
        if command.tick != next {
            return Err(PredictionError::UnexpectedTick {
                expected: next,
                got: command.tick,
            });
        }
        self.offset.decay();
        let (state, controller, zones) =
            simulate(&mut self.world, self.me, &command, self.frozen, self.scale);
        self.history.push(HistoryEntry {
            tick: command.tick,
            command,
            state,
            controller,
            zones,
            frozen: self.frozen,
            scale: self.scale,
        })?;
        self.current = Some(state);
        self.next = Some(next.next());
        Ok(state)
    }

    /// Compares a snapshot with the predicted state of the tick it
    /// describes and corrects the history if needed (see the module docs).
    /// Records the acknowledged command. Returns `None` when the snapshot is
    /// stale (not newer than the last one reconciled) or its tick is not in
    /// the history.
    pub fn reconcile(&mut self, server: &AuthoritativeState) -> Option<ReconcileOutcome> {
        if let Some(l) = server.last_command {
            if self.acked.is_none_or(|a| l > a) {
                self.acked = Some(l);
            }
        }
        if self.reconciled.is_some_and(|r| server.tick <= r) {
            return None;
        }
        self.reconciled = Some(server.tick);
        let k = server.state_tick()?;
        let entry = self.history.get(k)?;
        let predicted = entry.state;
        let position_error = (server.state.position - predicted.position).length();
        let velocity_error = (server.state.velocity - predicted.velocity).length();
        let mut outcome = ReconcileOutcome {
            tick: k,
            position_error,
            velocity_error,
            corrected: false,
            replayed: 0,
            snapped: false,
        };
        if position_error <= self.tolerance.position
            && velocity_error <= self.tolerance.velocity
            && server.state.weapon == predicted.weapon
        {
            return Some(outcome);
        }

        let old = self.current?.position;
        // The full predicted state of k with what the snapshot knows.
        let mut base = entry.controller;
        base.pos = server.state.position;
        base.vel = server.state.velocity;
        base.on_ground = server.state.on_ground;
        let zones = entry.zones.clone();
        if let Some(e) = self.history.get_mut(k) {
            e.state = server.state;
            e.controller = base;
        }
        self.world.set_controller_state(self.me, base);
        self.world.set_zone_tracker(self.me, zones);
        let mut state = server.state;
        let newest = self.history.newest().map_or(k, |e| e.tick);
        let mut t = k;
        while t != newest {
            t = t.next();
            let Some(entry) = self.history.get_mut(t) else {
                break;
            };
            let (s, controller, zones) = simulate(
                &mut self.world,
                self.me,
                &entry.command,
                entry.frozen,
                entry.scale,
            );
            entry.state = s;
            entry.controller = controller;
            entry.zones = zones;
            state = s;
            outcome.replayed += 1;
        }
        self.current = Some(state);
        outcome.corrected = true;
        outcome.snapped = self.offset.absorb(old - state.position);
        tracing::debug!(
            tick = k.0,
            error = position_error,
            replayed = outcome.replayed,
            snapped = outcome.snapped,
            "prediction corrected"
        );
        Some(outcome)
    }

    /// The newest unacknowledged commands to put in the next input packet,
    /// oldest first: at most [`COMMANDS_PER_PACKET`], all newer than the last
    /// command a snapshot acknowledged.
    #[must_use]
    pub fn unacknowledged(&self) -> Vec<InputCommand> {
        let Some(newest) = self.history.newest().map(|e| e.tick) else {
            return Vec::new();
        };
        let mut out: Vec<InputCommand> = (0..COMMANDS_PER_PACKET as u32)
            .map_while(|back| newest.0.checked_sub(back).map(Tick))
            .take_while(|&t| self.acked.is_none_or(|a| t > a))
            .filter_map(|t| self.history.get(t).map(|e| e.command))
            .collect();
        out.reverse();
        out
    }

    /// The local player.
    #[must_use]
    pub fn player(&self) -> NetId {
        self.me
    }

    /// Predicted state after the newest tick (the start state before any).
    #[must_use]
    pub fn state(&self) -> Option<PredictedState> {
        self.current
    }

    /// The whole controller state after the newest tick.
    #[must_use]
    pub fn controller_state(&self) -> Option<PlayerState> {
        self.world.controller_state(self.me)
    }

    /// Where to draw the local player: predicted position plus the visual
    /// offset.
    #[must_use]
    pub fn display_position(&self) -> Option<Vec3> {
        Some(self.current?.position + self.offset.value())
    }

    /// The tick [`predict`](Self::predict) expects next.
    #[must_use]
    pub fn next_tick(&self) -> Option<Tick> {
        self.next
    }

    /// Last command acknowledged by a snapshot.
    #[must_use]
    pub fn acknowledged(&self) -> Option<Tick> {
        self.acked
    }

    /// The history.
    #[must_use]
    pub fn history(&self) -> &PredictionHistory {
        &self.history
    }

    /// The visual offset.
    #[must_use]
    pub fn visual_offset(&self) -> VisualOffset {
        self.offset
    }

    /// The correction thresholds.
    #[must_use]
    pub fn tolerance(&self) -> Tolerance {
        self.tolerance
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use faset_core::DT;
    use faset_protocol::quant::quantize_yaw;
    use faset_sim::controller::BUTTON_JUMP;

    const ME: NetId = NetId(3);
    const SPAWN: Vec3 = Vec3::new(0.0, 0.91, 0.0);

    fn cmd(tick: u32, move_x: i8, move_y: i8) -> InputCommand {
        InputCommand {
            tick: Tick(tick),
            buttons: 0,
            move_x,
            move_y,
            yaw: quantize_yaw(0.0),
            pitch: 0,
            weapon_slot: 1,
        }
    }

    fn started(first: u32) -> Prediction {
        let mut p = Prediction::new(ME, 0, StaticWorld::test_arena());
        p.start(Tick(first), PredictedState::at_rest(SPAWN));
        p
    }

    /// What the server would hold after running `commands` from the full
    /// controller state `from`.
    fn server_states(from: PlayerState, commands: &[InputCommand]) -> Vec<PredictedState> {
        let mut w = World::with_controller(0, StaticWorld::test_arena(), MoveParams::default());
        w.spawn_player(ME, from.pos);
        w.set_controller_state(ME, from);
        commands
            .iter()
            .map(|c| simulate(&mut w, ME, c, false, MoveScale::ONE).0)
            .collect()
    }

    fn snapshot(tick: u32, last: u32, state: PredictedState) -> AuthoritativeState {
        AuthoritativeState {
            tick: Tick(tick),
            last_command: Some(Tick(last)),
            state,
        }
    }

    #[test]
    fn history_keeps_the_last_128_ticks() {
        let mut p = started(10);
        for t in 10..10 + 300 {
            p.predict(cmd(t, 0, 0)).unwrap();
        }
        let h = p.history();
        assert_eq!(h.len(), HISTORY_TICKS);
        assert_eq!(h.newest().unwrap().tick, Tick(309));
        assert!(h.get(Tick(309 - 127)).is_some());
        assert!(h.get(Tick(309 - 128)).is_none());
        let ticks: Vec<u32> = h.iter().map(|e| e.tick.0).collect();
        assert_eq!(ticks, (182..310).collect::<Vec<_>>());
    }

    #[test]
    fn ticks_must_be_consecutive() {
        let mut p = Prediction::new(ME, 0, StaticWorld::test_arena());
        assert_eq!(p.predict(cmd(0, 0, 0)), Err(PredictionError::NotStarted));
        p.start(Tick(5), PredictedState::at_rest(SPAWN));
        assert!(p.predict(cmd(5, 0, 0)).is_ok());
        assert_eq!(
            p.predict(cmd(7, 0, 0)),
            Err(PredictionError::UnexpectedTick {
                expected: Tick(6),
                got: Tick(7)
            })
        );
    }

    #[test]
    fn matching_snapshot_confirms_without_correction() {
        let mut p = started(1);
        let cmds: Vec<InputCommand> = (1..=20).map(|t| cmd(t, 50, 127)).collect();
        for &c in &cmds {
            p.predict(c).unwrap();
        }
        let server = server_states(PlayerState::spawn(SPAWN), &cmds);
        // Snapshot 11 describes tick 10, the 10th command.
        let out = p.reconcile(&snapshot(11, 10, server[9])).unwrap();
        assert_eq!(out.tick, Tick(10));
        assert!(!out.corrected);
        assert!(out.position_error < 1e-6);
        assert_eq!(p.acknowledged(), Some(Tick(10)));
        // Stale and duplicate snapshots are ignored.
        assert!(p.reconcile(&snapshot(11, 10, server[9])).is_none());
        assert!(p.reconcile(&snapshot(9, 8, server[7])).is_none());
    }

    #[test]
    fn mismatch_replays_from_the_server_state_without_moving_the_display() {
        let mut p = started(1);
        let cmds: Vec<InputCommand> = (1..=30).map(|t| cmd(t, 0, 127)).collect();
        for &c in &cmds {
            p.predict(c).unwrap();
        }
        let display_before = p.display_position().unwrap();
        // The server holds the player 0.5 m to the side at tick 20.
        let mut truth = p.history().get(Tick(20)).unwrap().state;
        truth.position.x += 0.5;
        let mut base = p.history().get(Tick(20)).unwrap().controller;
        base.pos = truth.position;
        let out = p.reconcile(&snapshot(21, 20, truth)).unwrap();
        assert!(out.corrected && !out.snapped);
        assert_eq!(out.replayed, 10);
        assert!((out.position_error - 0.5).abs() < 1e-4);
        // Replay is the same simulation from the restored full state.
        let replay = server_states(base, &cmds[20..]);
        assert_eq!(p.state().unwrap(), *replay.last().unwrap());
        assert_eq!(p.history().get(Tick(20)).unwrap().state, truth);
        assert_eq!(p.history().get(Tick(30)).unwrap().state, replay[9]);
        // No visible jump at the correction.
        assert!((p.display_position().unwrap() - display_before).length() < 1e-5);
        assert!((p.visual_offset().value().x + 0.5).abs() < 1e-4);

        // The offset decays by e^{-Δt/τ} per tick, display steps stay small.
        let run = MoveParams::default().run_speed;
        let mut last = p.display_position().unwrap();
        let mut offset = p.visual_offset().value().length();
        for t in 31..=90 {
            p.predict(cmd(t, 0, 127)).unwrap();
            let now = p.visual_offset().value().length();
            assert!(now <= offset * VisualOffset::decay_factor() + 1e-6);
            let step = (p.display_position().unwrap() - last).length();
            assert!(step <= run * DT + offset * (1.0 - VisualOffset::decay_factor()) + 1e-4);
            last = p.display_position().unwrap();
            offset = now;
        }
        assert!(offset < 1e-3, "offset left: {offset}");
    }

    #[test]
    fn a_correction_keeps_the_predicted_controller_state() {
        // A jump in flight: the restored state keeps the jump buffer and
        // the held button, so the replay does not jump again.
        let mut p = started(1);
        let mut cmds: Vec<InputCommand> = (1..=40).map(|t| cmd(t, 0, 0)).collect();
        for c in &mut cmds[20..] {
            c.buttons = BUTTON_JUMP;
        }
        for &c in &cmds {
            p.predict(c).unwrap();
        }
        let mut truth = p.history().get(Tick(25)).unwrap().state;
        truth.position.z += 0.2;
        let before = p.history().get(Tick(25)).unwrap().controller;
        assert!(!before.on_ground && before.jump_held);
        let uncorrected = p.state().unwrap().position;
        let out = p.reconcile(&snapshot(26, 25, truth)).unwrap();
        assert!(out.corrected);
        let mut base = before;
        base.pos = truth.position;
        let replay = server_states(base, &cmds[25..]);
        assert_eq!(p.state().unwrap(), *replay.last().unwrap());
        // The same jump, 20 cm further.
        let shifted = p.state().unwrap().position;
        assert!((shifted.y - uncorrected.y).abs() < 1e-4, "{shifted} {uncorrected}");
        assert!((shifted.z - uncorrected.z - 0.2).abs() < 1e-4, "{shifted} {uncorrected}");
        let confirm = snapshot(41, 40, p.state().unwrap());
        assert!(p.reconcile(&confirm).is_some_and(|o| !o.corrected));
    }

    #[test]
    fn frozen_ticks_do_not_move_and_are_replayed_frozen() {
        let mut p = started(1);
        for t in 1..=30 {
            p.predict(cmd(t, 0, 0)).unwrap();
        }
        let rest = p.state().unwrap().position;
        p.set_frozen(true);
        for t in 31..=40 {
            p.predict(cmd(t, 0, 127)).unwrap();
        }
        assert!((p.state().unwrap().position - rest).length() < 1e-4);
        assert!(p.history().get(Tick(35)).unwrap().frozen);
        // A correction at 32 replays the frozen ticks frozen.
        let mut truth = p.history().get(Tick(32)).unwrap().state;
        truth.position.x += 0.1;
        p.reconcile(&snapshot(33, 32, truth)).unwrap();
        assert!((p.state().unwrap().position - truth.position).length() < 1e-3);
        p.set_frozen(false);
        p.set_move_scale(MoveScale {
            run: 2.0,
            jump: 1.0,
        });
        for t in 41..=100 {
            p.predict(cmd(t, 0, 127)).unwrap();
        }
        let v = p.state().unwrap().velocity;
        assert!((v.length() - 2.0 * MoveParams::default().run_speed).abs() < 0.05, "{v}");
    }

    #[test]
    fn large_corrections_snap() {
        let mut p = started(1);
        for t in 1..=10 {
            p.predict(cmd(t, 0, 127)).unwrap();
        }
        let mut truth = p.history().get(Tick(5)).unwrap().state;
        truth.position.z -= 3.0;
        let out = p.reconcile(&snapshot(6, 5, truth)).unwrap();
        assert!(out.corrected && out.snapped);
        assert_eq!(p.visual_offset().value(), Vec3::ZERO);
        assert_eq!(p.display_position(), Some(p.state().unwrap().position));
    }

    #[test]
    fn velocity_and_weapon_mismatches_correct_too() {
        let mut p = started(1);
        for t in 1..=10 {
            p.predict(cmd(t, 0, 127)).unwrap();
        }
        let mut truth = p.history().get(Tick(4)).unwrap().state;
        truth.velocity.z += 0.2;
        assert!(p.reconcile(&snapshot(5, 4, truth)).unwrap().corrected);
        let mut truth = p.history().get(Tick(6)).unwrap().state;
        truth.weapon = 2;
        assert!(p.reconcile(&snapshot(7, 6, truth)).unwrap().corrected);
        // Within tolerance: 4 mm and 3 cm/s off.
        let mut truth = p.history().get(Tick(8)).unwrap().state;
        truth.position.x += 0.004;
        truth.velocity.x += 0.03;
        assert!(!p.reconcile(&snapshot(9, 8, truth)).unwrap().corrected);
    }

    #[test]
    fn snapshots_outside_the_history_are_skipped() {
        let mut p = started(100);
        for t in 100..110 {
            p.predict(cmd(t, 0, 0)).unwrap();
        }
        let s = PredictedState::at_rest(SPAWN);
        assert!(p.reconcile(&snapshot(50, 40, s)).is_none());
        assert!(p.reconcile(&snapshot(200, 109, s)).is_none());
    }

    #[test]
    fn packets_carry_the_newest_unacknowledged_commands() {
        let mut p = started(1);
        assert!(p.unacknowledged().is_empty());
        p.predict(cmd(1, 0, 0)).unwrap();
        assert_eq!(p.unacknowledged(), vec![cmd(1, 0, 0)]);
        for t in 2..=10 {
            p.predict(cmd(t, 0, 0)).unwrap();
        }
        let ticks =
            |p: &Prediction| -> Vec<u32> { p.unacknowledged().iter().map(|c| c.tick.0).collect() };
        assert_eq!(ticks(&p), [8, 9, 10]);
        let s = p.history().get(Tick(8)).unwrap().state;
        p.reconcile(&snapshot(10, 9, s));
        assert_eq!(ticks(&p), [10]);
    }
}

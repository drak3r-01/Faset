//! What a mode sees and can do: [`ModeCtx`] (server) and [`HudCtx`] (client).

use std::any::Any;

use faset_core::glam::Vec3;
use faset_core::{NetId, Tick, DT};
use faset_protocol::InputCommand;
use faset_sim::{World, ZoneEvent};

use crate::descriptor::Settings;
use crate::event::GameEvent;
use crate::message::{ModeId, ModeMessage};
use crate::phase::Phase;
use crate::services::{BotControl, ScoreTable, Services, SpawnPoints, TeamId, Teams, Timers};

/// A mode objective of the map: an axis-aligned box with an id (the map
/// format's `Objective { id }` zones, decision: boxes only).
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectiveZone {
    pub id: String,
    pub min: Vec3,
    pub max: Vec3,
}

impl ObjectiveZone {
    /// Whether `point` is inside the box, faces included.
    #[must_use]
    pub fn contains(&self, point: Vec3) -> bool {
        point.cmpge(self.min).all() && point.cmple(self.max).all()
    }
}

/// A pickup placed in the map file (lot 4.11): what it is, by name, and
/// where it stands. Names are the map format's (`health`, `armor`, `ammo`,
/// `weapon:<id>`); the mode decides what each gives.
#[derive(Debug, Clone, PartialEq)]
pub struct MapPickup {
    pub kind: String,
    /// Centre of the trigger, metres.
    pub position: Vec3,
}

/// Something a mode asks the host to do to the world (lot 4.11), drained
/// with [`Match::take_effects`](crate::Match::take_effects).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HostEffect {
    /// Add `delta_v` (m/s) to the player's velocity: knockback, explosive
    /// jump.
    Push { player: NetId, delta_v: Vec3 },
    /// Multiply the controller's run and jump speeds for the player (class
    /// speed, timed bonuses); stays until changed.
    MoveScale { player: NetId, run: f32, jump: f32 },
}

/// What the host gives a match for one tick besides the world (lot 4.11).
#[derive(Default)]
pub struct HostFrame<'a> {
    /// The command each player simulated this tick, ascending id.
    pub commands: &'a [(NetId, InputCommand)],
    /// Zone events of this tick, from `World::drain_zone_events`.
    pub zone_events: &'a [(NetId, ZoneEvent)],
    /// Host-specific services a mode may know about (the server passes its
    /// lag compensation this way), read with [`ModeCtx::host`].
    pub host: Option<&'a mut dyn Any>,
}

/// What a mode call produces besides its own state.
#[derive(Debug, Default)]
pub(crate) struct Outbox {
    /// Mode events to dispatch to the mode at the next tick.
    pub emitted: Vec<GameEvent>,
    /// Every event to replicate, in order.
    pub log: Vec<GameEvent>,
    /// Mode messages to send to every client.
    pub messages: Vec<ModeMessage>,
    /// Requests to the host, in order.
    pub effects: Vec<HostEffect>,
}

/// The server-side context of one mode call.
pub struct ModeCtx<'a> {
    pub(crate) tick: Tick,
    pub(crate) phase: Phase,
    pub(crate) phase_end: Option<Tick>,
    pub(crate) mode: ModeId,
    pub(crate) world: &'a World,
    pub(crate) players: &'a [NetId],
    pub(crate) objectives: &'a [ObjectiveZone],
    pub(crate) pickups: &'a [MapPickup],
    pub(crate) settings: &'a Settings,
    pub(crate) services: &'a mut Services,
    pub(crate) out: &'a mut Outbox,
    pub(crate) commands: &'a [(NetId, InputCommand)],
    pub(crate) zone_events: &'a [(NetId, ZoneEvent)],
    pub(crate) host: Option<&'a mut dyn Any>,
}

impl<'a> ModeCtx<'a> {
    /// Current simulation tick (after the world step).
    #[must_use]
    pub fn tick(&self) -> Tick {
        self.tick
    }

    /// Current phase.
    #[must_use]
    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// End tick of the current phase, if timed.
    #[must_use]
    pub fn phase_end(&self) -> Option<Tick> {
        self.phase_end
    }

    /// Seconds left in the current timed phase (the match timer while
    /// `Playing`).
    #[must_use]
    pub fn time_left(&self) -> Option<f64> {
        self.phase_end
            .map(|end| f64::from(end.0.saturating_sub(self.tick.0)) * f64::from(DT))
    }

    /// The simulation world, read only (borrowed for the whole call, not
    /// just from the context).
    #[must_use]
    pub fn world(&self) -> &'a World {
        self.world
    }

    /// Players in the match, in join order.
    #[must_use]
    pub fn players(&self) -> &[NetId] {
        self.players
    }

    /// Position of a player (capsule centre).
    #[must_use]
    pub fn position(&self, player: NetId) -> Option<Vec3> {
        self.world.position(player)
    }

    /// The command `player` simulated this tick, if the host gave one.
    #[must_use]
    pub fn command(&self, player: NetId) -> Option<&InputCommand> {
        self.commands
            .iter()
            .find(|(id, _)| *id == player)
            .map(|(_, c)| c)
    }

    /// Every command of this tick, ascending id.
    #[must_use]
    pub fn commands(&self) -> &'a [(NetId, InputCommand)] {
        self.commands
    }

    /// Zone events of this tick (kill volumes, pads, objectives).
    #[must_use]
    pub fn zone_events(&self) -> &'a [(NetId, ZoneEvent)] {
        self.zone_events
    }

    /// Host services of type `T`, if the host passed them (see
    /// [`HostFrame::host`]).
    pub fn host<T: 'static>(&mut self) -> Option<&mut T> {
        self.host.as_mut()?.downcast_mut::<T>()
    }

    /// Every objective zone of the map.
    #[must_use]
    pub fn objectives(&self) -> &[ObjectiveZone] {
        self.objectives
    }

    /// The objective zone `id`.
    #[must_use]
    pub fn objective(&self, id: &str) -> Option<&ObjectiveZone> {
        self.objectives.iter().find(|z| z.id == id)
    }

    /// Pickups placed in the map file.
    #[must_use]
    pub fn pickups(&self) -> &[MapPickup] {
        self.pickups
    }

    /// Match settings, resolved against the mode's schema.
    #[must_use]
    pub fn settings(&self) -> &Settings {
        self.settings
    }

    /// Spawn and respawn points.
    pub fn spawns(&mut self) -> &mut SpawnPoints {
        &mut self.services.spawns
    }

    /// Teams.
    pub fn teams(&mut self) -> &mut Teams {
        &mut self.services.teams
    }

    /// The score table, read only: change scores with
    /// [`add_score`](Self::add_score) so clients are told.
    #[must_use]
    pub fn scores(&self) -> &ScoreTable {
        &self.services.scores
    }

    /// Adds `delta` to a score, replicates the change and returns the new
    /// score.
    pub fn add_score(&mut self, player: NetId, delta: i32) -> i32 {
        let score = self.services.scores.add(player, delta);
        self.out.log.push(GameEvent::ScoreChanged { player, score });
        score
    }

    /// Named timers.
    pub fn timers(&mut self) -> &mut Timers {
        &mut self.services.timers
    }

    /// Bot control (stub).
    pub fn bots(&mut self) -> &mut BotControl {
        &mut self.services.bots
    }

    /// Adds a line to the event feed and replicates it.
    pub fn feed(&mut self, text: impl Into<String>) {
        let text = text.into();
        self.services.feed.push(self.tick, text.clone());
        self.out.log.push(GameEvent::Feed(text));
    }

    /// Emits an event: replicated, and given back to the mode's
    /// [`on_event`](crate::GameMode::on_event) at the next tick.
    pub fn emit(&mut self, event: GameEvent) {
        self.out.emitted.push(event.clone());
        self.out.log.push(event);
    }

    /// Replicates an event to the clients without giving it back to the
    /// mode (projectiles, explosions, hit cues).
    pub fn broadcast(&mut self, event: GameEvent) {
        self.out.log.push(event);
    }

    /// Sends mode-defined bytes to every client, as a [`ModeMessage`].
    pub fn send(&mut self, bytes: Vec<u8>) {
        self.out.messages.push(ModeMessage {
            mode: self.mode,
            bytes,
        });
    }

    /// Asks the host to add `delta_v` to the player's velocity.
    pub fn push_player(&mut self, player: NetId, delta_v: Vec3) {
        self.out.effects.push(HostEffect::Push { player, delta_v });
    }

    /// Asks the host to scale the player's run and jump speeds.
    pub fn set_move_scale(&mut self, player: NetId, run: f32, jump: f32) {
        self.out
            .effects
            .push(HostEffect::MoveScale { player, run, jump });
    }
}

/// One row of the client's score table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HudScore {
    pub player: NetId,
    pub score: i32,
    pub team: Option<TeamId>,
}

/// The client-side HUD context: what the framework knows about the match
/// (lot 4.11: phase, match timer, scores, local player) and the text lines
/// the mode draws. The `ui` lot replaces the lines with real drawing.
#[derive(Debug, Clone, Default)]
pub struct HudCtx {
    lines: Vec<String>,
    /// The local player.
    pub player: Option<NetId>,
    /// Current phase, once the server told.
    pub phase: Option<Phase>,
    /// Seconds left in the current timed phase.
    pub time_left: Option<f32>,
    /// Scores, best first (ties by ascending id).
    pub scores: Vec<HudScore>,
}

impl HudCtx {
    /// An empty HUD.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a line of text.
    pub fn text(&mut self, line: impl Into<String>) {
        self.lines.push(line.into());
    }

    /// Lines drawn so far.
    #[must_use]
    pub fn lines(&self) -> &[String] {
        &self.lines
    }
}

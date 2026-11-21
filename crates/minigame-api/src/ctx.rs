//! What a mode sees and can do: [`ModeCtx`] (server) and [`HudCtx`] (client).

use faset_core::glam::Vec3;
use faset_core::{NetId, Tick};
use faset_sim::World;

use crate::descriptor::Settings;
use crate::event::GameEvent;
use crate::message::{ModeId, ModeMessage};
use crate::phase::Phase;
use crate::services::{BotControl, ScoreTable, Services, SpawnPoints, Teams, Timers};

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

/// What a mode call produces besides its own state.
#[derive(Debug, Default)]
pub(crate) struct Outbox {
    /// Mode events to dispatch to the mode at the next tick.
    pub emitted: Vec<GameEvent>,
    /// Every event to replicate, in order.
    pub log: Vec<GameEvent>,
    /// Mode messages to send to every client.
    pub messages: Vec<ModeMessage>,
}

/// The server-side context of one mode call.
pub struct ModeCtx<'a> {
    pub(crate) tick: Tick,
    pub(crate) phase: Phase,
    pub(crate) mode: ModeId,
    pub(crate) world: &'a World,
    pub(crate) players: &'a [NetId],
    pub(crate) objectives: &'a [ObjectiveZone],
    pub(crate) settings: &'a Settings,
    pub(crate) services: &'a mut Services,
    pub(crate) out: &'a mut Outbox,
}

impl ModeCtx<'_> {
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

    /// The simulation world, read only.
    #[must_use]
    pub fn world(&self) -> &World {
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

    /// Sends mode-defined bytes to every client, as a [`ModeMessage`].
    pub fn send(&mut self, bytes: Vec<u8>) {
        self.out.messages.push(ModeMessage {
            mode: self.mode,
            bytes,
        });
    }
}

/// The client-side HUD context. Stub: the `ui` lot replaces the text lines
/// with real drawing.
#[derive(Debug, Clone, Default)]
pub struct HudCtx {
    lines: Vec<String>,
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

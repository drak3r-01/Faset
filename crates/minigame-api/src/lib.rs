//! Mini-game framework (plan section 14).
//!
//! A mode is a crate implementing [`GameMode`] (server) and
//! [`GameModeClient`] (client), registered once at startup with
//! [`Registry::register`]. Adding a mode touches neither the engine nor the
//! protocol: mode data travels as opaque bytes, in [`ModeMessage`]s and in
//! [`GameMode::replicated_state`].
//!
//! [`Match`] is the server-side driver: it owns the mode instance, runs the
//! [`Phase`] cycle (`Warmup`, `Countdown`, `Playing`, `RoundEnd`, `MapChange`,
//! `Loading`, back to `Warmup`) on simulation ticks and gives the mode its
//! [`ModeCtx`] with the common [`services`]: spawn points, teams, score
//! table, timers, event feed and bot control (stub).
//!
//! The host (the server) calls [`Match::tick`] once per simulation tick
//! after the world step, forwards joins and leaves, and replicates
//! [`Match::state`], [`Match::drain_events`] and [`Match::drain_messages`].
#![forbid(unsafe_code)]

mod ctx;
mod descriptor;
mod event;
mod message;
mod mirror;
mod phase;
mod registry;
mod runner;
pub mod services;

pub use ctx::{HostEffect, HostFrame, HudCtx, HudScore, MapPickup, ModeCtx, ObjectiveZone};
pub use descriptor::{
    ModeDescriptor, SettingKind, SettingSpec, SettingValue, Settings, SettingsError,
    TIME_LIMIT_SETTING,
};
pub use event::{EndReason, GameEvent, MatchResult};
pub use message::{DecodeError, MatchState, ModeId, ModeMessage};
pub use mirror::MatchMirror;
pub use phase::{Phase, COUNTDOWN_TICKS, LOADING_TICKS, MAP_CHANGE_TICKS, ROUND_END_TICKS};
pub use registry::{ModeEntry, Registry, RegistryError};
pub use runner::{Match, MatchConfig, DEFAULT_MIN_PLAYERS, DEFAULT_TIME_LIMIT_TICKS};
pub use services::{
    BotControl, EventFeed, FeedEntry, ScoreTable, Services, SpawnPoints, TeamId, Teams, Timers,
};

use faset_core::NetId;

/// A player, as identified by the server.
pub type PlayerId = NetId;

/// What the host needs to know about a player of a mode with health (lot
/// 4.11): snapshots carry health and the alive flag, a dead player does
/// not move, bots tell friend from foe.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vitals {
    pub health: f32,
    pub max_health: f32,
    pub alive: bool,
    pub team: Option<TeamId>,
}

/// Server side of a mode. The server is authoritative: only this side
/// decides scores, phases and the result.
pub trait GameMode {
    /// Static description: id, name, maps, bots, settings schema.
    fn descriptor() -> ModeDescriptor
    where
        Self: Sized;
    /// The `Playing` phase starts (after the countdown).
    fn on_match_start(&mut self, ctx: &mut ModeCtx);
    /// A player entered the match (any phase).
    fn on_player_join(&mut self, ctx: &mut ModeCtx, player: PlayerId);
    /// A player left the match (any phase).
    fn on_player_leave(&mut self, ctx: &mut ModeCtx, player: PlayerId);
    /// One simulation tick of the `Playing` phase.
    fn on_tick(&mut self, ctx: &mut ModeCtx);
    /// A game event: framework events at once, the mode's own events
    /// ([`ModeCtx::emit`]) at the start of the next tick.
    fn on_event(&mut self, ctx: &mut ModeCtx, event: &GameEvent);
    /// `Some` once the win condition is met; ends the `Playing` phase.
    fn result(&self) -> Option<MatchResult>;
    /// Mode state sent to every client, read by
    /// [`GameModeClient::apply_state`].
    fn replicated_state(&self) -> Vec<u8>;
    /// A message a client sent to the mode (lot 4.11): the bytes of a
    /// `ModeMessage` addressed to this mode, any phase, delivered at the
    /// start of the next match tick in arrival order. Ignored by default.
    fn on_client_message(&mut self, _ctx: &mut ModeCtx, _player: PlayerId, _bytes: &[u8]) {}
    /// Health and the alive flag of a player, for modes that have them
    /// (lot 4.11). `None` by default: the host treats the player as alive.
    fn vitals(&self, _player: PlayerId) -> Option<Vitals> {
        None
    }
}

/// Client side of a mode: mirrors the replicated state and draws the HUD.
pub trait GameModeClient {
    /// The latest [`GameMode::replicated_state`] from the server.
    fn apply_state(&mut self, bytes: &[u8]);
    /// A game event replicated by the server.
    fn on_event(&mut self, event: &GameEvent);
    /// Draws the mode's HUD.
    fn draw_hud(&mut self, ui: &mut HudCtx);
    /// What the replicated state says of a player (lot 4.11). `None` by
    /// default.
    fn vitals(&self, _player: PlayerId) -> Option<Vitals> {
        None
    }
    /// Run and jump speed multipliers of a player as the replicated state
    /// shows them, so the client predicts with the server's controller
    /// (lot 4.11). `(1, 1)` by default.
    fn move_scale(&self, _player: PlayerId) -> (f32, f32) {
        (1.0, 1.0)
    }
    /// The mode's client as `Any`, so the application can reach what only
    /// the mode's crate knows (the HUD model, the pickups; lot 6.6).
    /// `None` by default.
    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        None
    }
}

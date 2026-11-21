//! Game events and match results.

use faset_core::{NetId, Tick};

use crate::phase::Phase;
use crate::services::TeamId;

/// Something that happened in a match. Replicated to clients and given to
/// [`GameMode::on_event`](crate::GameMode::on_event). New variants arrive
/// with later lots (damage, pickups), hence `non_exhaustive`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum GameEvent {
    /// A player entered the match.
    PlayerJoined(NetId),
    /// A player left the match.
    PlayerLeft(NetId),
    /// The match entered `phase`; `end` is the tick it ends at, if timed.
    PhaseChanged { phase: Phase, end: Option<Tick> },
    /// A player's score changed to `score`.
    ScoreChanged { player: NetId, score: i32 },
    /// The `Playing` phase ended with this result.
    MatchEnded(MatchResult),
    /// The rotation moved to map `map`.
    MapChanged { map: String },
    /// A line of the event feed.
    Feed(String),
    /// A mode-defined event, opaque to the framework.
    Mode(Vec<u8>),
}

/// Why the `Playing` phase ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndReason {
    /// The mode's win condition was met.
    WinCondition,
    /// The time limit ran out (winners from the score table).
    TimeLimit,
}

/// The outcome of a match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchResult {
    /// Winning players, several on a tie; empty for a draw with no score.
    pub winners: Vec<NetId>,
    /// Winning team, for team modes.
    pub winning_team: Option<TeamId>,
    /// Why the match ended.
    pub reason: EndReason,
}

impl MatchResult {
    /// One player won by the win condition.
    #[must_use]
    pub fn winner(player: NetId) -> Self {
        Self {
            winners: vec![player],
            winning_team: None,
            reason: EndReason::WinCondition,
        }
    }
}

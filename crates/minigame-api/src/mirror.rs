//! [`MatchMirror`]: the client-side copy of a match (lot 4.11).

use std::collections::BTreeMap;

use faset_core::{NetId, Tick, DT};

use crate::ctx::{HudCtx, HudScore};
use crate::event::GameEvent;
use crate::message::{DecodeError, MatchState, ModeId};
use crate::phase::Phase;
use crate::registry::ModeEntry;
use crate::{GameModeClient, Vitals};

/// What a client knows of the match it plays: the mode's client instance,
/// the replicated [`MatchState`], the score table rebuilt from the
/// `ScoreChanged` events, and the events seen so far.
pub struct MatchMirror {
    mode: ModeId,
    client: Box<dyn GameModeClient>,
    state: Option<MatchState>,
    scores: BTreeMap<NetId, i32>,
    events: Vec<GameEvent>,
}

impl std::fmt::Debug for MatchMirror {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MatchMirror")
            .field("mode", &self.mode)
            .field("phase", &self.phase())
            .field("scores", &self.scores)
            .field("events", &self.events.len())
            .finish_non_exhaustive()
    }
}

impl MatchMirror {
    /// A mirror of `entry`'s mode, before any state arrived.
    #[must_use]
    pub fn new(entry: &ModeEntry) -> Self {
        Self {
            mode: entry.id,
            client: entry.create_client(),
            state: None,
            scores: BTreeMap::new(),
            events: Vec::new(),
        }
    }

    /// The mode's id.
    #[must_use]
    pub fn mode_id(&self) -> ModeId {
        self.mode
    }

    /// Applies a replicated [`MatchState`] (its wire bytes).
    ///
    /// # Errors
    /// Malformed bytes; the mirror is left unchanged.
    pub fn apply_state_bytes(&mut self, bytes: &[u8]) -> Result<(), DecodeError> {
        let state = MatchState::decode(bytes)?;
        self.client.apply_state(&state.mode_state);
        self.state = Some(state);
        Ok(())
    }

    /// Applies one replicated event.
    pub fn on_event(&mut self, event: GameEvent) {
        match &event {
            GameEvent::ScoreChanged { player, score } => {
                self.scores.insert(*player, *score);
            }
            GameEvent::PlayerLeft(p) => {
                self.scores.remove(p);
            }
            GameEvent::PhaseChanged { phase, end } => {
                if let Some(s) = self.state.as_mut() {
                    s.phase = *phase;
                    s.phase_end = *end;
                }
                if *phase == Phase::Playing {
                    self.scores.values_mut().for_each(|s| *s = 0);
                }
            }
            _ => {}
        }
        self.client.on_event(&event);
        self.events.push(event);
    }

    /// The latest replicated state.
    #[must_use]
    pub fn state(&self) -> Option<&MatchState> {
        self.state.as_ref()
    }

    /// Current phase, once known.
    #[must_use]
    pub fn phase(&self) -> Option<Phase> {
        self.state.as_ref().map(|s| s.phase)
    }

    /// Whether the phase lets players move (unknown: yes).
    #[must_use]
    pub fn movement_allowed(&self) -> bool {
        self.phase().is_none_or(Phase::movement_allowed)
    }

    /// Score of a player.
    #[must_use]
    pub fn score(&self, player: NetId) -> i32 {
        self.scores.get(&player).copied().unwrap_or(0)
    }

    /// Every event received, oldest first.
    #[must_use]
    pub fn events(&self) -> &[GameEvent] {
        &self.events
    }

    /// The mode's client side.
    #[must_use]
    pub fn client(&self) -> &dyn GameModeClient {
        self.client.as_ref()
    }

    /// The mode's client side, mutable.
    pub fn client_mut(&mut self) -> &mut dyn GameModeClient {
        self.client.as_mut()
    }

    /// What the mode's replicated state says of `player`.
    #[must_use]
    pub fn vitals(&self, player: NetId) -> Option<Vitals> {
        self.client.vitals(player)
    }

    /// Draws the HUD of tick `now` for `player`: the framework fills the
    /// phase, the timer and the scores, then the mode draws.
    pub fn draw_hud(&mut self, player: Option<NetId>, now: Tick) -> HudCtx {
        let mut ui = HudCtx::new();
        ui.player = player;
        ui.phase = self.phase();
        ui.time_left = self
            .state
            .as_ref()
            .and_then(|s| s.phase_end)
            .map(|end| end.0.saturating_sub(now.0) as f32 * DT);
        let mut rows: Vec<HudScore> = self
            .scores
            .iter()
            .map(|(&p, &score)| HudScore {
                player: p,
                score,
                team: self.client.vitals(p).and_then(|v| v.team),
            })
            .collect();
        rows.sort_by_key(|r| (std::cmp::Reverse(r.score), r.player));
        ui.scores = rows;
        self.client.draw_hud(&mut ui);
        ui
    }
}

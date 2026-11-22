//! Witness mode `race`: a race between two objective zones, A then B.
//!
//! The first player to touch zone B after zone A wins and the match ends
//! on that result. Movement is the basic player motion of the simulation;
//! no weapons, no bonuses. The mode is built only on `minigame-api`: it
//! needs no change to the engine, the protocol or the framework.
//!
//! Zones are the map's `Objective` zones with ids [`ZONE_A`] and
//! [`ZONE_B`] (decision).
#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use faset_core::NetId;
use faset_minigame_api::{
    GameEvent, GameMode, GameModeClient, HudCtx, MatchResult, ModeCtx, ModeDescriptor, SettingKind,
    SettingSpec, TIME_LIMIT_SETTING,
};

pub use shared::Stage;

/// Objective id of the first zone.
pub const ZONE_A: &str = "race_a";
/// Objective id of the finish zone.
pub const ZONE_B: &str = "race_b";

/// Race rules shared by the server and client prediction.
pub mod shared {
    use faset_core::glam::Vec3;
    use faset_minigame_api::ObjectiveZone;

    /// How far a player got.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub enum Stage {
        /// Has not touched zone A yet.
        #[default]
        Start,
        /// Touched zone A; zone B now finishes.
        ReachedA,
        /// Touched zone B after zone A.
        Finished,
    }

    impl Stage {
        /// Wire code.
        #[must_use]
        pub fn to_u8(self) -> u8 {
            match self {
                Self::Start => 0,
                Self::ReachedA => 1,
                Self::Finished => 2,
            }
        }

        /// Inverse of [`to_u8`](Self::to_u8).
        #[must_use]
        pub fn from_u8(code: u8) -> Option<Self> {
            Some(match code {
                0 => Self::Start,
                1 => Self::ReachedA,
                2 => Self::Finished,
                _ => return None,
            })
        }
    }

    /// The stage after one tick at `position`. Zone B before zone A counts
    /// for nothing.
    #[must_use]
    pub fn advance(stage: Stage, position: Vec3, a: &ObjectiveZone, b: &ObjectiveZone) -> Stage {
        match stage {
            Stage::Start if a.contains(position) => Stage::ReachedA,
            Stage::ReachedA if b.contains(position) => Stage::Finished,
            s => s,
        }
    }
}

/// The race mode, server and client side.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RaceMode {
    progress: BTreeMap<NetId, Stage>,
    winner: Option<NetId>,
}

impl RaceMode {
    /// Stage of a player.
    #[must_use]
    pub fn stage(&self, player: NetId) -> Option<Stage> {
        self.progress.get(&player).copied()
    }

    /// The winner, once there is one.
    #[must_use]
    pub fn winner(&self) -> Option<NetId> {
        self.winner
    }

    /// Layout: winner flag (1), winner id (2, LE), player count (2, LE),
    /// then per player id (2, LE) and stage (1).
    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(5 + 3 * self.progress.len());
        out.push(u8::from(self.winner.is_some()));
        out.extend_from_slice(&self.winner.unwrap_or_default().0.to_le_bytes());
        let count = u16::try_from(self.progress.len()).unwrap_or(u16::MAX);
        out.extend_from_slice(&count.to_le_bytes());
        for (id, stage) in self.progress.iter().take(usize::from(count)) {
            out.extend_from_slice(&id.0.to_le_bytes());
            out.push(stage.to_u8());
        }
        out
    }

    fn decode(bytes: &[u8]) -> Option<Self> {
        let u16_at = |i: usize| Some(u16::from_le_bytes([*bytes.get(i)?, *bytes.get(i + 1)?]));
        let flag = *bytes.first()?;
        let winner = (flag != 0).then_some(NetId(u16_at(1)?));
        let count = usize::from(u16_at(3)?);
        if bytes.len() != 5 + 3 * count {
            return None;
        }
        let mut progress = BTreeMap::new();
        for i in 0..count {
            let at = 5 + 3 * i;
            progress.insert(NetId(u16_at(at)?), Stage::from_u8(bytes[at + 2])?);
        }
        Some(Self { progress, winner })
    }
}

impl GameMode for RaceMode {
    fn descriptor() -> ModeDescriptor {
        ModeDescriptor {
            id: "race",
            name: "Race",
            map_labels: &["race"],
            bots: false,
            settings: vec![SettingSpec {
                key: TIME_LIMIT_SETTING,
                label: "Time limit (s)",
                kind: SettingKind::Int {
                    min: 30,
                    max: 1800,
                    default: 300,
                },
            }],
        }
    }

    fn on_match_start(&mut self, ctx: &mut ModeCtx) {
        self.winner = None;
        self.progress = ctx.players().iter().map(|&p| (p, Stage::Start)).collect();
        if ctx.objective(ZONE_A).is_none() || ctx.objective(ZONE_B).is_none() {
            tracing::warn!("race: map has no `{ZONE_A}`/`{ZONE_B}` objective zones");
        }
        ctx.feed("Race: touch A, then B");
    }

    fn on_player_join(&mut self, _ctx: &mut ModeCtx, player: NetId) {
        self.progress.entry(player).or_default();
    }

    fn on_player_leave(&mut self, _ctx: &mut ModeCtx, player: NetId) {
        self.progress.remove(&player);
    }

    fn on_tick(&mut self, ctx: &mut ModeCtx) {
        if self.winner.is_some() {
            return;
        }
        let (Some(a), Some(b)) = (ctx.objective(ZONE_A), ctx.objective(ZONE_B)) else {
            return;
        };
        let (a, b) = (a.clone(), b.clone());
        // Ascending id: on a same-tick finish the lowest id wins (decision).
        let players: Vec<NetId> = self.progress.keys().copied().collect();
        for player in players {
            let Some(pos) = ctx.position(player) else {
                continue;
            };
            let before = self.progress[&player];
            let after = shared::advance(before, pos, &a, &b);
            if after == before {
                continue;
            }
            self.progress.insert(player, after);
            match after {
                Stage::ReachedA => ctx.feed(format!("Player {} reached A", player.0)),
                Stage::Finished => {
                    self.winner = Some(player);
                    ctx.add_score(player, 1);
                    ctx.feed(format!("Player {} wins the race", player.0));
                    return;
                }
                Stage::Start => {}
            }
        }
    }

    fn on_event(&mut self, _ctx: &mut ModeCtx, _event: &GameEvent) {}

    fn result(&self) -> Option<MatchResult> {
        self.winner.map(MatchResult::winner)
    }

    fn replicated_state(&self) -> Vec<u8> {
        self.encode()
    }
}

impl GameModeClient for RaceMode {
    fn apply_state(&mut self, bytes: &[u8]) {
        if let Some(state) = Self::decode(bytes) {
            *self = state;
        }
    }

    fn on_event(&mut self, _event: &GameEvent) {}

    fn draw_hud(&mut self, ui: &mut HudCtx) {
        if let Some(w) = self.winner {
            ui.text(format!("Winner: player {}", w.0));
        }
        for (id, stage) in &self.progress {
            let label = match stage {
                Stage::Start => "go to A",
                Stage::ReachedA => "go to B",
                Stage::Finished => "finished",
            };
            ui.text(format!("Player {}: {label}", id.0));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use faset_core::glam::Vec3;
    use faset_minigame_api::ObjectiveZone;

    #[test]
    fn b_before_a_counts_for_nothing() {
        let a = ObjectiveZone {
            id: ZONE_A.into(),
            min: Vec3::ZERO,
            max: Vec3::ONE,
        };
        let b = ObjectiveZone {
            id: ZONE_B.into(),
            min: Vec3::splat(5.0),
            max: Vec3::splat(6.0),
        };
        let in_a = Vec3::splat(0.5);
        let in_b = Vec3::splat(5.5);
        assert_eq!(shared::advance(Stage::Start, in_b, &a, &b), Stage::Start);
        let s = shared::advance(Stage::Start, in_a, &a, &b);
        assert_eq!(s, Stage::ReachedA);
        assert_eq!(shared::advance(s, in_b, &a, &b), Stage::Finished);
    }

    #[test]
    fn state_round_trips_and_draws() {
        let mut m = RaceMode::default();
        m.progress.insert(NetId(1), Stage::ReachedA);
        m.progress.insert(NetId(7), Stage::Finished);
        m.winner = Some(NetId(7));
        let mut c = RaceMode::default();
        c.apply_state(&m.replicated_state());
        assert_eq!(c, m);
        let mut hud = HudCtx::new();
        c.draw_hud(&mut hud);
        assert_eq!(hud.lines()[0], "Winner: player 7");
        assert_eq!(RaceMode::decode(&[1, 0]), None);
    }
}

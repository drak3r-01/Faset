//! [`ModeServices`]: what the server lends a hosted mode each tick (lot
//! 4.11), through `minigame-api`'s `HostFrame::host`.

use std::collections::BTreeMap;

use faset_core::NetId;
use faset_sim::StaticWorld;

use crate::lag_compensation::{FireCommand, LagCompensation, ShotOutcome};

/// The server's hitbox history and the players' round trips. A mode reads
/// it with `ModeCtx::host::<ModeServices>()` to validate shots as the
/// shooter saw them (lot 1.7). Absent outside a server (tests, headless
/// harnesses).
#[derive(Debug, Clone)]
pub struct ModeServices {
    lag: LagCompensation,
    rtt: BTreeMap<NetId, f32>,
}

impl Default for ModeServices {
    fn default() -> Self {
        Self {
            lag: LagCompensation::new(),
            rtt: BTreeMap::new(),
        }
    }
}

impl ModeServices {
    /// The hitbox history of every player.
    #[must_use]
    pub fn lag(&self) -> &LagCompensation {
        &self.lag
    }

    /// The hitbox history, mutable (rewinds place the hitboxes).
    pub fn lag_mut(&mut self) -> &mut LagCompensation {
        &mut self.lag
    }

    /// Smoothed round trip of a player in seconds: 0 for a bot or an
    /// unknown player.
    #[must_use]
    pub fn rtt(&self, player: NetId) -> f32 {
        self.rtt.get(&player).copied().unwrap_or(0.0)
    }

    pub(crate) fn set_rtt(&mut self, rtt: BTreeMap<NetId, f32>) {
        self.rtt = rtt;
    }

    /// Casts a hitscan shot of `shooter` with lag compensation and its
    /// round trip (see [`LagCompensation::rewind_and_cast`]).
    pub fn rewind_and_cast(
        &mut self,
        statics: &StaticWorld,
        shooter: NetId,
        fire: &FireCommand,
    ) -> Option<ShotOutcome> {
        let rtt = self.rtt(shooter);
        self.lag.rewind_and_cast(statics, shooter, fire, rtt)
    }
}

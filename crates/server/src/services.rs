//! [`ModeServices`]: what the server lends a hosted mode each tick (lot
//! 4.11), through `minigame-api`'s `HostFrame::host`.

use std::collections::BTreeMap;

use faset_core::NetId;
use faset_sim::StaticWorld;

use crate::lag_compensation::{FireCommand, LagCompensation, ShotOutcome};

/// How far behind the present a client shows the other players, seconds
/// (decision: about two snapshot intervals of interpolation plus jitter).
pub const VIEW_DELAY: f32 = 0.1;

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

    /// The instant a shot of `player` fired at tick `now` is rewound to
    /// (lot 4.11): a human saw the other players one round trip plus the
    /// interpolation delay ([`VIEW_DELAY`]) in the past; a bot (no
    /// connection) sees the present. The command carries no view tick
    /// (decision: humans and bots fire through the same input bit), so the
    /// server estimates it; [`LagCompensation::resolve_view_time`] still
    /// bounds the rewind.
    #[must_use]
    pub fn view_instant(&self, player: NetId, now: faset_core::Tick) -> (faset_core::Tick, f32) {
        let delay = self
            .rtt
            .get(&player)
            .map_or(0.0, |rtt| f64::from(rtt + VIEW_DELAY));
        let t = (f64::from(now.0) - delay / f64::from(faset_core::DT)).max(0.0);
        let tick = t.floor();
        (faset_core::Tick(tick as u32), (t - tick) as f32)
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

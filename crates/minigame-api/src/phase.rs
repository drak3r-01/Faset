//! Match phases and their durations, in simulation ticks at 60 Hz.

use faset_core::TICK_RATE;

const SECOND: u32 = TICK_RATE as u32;

/// `Countdown` lasts 3 s.
pub const COUNTDOWN_TICKS: u32 = 3 * SECOND;
/// `RoundEnd` (score table) lasts 8 s.
pub const ROUND_END_TICKS: u32 = 8 * SECOND;
/// `MapChange` lasts 1 s (decision: announce the next map before loading).
pub const MAP_CHANGE_TICKS: u32 = SECOND;
/// `Loading` lasts 1 s (decision: fixed until real map loading lands).
pub const LOADING_TICKS: u32 = SECOND;

/// The phase of a match. The server is authoritative; the end tick of each
/// timed phase is replicated in [`MatchState`](crate::MatchState).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Phase {
    /// Waiting for players, free movement.
    Warmup,
    /// 3 s before play starts; movement frozen.
    Countdown,
    /// Until the time limit ends or the win condition is met.
    Playing,
    /// Score table, 8 s.
    RoundEnd,
    /// Switch to the next map of the rotation.
    MapChange,
    /// Loading the map, then back to `Warmup`.
    Loading,
}

impl Phase {
    /// Fixed duration in ticks; `None` for `Warmup` (waits for players) and
    /// `Playing` (time limit from the match settings).
    #[must_use]
    pub fn fixed_ticks(self) -> Option<u32> {
        match self {
            Self::Countdown => Some(COUNTDOWN_TICKS),
            Self::RoundEnd => Some(ROUND_END_TICKS),
            Self::MapChange => Some(MAP_CHANGE_TICKS),
            Self::Loading => Some(LOADING_TICKS),
            Self::Warmup | Self::Playing => None,
        }
    }

    /// Whether players may move in this phase.
    #[must_use]
    pub fn movement_allowed(self) -> bool {
        matches!(self, Self::Warmup | Self::Playing)
    }

    /// Wire code (as in the protocol's `Event::Phase`).
    #[must_use]
    pub fn to_u8(self) -> u8 {
        match self {
            Self::Warmup => 0,
            Self::Countdown => 1,
            Self::Playing => 2,
            Self::RoundEnd => 3,
            Self::MapChange => 4,
            Self::Loading => 5,
        }
    }

    /// Inverse of [`to_u8`](Self::to_u8).
    #[must_use]
    pub fn from_u8(code: u8) -> Option<Self> {
        Some(match code {
            0 => Self::Warmup,
            1 => Self::Countdown,
            2 => Self::Playing,
            3 => Self::RoundEnd,
            4 => Self::MapChange,
            5 => Self::Loading,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_at_sixty_hertz() {
        assert_eq!(COUNTDOWN_TICKS, 180);
        assert_eq!(ROUND_END_TICKS, 480);
    }

    #[test]
    fn codes_round_trip() {
        for code in 0..6 {
            assert_eq!(Phase::from_u8(code).unwrap().to_u8(), code);
        }
        assert_eq!(Phase::from_u8(6), None);
    }
}

//! Deterministic fixed-step simulation: ECS world, ordered systems, base
//! collision queries and replayable traces.
//!
//! The same code runs on client and server. No hash tables are used; all
//! iteration is by ascending [`faset_core::NetId`].
#![forbid(unsafe_code)]

pub mod collision;
pub mod components;
pub mod controller;
pub mod map_collision;
pub mod rng;
pub mod slide;
pub mod trace;
pub mod world;
pub mod zones;

pub use collision::{RayHit, StaticWorld, SweepHit, Volume};
pub use controller::{step_player, MoveCmd, MoveEvents, MoveParams};
pub use map_collision::MapCollision;
pub use trace::Trace;
pub use world::World;
pub use zones::{ZoneEvent, Zones};

/// Fixed-step accumulator: feeds variable frame times into 1/60 s steps.
///
/// `frame_dt` is clamped to 0.25 s to avoid the spiral of death.
#[derive(Debug, Default, Clone, Copy)]
pub struct Accumulator {
    acc: f32,
}

impl Accumulator {
    /// Adds a frame and returns how many simulation steps to run.
    pub fn advance(&mut self, frame_dt: f32) -> u32 {
        self.acc += frame_dt.min(0.25);
        let mut n = 0;
        while self.acc >= faset_core::DT {
            self.acc -= faset_core::DT;
            n += 1;
        }
        n
    }

    /// Render interpolation factor in [0, 1).
    #[must_use]
    pub fn alpha(&self) -> f32 {
        self.acc / faset_core::DT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accumulator_clamps_long_frames() {
        let mut a = Accumulator::default();
        let n = a.advance(10.0);
        assert!((14..=15).contains(&n)); // 0.25 s of steps
        assert!(a.alpha() < 1.0);
        assert_eq!(a.advance(0.001), 0);
    }
}

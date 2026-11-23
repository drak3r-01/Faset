//! Base types shared by every crate: identifiers, time, maths.
//!
//! Units: metre, second, radian. Right-handed, Y up, -Z forward.
#![forbid(unsafe_code)]

pub use glam;

pub mod bindings;
pub mod settings;
pub mod translations;

/// Fixed simulation step in seconds (60 Hz).
pub const DT: f32 = 1.0 / 60.0;

/// Simulation ticks per second.
pub const TICK_RATE: u8 = 60;

/// Network identifier of an entity, assigned by the server.
///
/// Distinct from ECS entity ids.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct NetId(pub u16);

/// Simulation tick counter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Tick(pub u32);

impl Tick {
    /// The next tick, wrapping on overflow.
    #[must_use]
    pub fn next(self) -> Self {
        Self(self.0.wrapping_add(1))
    }

    /// Time of this tick in seconds since tick 0.
    #[must_use]
    pub fn seconds(self) -> f64 {
        f64::from(self.0) * f64::from(DT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dt_is_sixty_hertz() {
        assert!((DT * 60.0 - 1.0).abs() < 1e-6);
    }

    #[test]
    fn tick_advances_and_wraps() {
        assert_eq!(Tick(1).next(), Tick(2));
        assert_eq!(Tick(u32::MAX).next(), Tick(0));
        assert!((Tick(60).seconds() - 1.0).abs() < 1e-6);
    }
}

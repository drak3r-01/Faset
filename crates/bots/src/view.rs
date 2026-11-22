//! Data exchanged between the host (server loop) and a bot brain.
//!
//! The brain depends on `sim` only: the host copies what it knows about the
//! world into [`RawState`] each tick (friend and foe filtering, weapon
//! numbers, pickups, the objective), and gets an `InputCommand` back through
//! the same input path as a human client.

use faset_core::glam::Vec3;
use faset_core::{NetId, Tick};

use crate::NavGraph;

/// Button bit of the fire intent in `InputCommand::buttons`: the protocol's
/// own fire bit (bit 2, defined by the integration lot 4.11), which the
/// FPS modes read for humans and bots alike.
pub use faset_protocol::BUTTON_FIRE;

/// The bot's own state, always exact (a player knows where it stands).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SelfState {
    /// Feet position.
    pub pos: Vec3,
    pub vel: Vec3,
    /// Eye position.
    pub eye: Vec3,
    /// View yaw, radians (0 faces -Z).
    pub yaw: f32,
    /// View pitch, radians (positive looks up).
    pub pitch: f32,
    pub on_ground: bool,
    pub health: f32,
    pub max_health: f32,
}

/// One enemy as the host knows it (only hostile, living players).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RawEntity {
    pub id: NetId,
    /// Feet position.
    pub pos: Vec3,
    pub vel: Vec3,
}

/// A shot fired somewhere this tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HeardShot {
    pub shooter: NetId,
    pub pos: Vec3,
}

/// What a pickup gives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickupKind {
    Health,
    Weapon,
    Bonus,
}

/// A pickup spot and whether it is ready.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PickupInfo {
    pub kind: PickupKind,
    pub pos: Vec3,
    pub available: bool,
}

/// How the weapon flies. Passed in as data: `bots` does not depend on
/// `mode-fps`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WeaponParams {
    /// Slot sent in the input command.
    pub slot: u8,
    /// Projectile speed, m/s; zero or less means hitscan.
    pub projectile_speed: f32,
    /// Gravity acting on the projectile, m/s^2 (0 for a flat flight).
    pub gravity: f32,
    /// Maximum useful distance, metres.
    pub range: f32,
    /// Fire along the high arc (grenade launcher).
    pub lob: bool,
}

impl WeaponParams {
    /// A hitscan weapon.
    #[must_use]
    pub fn hitscan(range: f32) -> Self {
        Self {
            slot: 0,
            projectile_speed: 0.0,
            gravity: 0.0,
            range,
            lob: false,
        }
    }
}

/// Everything the host feeds a bot each tick.
#[derive(Debug, Clone, Copy)]
pub struct RawState<'a> {
    pub tick: Tick,
    /// Time in seconds, `tick.seconds()`.
    pub time: f64,
    pub me: SelfState,
    /// Living hostile players.
    pub enemies: &'a [RawEntity],
    /// Shots fired this tick (any player; the bot filters by distance).
    pub shots: &'a [HeardShot],
    pub pickups: &'a [PickupInfo],
    /// Where the mode wants the bot to go (flag, zone, ...).
    pub objective: Option<Vec3>,
    pub weapon: WeaponParams,
}

/// What the bot remembers about one enemy, after the reaction delay.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sighting {
    pub id: NetId,
    /// Feet position when observed.
    pub pos: Vec3,
    pub vel: Vec3,
    /// Time of the observation, seconds.
    pub seen_at: f64,
    /// Seen with its own eyes (as opposed to heard) at that time.
    pub seen: bool,
    /// Whether the latest delivered observation still shows it.
    pub visible: bool,
}

/// The perceived state a brain thinks with.
#[derive(Debug, Clone, Copy)]
pub struct BotView<'a> {
    pub tick: Tick,
    pub time: f64,
    pub me: SelfState,
    /// The freshest sighting of each known enemy, after the reaction delay.
    pub sightings: &'a [Sighting],
    pub pickups: &'a [PickupInfo],
    pub objective: Option<Vec3>,
    pub weapon: WeaponParams,
    pub nav: &'a NavGraph,
}

//! ECS components. `NetId` is the network id, distinct from `hecs::Entity`.

use faset_core::glam::Vec3;
pub use faset_core::NetId;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Transform {
    /// Capsule centre in metres.
    pub position: Vec3,
    /// Radians.
    pub yaw: f32,
    pub pitch: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Velocity(pub Vec3);

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PlayerState {
    pub on_ground: bool,
    pub sliding: bool,
    pub slide_timer: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Health(pub i16);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Inventory {
    pub weapon_slot: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WeaponState {
    pub shots_fired: u32,
    pub cooldown_ticks: u16,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Projectile {
    pub ticks_left: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pickup {
    pub kind: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Team(pub u8);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Owner(pub NetId);

/// Standing capsule: radius 0.35 m, total height 1.8 m.
pub const CAPSULE_RADIUS: f32 = 0.35;
pub const STANDING_HEIGHT: f32 = 1.8;
pub const SLIDING_HEIGHT: f32 = 1.0;

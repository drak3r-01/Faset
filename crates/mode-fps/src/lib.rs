//! FPS core (plan lot 4.4): hitscan weapons, spread, recoil, damage chain,
//! respawn choice, and the base mode built on them.
//!
//! * [`weapon`] - [`WeaponDef`], one RON file per weapon.
//! * [`spread`] and [`recoil`] - deterministic cone and view kick; client and
//!   server draw the same bits.
//! * [`shot`] - [`fire`], the pure shot function shared by client prediction
//!   and server validation, and the [`RayCaster`]s that feed it
//!   ([`SceneCaster`] as displayed, [`RewindCaster`] and [`ServerCaster`]
//!   through the lag-compensation history of lot 1.7).
//! * [`damage`] - [`DamageEvent`], falloff and the [`Combatant`] chain.
//! * [`respawn`] - [`choose_spawn`].
//! * [`rules`] and [`mode`] - [`FpsRules`] (shots, kills, respawns) and
//!   [`FpsMode`], its `minigame-api` adapter.
//! * [`message`] - wire layouts of the mode's messages.
//!
//! Time is in seconds as `f64` taken from the tick (`Tick::seconds`), never
//! rounded to ticks. Units: metre, second, radian; -Z forward, Y up.
#![forbid(unsafe_code)]

pub mod damage;
pub mod message;
pub mod mode;
pub mod recoil;
pub mod respawn;
pub mod rules;
pub mod shot;
pub mod spread;
pub mod weapon;

pub use damage::{
    falloff_multiplier, pellet_damage, Combatant, DamageContext, DamageEvent, DamageOutcome,
    DamageType, HitZone, Ignored, DEFAULT_HEALTH, RESPAWN_DELAY, SPAWN_PROTECTION,
};
pub use message::{FireMsg, ServerMsg, FLAG_FRESH_PRESS, FLAG_SIGHTED};
pub use mode::{FpsMode, PlayerView, MODE_ID};
pub use recoil::RecoilState;
pub use respawn::{choose_spawn, visible_from, BEST_COUNT};
pub use rules::{FireError, FpsHost, FpsRules, ShotContext, Slot};
pub use shot::{
    fire, ray_sphere, PelletReport, PlayerContact, RayCaster, RewindCaster, SceneCaster,
    ServerCaster, ShotInput, ShotReject, ShotReport, WeaponState, CADENCE_TOLERANCE, SPRAY_RESET,
};
pub use spread::{cone_sigma, decay_beta, spread_direction, state_multiplier, AimBasis, Stance};
pub use weapon::{
    DamageDef, FireMode, ProjectileDef, RecoilDef, SightDef, SpreadDef, WeaponDef, WeaponError,
    WeaponKind,
};

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
//! * [`projectile`], [`explosion`] and [`melee`] - lot 4.5: projectile weapons,
//!   area blasts with knockback (the explosive jump) and the melee arc.
//! * [`damage`] - [`DamageEvent`], falloff and the [`Combatant`] chain.
//! * [`respawn`] - [`choose_spawn`].
//! * [`rules`] and [`mode`] - [`FpsRules`] (shots, kills, respawns) and
//!   [`FpsMode`], its `minigame-api` adapter.
//! * [`message`] - wire layouts of the mode's messages.
//! * [`pickup`] and [`play`] - lot 4.7: pickups, timed bonuses, and the two
//!   playable modes `fps.ffa` and `fps.team`.
//! * [`class`] and [`balance`] - lot 4.6: classes, loadouts, the data catalog and
//!   the time-to-kill table of `cargo xtask balance`.
//!
//! Time is in seconds as `f64` taken from the tick (`Tick::seconds`), never
//! rounded to ticks. Units: metre, second, radian; -Z forward, Y up.
#![forbid(unsafe_code)]

pub mod balance;
pub mod class;
pub mod damage;
pub mod explosion;
pub mod melee;
pub mod message;
pub mod mode;
pub mod pickup;
pub mod play;
pub mod projectile;
pub mod recoil;
pub mod respawn;
pub mod rules;
pub mod shot;
pub mod spread;
pub mod weapon;

#[cfg(test)]
mod harness;

pub use damage::{
    falloff_multiplier, pellet_damage, Combatant, DamageContext, DamageEvent, DamageOutcome,
    DamageType, HitZone, Ignored, DEFAULT_HEALTH, RESPAWN_DELAY, SPAWN_PROTECTION,
};
pub use explosion::{
    blast, explosion_centre, explosion_effect, BlastEffect, CENTRE_OFFSET, SELF_DAMAGE_FACTOR,
};
pub use melee::{melee_attack, rewound_hitboxes, MeleeReport};
pub use message::ProjectileSpawn;
pub use message::{FireMsg, ServerMsg, FLAG_FRESH_PRESS, FLAG_SIGHTED};
pub use mode::{FpsMode, PlayerView, MODE_ID};
pub use pickup::{ActiveBonuses, Pickup, PickupEvent, PickupKind, Pickups, KINDS, RADIUS};
pub use play::{
    register_modes, FfaMode, FpsPlay, PlayView, TeamMode, FFA_ID, MAX_PLAYERS, PLAYERS, TEAM_ID,
};
pub use projectile::{
    advance_projectile, catch_up_time, launch, step_projectile, Detonation, Launch, ProjectileEnv,
    ProjectileEvent, ProjectileState, ProjectileStep, ProjectileSystem, GRAVITY, MAX_CATCH_UP,
};
pub use recoil::RecoilState;
pub use respawn::{choose_spawn, visible_from, BEST_COUNT};
pub use rules::{FireError, FpsHost, FpsRules, ShotContext, Slot};
pub use shot::{
    fire, ray_sphere, PelletReport, PlayerContact, RayCaster, RewindCaster, SceneCaster,
    ServerCaster, ShotInput, ShotReject, ShotReport, WeaponState, CADENCE_TOLERANCE, SPRAY_RESET,
};
pub use spread::{cone_sigma, decay_beta, spread_direction, state_multiplier, AimBasis, Stance};
pub use weapon::{
    DamageDef, EffectDef, ExplosionDef, FireMode, MeleeDef, ProjectileDef, RecoilDef, SightDef,
    SpreadDef, WeaponDef, WeaponError, WeaponKind,
};

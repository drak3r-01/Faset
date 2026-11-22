//! Authoritative game server.
//!
//! [`Server`] accepts clients on any number of transports at once, runs the
//! connection handshake, buffers each client's input commands by tick, steps
//! the [`faset_sim::World`] at a fixed 60 Hz and sends snapshots, as deltas
//! against the last snapshot each client acknowledged (see
//! [`snapshot::delta`]).
//!
//! * [`Server::tick`] advances exactly one step and never sleeps; tests drive
//!   it directly.
//! * [`Server::run_until`] is the real-time loop: it sleeps until the next
//!   tick, spins for the last millisecond, and catches up at most
//!   [`MAX_CATCH_UP`] ticks per iteration under overload.
//!
//! Handshake (all on the reliable event channel):
//! 1. client sends `ClientHello` (protocol version, build fingerprint);
//!    a mismatch is rejected by closing the connection;
//! 2. server replies `ServerHello` (mode, player id, map fingerprint, tick rate);
//! 3. client loads the map (only the fingerprint is checked for now);
//! 4. client sends `Ready`;
//! 5. server spawns the player and sends a full snapshot.
//!
//! Every tick also records each player's hitboxes in a one-second history
//! ([`lag_compensation`]); [`Server::rewind_and_cast`] casts a hitscan shot
//! against the other players as the shooter saw them.
//!
//! Lot 4.11 (integration):
//! * players move with the character controller of `faset-sim`
//!   ([`ServerConfig::move_params`], map zones in [`MapConfig::zones`]);
//!   snapshot velocities are the controller's;
//! * [`Server::host_match`] hosts a `minigame-api` [`Match`](faset_minigame_api::Match):
//!   it runs after the world step with the tick's commands, zone events
//!   and [`ModeServices`] (lag compensation), its respawns and effects are
//!   applied to the world, movement is frozen outside `Warmup` and
//!   `Playing` and for dead players, and its state (`Event::MatchState`,
//!   when it changed, at the snapshot rate), events (`Event::Game`,
//!   `Event::ProjectileSpawn`, `Event::Explosion`) and messages
//!   (`Event::Mode`) go to every client on the event channel; clients'
//!   `Event::Mode` messages reach the mode;
//! * bots get a [`BotContext`] with the match and may send mode messages.
#![forbid(unsafe_code)]

mod clock;
mod commands;
mod config;
pub mod lag_compensation;
mod link;
pub mod rotation;
mod server;
mod services;
pub mod snapshot;
pub mod transfer;

pub use rotation::{
    MapOrder, MapProvider, PreparedMap, Rotation, RotationAction, RotationConfig, RotationOrder,
};
pub use services::{ModeServices, VIEW_DELAY};

pub use clock::{TickClock, MAX_CATCH_UP};
pub use commands::CommandBuffer;
pub use config::{
    build_fingerprint, map_fingerprint, ConfigError, Limits, MapConfig, ServerConfig,
    MAX_SNAPSHOT_RATE, MIN_SNAPSHOT_RATE,
};
pub use lag_compensation::{
    BodyPart, FireCommand, Hit, Hitbox, LagCompensation, ShotOutcome, CONSISTENCY_SLACK,
    EYE_HEIGHT, HEAD_RADIUS, HISTORY_TICKS, MAX_REWIND,
};
pub use link::ServerTransport;
pub use server::{Bot, BotContext, RejectReason, Server, ServerNotice};
pub use snapshot::{
    EntityState, InputPacket, QuantizedEntity, QuantizedSnapshot, Snapshot, SnapshotDecoder,
    SnapshotEncoder, SnapshotError, MAX_SNAPSHOT_ENTITIES, REFERENCE_SNAPSHOTS,
};

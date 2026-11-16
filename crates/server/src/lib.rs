//! Authoritative game server.
//!
//! [`Server`] accepts clients on any number of transports at once, runs the
//! connection handshake, buffers each client's input commands by tick, steps
//! the [`faset_sim::World`] at a fixed 60 Hz and sends full snapshots.
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
#![forbid(unsafe_code)]

mod clock;
mod commands;
mod config;
mod link;
mod server;
pub mod snapshot;

pub use clock::{TickClock, MAX_CATCH_UP};
pub use commands::CommandBuffer;
pub use config::{
    build_fingerprint, map_fingerprint, ConfigError, Limits, MapConfig, ServerConfig,
    MAX_SNAPSHOT_RATE, MIN_SNAPSHOT_RATE,
};
pub use link::ServerTransport;
pub use server::{Bot, RejectReason, Server, ServerNotice};
pub use snapshot::{EntityState, Snapshot, SnapshotError, MAX_SNAPSHOT_ENTITIES};

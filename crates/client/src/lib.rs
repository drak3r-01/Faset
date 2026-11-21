//! Game client: integrated server hosting, LAN discovery, clock
//! synchronization, local prediction, reconciliation and smoothing of
//! corrections.
//!
//! * [`hosting`]: [`host_game`] runs a `faset-server` in-process on its own
//!   thread, joined by a memory transport; a solo game binds no socket, and
//!   [`HostHandle::open_to_lan`] adds UDP on the game port without a restart.
//! * [`discovery`]: UDP broadcast server browser ([`ServerBrowser`]) and the
//!   responder hosts run ([`DiscoveryResponder`]); 2 s refresh, 6 s expiry.
//!
//! * [`clock`]: estimates round-trip time and jitter, keeps the client's
//!   simulation a few ticks ahead of the server so its commands arrive just
//!   before the tick that consumes them, and corrects drift by running at
//!   95 % to 105 % of real speed, never skipping a tick.
//! * [`prediction`]: runs the local player through the shared
//!   [`faset_sim::World`] as soon as a command is produced, keeps the last
//!   [`HISTORY_TICKS`] commands and predicted states, reconciles them against
//!   server snapshots (replaying with the same simulation code) and hides
//!   corrections behind a decaying [`VisualOffset`].
//! * [`map_load`]: turns a map file into collisions and render meshes (steps 1
//!   to 3 of the map load pipeline) and uploads the meshes.
//! * [`interpolation`]: shows remote entities a deliberate delay in the
//!   past with Hermite interpolation between snapshots, extrapolating for at
//!   most a quarter of a second, and gives the [`ViewTime`] a fire command
//!   carries for server-side lag compensation.
//!
//! The crate is transport- and codec-agnostic: the snapshot and input packet
//! codecs live in `faset-server` (lot 1.5), which a client library may not
//! depend on. Callers decode a snapshot, pick the local player's entity and
//! pass it as an [`AuthoritativeState`]; they wrap the commands returned by
//! [`Prediction::unacknowledged`] in input packets. The integration tests
//! (`tests/`) show the full loop over the conditioned transport.
//!
//! Predicted: movement and weapon switching (the only player systems the sim
//! has today). Firing feedback and ammunition plug into the same path when
//! the sim gains weapons (lot 4.4): [`Prediction::predict`] runs once per
//! tick and is the only place where first-time effects belong, since a
//! replay never re-emits them. Never predicted: damage, deaths, pickups.
#![forbid(unsafe_code)]

pub mod clock;
pub mod discovery;
pub mod hosting;
pub mod interpolation;
pub mod map_load;
pub mod prediction;

pub use clock::{
    ClockConfig, ClockSync, DEFAULT_GAIN, DEFAULT_MARGIN_TICKS, DEFAULT_SYNC_SNAPSHOTS,
    MAX_TIMESCALE, MIN_TIMESCALE,
};
pub use discovery::{
    DiscoveryResponder, ServerBrowser, ServerEntry, ServerInfo, DISCOVERY_PORT, ENTRY_TTL,
    GAME_PORT, REFRESH_INTERVAL,
};
pub use hosting::{host_game, HostConfig, HostError, HostHandle, LanStatus};
pub use interpolation::{
    hermite, interpolation_delay, lerp_angle, InterpolationBuffer, InterpolationConfig,
    RemoteInterpolation, RemoteState, RemoteView, SampleMode, ViewTime, BUFFER_SNAPSHOTS,
    DEFAULT_SNAPSHOT_INTERVAL, MAX_EXTRAPOLATION,
};
pub use map_load::{load_map, LoadedMap, MapLoadError};
pub use prediction::{
    AuthoritativeState, HistoryEntry, PredictedState, Prediction, PredictionError,
    PredictionHistory, ReconcileOutcome, Tolerance, VisualOffset, COMMANDS_PER_PACKET,
    HISTORY_TICKS, SMOOTHING_TAU, SNAP_DISTANCE,
};

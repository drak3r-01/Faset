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
//! * [`camera`] and [`feel`]: the first-person camera (eye height, speed FOV,
//!   roll, landing dip), its TOML-compatible [`FeelParams`] and the mouse
//!   sensitivity conversions.
//! * [`hud`]: draws the FPS HUD model with egui.
//! * [`effects`], [`shoot_feel`], [`sounds`], [`surface`] (lot 6.9): the feel of
//!   shooting. Muzzle flash, tracers, material impacts and bullet decals in
//!   bounded pools, the recoil curve and camera shake derived from the weapon
//!   data, the sounds synthesised in code and the surface kind a shot hits.
//! * [`map_load`]: turns a map file into collisions and render meshes (steps 1
//!   to 3 of the map load pipeline) and uploads the meshes; [`scenery`] (lot
//!   6.11) draws the map's props and its instanced vegetation.
//! * [`map_sync`]: the map cache (by fingerprint) and the `MapChange` and map
//!   download handler (lot 5.1).
//! * [`state`], [`session`], [`app`] (lot 6.1): the stack of application states
//!   ([`AppState`], [`StateStack`]), the reusable match [`Session`] (handshake,
//!   prediction, interpolation over a memory or UDP link) and the [`App`] that
//!   ties them to the menus; the `faset` executable (`src/bin/faset.rs`) puts a
//!   window, the renderer and egui around it.
//! * [`host_flow`], [`join`], [`favorites`] (lot 6.2): the play screen's request
//!   becomes a [`HostConfig`] (match, rotation, bots), a [`JoinTarget`] is an
//!   address or a LAN entry, and [`Favorites`] keeps favourites and history in
//!   `favorites.ron`.
//! * [`profile`], [`loadout_flow`], [`preview`] (lot 6.3): the player's classes
//!   in `profile.ron` ([`Profile`]), the glue between the loadout screen and
//!   the class catalogue and budget of `faset-mode-fps`, and the 3D character
//!   preview ([`CharacterPreview`]) drawn through the renderer's usual
//!   interfaces. The server revalidates every loadout it receives.
//! * [`input`], [`egui_input`], [`match_view`], [`scene`] (lot 6.6): the player's keys and mouse as input commands (`GameInput`), the winit to egui adapter (`EguiInput`), the client's copy of a match (`MatchView`: mode mirror, kills, projectiles, the HUD model of each frame) and the other players, pickups and weapon in hand (`MatchScene`).
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

pub mod app;
pub mod camera;
pub mod clock;
pub mod discovery;
pub mod effects;
pub mod egui_input;
pub mod favorites;
pub mod feel;
pub mod host_flow;
pub mod hosting;
pub mod hud;
pub mod input;
pub mod interpolation;
pub mod join;
pub mod loadout_flow;
pub mod look;
pub mod map_load;
pub mod map_sync;
pub mod match_view;
pub mod prediction;
pub mod preview;
pub mod profile;
pub mod scene;
pub mod scenery;
pub mod session;
pub mod shoot_feel;
pub mod sounds;
pub mod state;
pub mod surface;
pub mod weapon_models;

pub use app::{App, MapCatalogue, CONNECT_TIMEOUT};
pub use camera::{CameraPose, FirstPersonCamera, Look};
pub use clock::{
    ClockConfig, ClockSync, DEFAULT_GAIN, DEFAULT_MARGIN_TICKS, DEFAULT_SYNC_SNAPSHOTS,
    MAX_TIMESCALE, MIN_TIMESCALE,
};
pub use discovery::{
    DiscoveryResponder, ServerBrowser, ServerEntry, ServerInfo, DISCOVERY_PORT, ENTRY_TTL,
    GAME_PORT, REFRESH_INTERVAL,
};
pub use egui_input::{EguiInput, RawInput};
pub use faset_mode_fps::Loadout;
pub use favorites::{data_dir, Favorites, FavoritesError};
pub use feel::{cm_per_360, degrees_per_count, CriticalSpring, FeelParams, DEGREES_PER_COUNT};
pub use host_flow::host_config;
pub use hosting::{
    host_game, HostConfig, HostError, HostHandle, HostRotation, LanStatus, TickHook,
};
pub use hud::{FpsHud, HudOptions};
pub use input::GameInput;
pub use interpolation::{
    hermite, interpolation_delay, lerp_angle, InterpolationBuffer, InterpolationConfig,
    RemoteInterpolation, RemoteState, RemoteView, SampleMode, ViewTime, BUFFER_SNAPSHOTS,
    DEFAULT_SNAPSHOT_INTERVAL, MAX_EXTRAPOLATION,
};
pub use join::JoinTarget;
pub use look::MapLook;
pub use map_load::{load_map, LoadedMap, MapLoadError};
pub use map_sync::{MapCache, MapEvent, MapSync, MapSyncError};
pub use match_view::{Blast, Feedback, LiveProjectile, LocalFrame, MatchView};
pub use prediction::{
    AuthoritativeState, HistoryEntry, PredictedState, Prediction, PredictionError,
    PredictionHistory, ReconcileOutcome, Tolerance, VisualOffset, COMMANDS_PER_PACKET,
    HISTORY_TICKS, SMOOTHING_TAU, SNAP_DISTANCE,
};
pub use preview::CharacterPreview;
pub use profile::{Appearance, Profile, ProfileError, SavedClass, PROFILE_VERSION};
pub use scene::{MatchScene, SceneFrame, SceneOutput};
pub use session::{map_config, Session, SessionError};
pub use shoot_feel::{
    blast_trauma, recoil_pattern, shot_trauma, CameraShake, DamageArc, DamageIndicators,
};
pub use sounds::{shot_recipe, Cue};
pub use state::{AppState, StateError, StateStack, Transition};
pub use surface::{Surface, SurfaceMap};

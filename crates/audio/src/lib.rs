//! Audio engine: kira-based voices, effects synthesised from RON recipes and
//! 3D spatialisation. Units: metres, seconds. Y up, -Z forward.
//!
//! Playback runs on the thread managed by kira; this crate adds no thread.
//!
//! Dependency note: kira pulls in the unmaintained `paste` (RUSTSEC-2024-0436), ignored
//! in `deny.toml`; re-check when kira drops it.

#![forbid(unsafe_code)]

mod engine;
mod recipe;
mod spatial;

pub use engine::{Bus, Config, Engine, Error, VoiceId, VoiceInfo, MAX_VOICES};
pub use recipe::{synthesise, Envelope, Recipe, Waveform};
pub use spatial::{attenuation, spatialise, Spatial, SpatialConfig, D_REF, K};

/// World position of a sound source, in metres.
pub type Position3D = faset_core::glam::Vec3;

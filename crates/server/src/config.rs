//! Server configuration and fingerprints.

use std::fmt;

use faset_core::glam::Vec3;
use faset_core::TICK_RATE;
use faset_sim::StaticWorld;

use crate::snapshot::MAX_SNAPSHOT_ENTITIES;

/// Lowest accepted snapshot rate in Hz.
pub const MIN_SNAPSHOT_RATE: u8 = 20;
/// Highest accepted snapshot rate in Hz (one snapshot per tick).
pub const MAX_SNAPSHOT_RATE: u8 = 60;

/// Invalid [`ServerConfig`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    /// The simulation step is fixed at [`TICK_RATE`] Hz.
    #[error("tick rate must be {TICK_RATE} Hz, got {0}")]
    TickRate(u8),
    /// Snapshot rate outside [`MIN_SNAPSHOT_RATE`]..=[`MAX_SNAPSHOT_RATE`].
    #[error("snapshot rate must be {MIN_SNAPSHOT_RATE} to {MAX_SNAPSHOT_RATE} Hz, got {0}")]
    SnapshotRate(u8),
    /// `max_players` is zero or does not fit one full snapshot.
    #[error("max players must be 1 to {MAX_SNAPSHOT_ENTITIES}, got {0}")]
    MaxPlayers(usize),
    /// The command buffer must hold at least one tick.
    #[error("command buffer must hold at least one tick")]
    CommandBuffer,
    /// The map has no spawn point.
    #[error("the map has no spawn point")]
    NoSpawnPoint,
}

/// Capacity and timeout limits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limits {
    /// Most players (humans and bots) at once.
    pub max_players: usize,
    /// How far ahead of the current tick a command may be buffered.
    pub command_buffer_ticks: u32,
    /// A client that has not finished the handshake after this many ticks is
    /// dropped.
    pub handshake_timeout_ticks: u32,
}

impl Default for Limits {
    /// Decisions: 32 players, 64 ticks (about 1 s) of buffered commands,
    /// 10 s to finish the handshake.
    fn default() -> Self {
        Self {
            max_players: 32,
            command_buffer_ticks: 64,
            handshake_timeout_ticks: 600,
        }
    }
}

/// The map the server runs.
#[derive(Clone)]
pub struct MapConfig {
    /// Static collision geometry.
    pub statics: StaticWorld,
    /// [`map_fingerprint`] of the map's canonical form, sent in the handshake.
    pub fingerprint: u64,
    /// Capsule-centre spawn positions, used round-robin by player id.
    pub spawn_points: Vec<Vec3>,
}

impl fmt::Debug for MapConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MapConfig")
            .field("volumes", &self.statics.volumes().len())
            .field("fingerprint", &format_args!("{:016x}", self.fingerprint))
            .field("spawn_points", &self.spawn_points)
            .finish()
    }
}

impl Default for MapConfig {
    /// The sim's test arena (floor at y = 0, wall at x = 10) with spawn
    /// points 2 m apart along +Z (decision).
    fn default() -> Self {
        Self {
            statics: StaticWorld::test_arena(),
            fingerprint: map_fingerprint(b"faset:test-arena"),
            spawn_points: (0..8u8)
                .map(|i| Vec3::new(0.0, 0.91, 2.0 * f32::from(i)))
                .collect(),
        }
    }
}

/// Everything [`Server::new`](crate::Server::new) needs besides transports.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Simulation rate; must equal [`TICK_RATE`] (the sim step is fixed).
    pub tick_rate: u8,
    /// Full snapshots per second, 20 to 60 (default 30).
    pub snapshot_rate: u8,
    /// Game mode id sent in the handshake (meaning defined by later lots).
    pub mode: u8,
    /// Build fingerprint clients must match.
    pub build_fingerprint: u64,
    /// World RNG seed.
    pub seed: u64,
    /// The map.
    pub map: MapConfig,
    /// Capacity and timeout limits.
    pub limits: Limits,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            tick_rate: TICK_RATE,
            snapshot_rate: 30,
            mode: 0,
            build_fingerprint: build_fingerprint(),
            seed: 0,
            map: MapConfig::default(),
            limits: Limits::default(),
        }
    }
}

impl ServerConfig {
    /// Checks every field.
    ///
    /// # Errors
    /// The first invalid field, as a [`ConfigError`].
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.tick_rate != TICK_RATE {
            return Err(ConfigError::TickRate(self.tick_rate));
        }
        if !(MIN_SNAPSHOT_RATE..=MAX_SNAPSHOT_RATE).contains(&self.snapshot_rate) {
            return Err(ConfigError::SnapshotRate(self.snapshot_rate));
        }
        if self.limits.max_players == 0 || self.limits.max_players > MAX_SNAPSHOT_ENTITIES {
            return Err(ConfigError::MaxPlayers(self.limits.max_players));
        }
        if self.limits.command_buffer_ticks == 0 {
            return Err(ConfigError::CommandBuffer);
        }
        if self.map.spawn_points.is_empty() {
            return Err(ConfigError::NoSpawnPoint);
        }
        Ok(())
    }
}

/// First 8 bytes (little endian) of the BLAKE3 hash of `data`.
fn blake3_u64(data: &[u8]) -> u64 {
    let hash = blake3::hash(data);
    let mut first = [0u8; 8];
    first.copy_from_slice(&hash.as_bytes()[..8]);
    u64::from_le_bytes(first)
}

/// Map fingerprint: BLAKE3 of the map's canonical form, truncated to the
/// 64 bits the handshake carries (decision: first 8 bytes, little endian).
#[must_use]
pub fn map_fingerprint(canonical: &[u8]) -> u64 {
    blake3_u64(canonical)
}

/// Fingerprint of this build: BLAKE3 of the package version and the wire
/// protocol version (decision). Client and server of the same build agree.
#[must_use]
pub fn build_fingerprint() -> u64 {
    let text = format!(
        "faset {} protocol {}",
        env!("CARGO_PKG_VERSION"),
        faset_protocol::PROTOCOL_VERSION
    );
    blake3_u64(text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_valid() {
        ServerConfig::default().validate().unwrap();
    }

    #[test]
    fn rates_and_limits_are_checked() {
        let mut c = ServerConfig {
            snapshot_rate: 19,
            ..ServerConfig::default()
        };
        assert_eq!(c.validate(), Err(ConfigError::SnapshotRate(19)));
        c.snapshot_rate = 61;
        assert_eq!(c.validate(), Err(ConfigError::SnapshotRate(61)));
        c.snapshot_rate = 60;
        c.validate().unwrap();
        c.tick_rate = 30;
        assert_eq!(c.validate(), Err(ConfigError::TickRate(30)));
        c.tick_rate = TICK_RATE;
        c.limits.max_players = MAX_SNAPSHOT_ENTITIES + 1;
        assert!(matches!(c.validate(), Err(ConfigError::MaxPlayers(_))));
    }

    #[test]
    fn fingerprints_are_stable_and_distinct() {
        assert_eq!(map_fingerprint(b"a"), map_fingerprint(b"a"));
        assert_ne!(map_fingerprint(b"a"), map_fingerprint(b"b"));
        assert_eq!(build_fingerprint(), build_fingerprint());
    }
}

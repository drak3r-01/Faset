//! Immediate test: an integrated server on the document, joined over the
//! memory transport, with the player standing where the camera is.

use faset_client::{host_game, HostConfig, HostError, HostHandle};
use faset_core::glam::Vec3;
use faset_mapfmt::{expand, MapFile};
use faset_net::{MemoryAddr, MemoryTransport};
use faset_server::{map_fingerprint, MapConfig, ServerConfig};
use faset_sim::MapCollision;

/// Why a test could not start.
#[derive(Debug, thiserror::Error)]
pub enum TestError {
    #[error("cannot start the integrated server: {0}")]
    Host(#[from] HostError),
}

/// A running test of the current map. The server stops when this is dropped
/// or [`TestSession::stop`] is called.
pub struct TestSession {
    host: HostHandle,
    spawn: Vec3,
    fingerprint: u64,
}

impl std::fmt::Debug for TestSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TestSession")
            .field("spawn", &self.spawn)
            .field("fingerprint", &format_args!("{:016x}", self.fingerprint))
            .finish()
    }
}

impl TestSession {
    /// The server configuration for `map` with the player at `spawn`.
    ///
    /// Decision: the server places a player's capsule centre at the spawn
    /// point, so the camera position is used as it is; the map's own spawn
    /// points are not used during an editor test. The map is sent through
    /// its canonical form, so the handshake fingerprint is the first 64 bits
    /// of the map's BLAKE3 id.
    #[must_use]
    pub fn server_config(map: &MapFile, spawn: Vec3) -> ServerConfig {
        let expanded = expand(map);
        let collision = MapCollision::build(&expanded);
        ServerConfig {
            map: MapConfig {
                statics: collision.world().clone(),
                fingerprint: map_fingerprint(faset_mapfmt::to_ron(map).as_bytes()),
                spawn_points: vec![spawn],
                zones: faset_sim::Zones::from_map(&expanded),
            },
            ..ServerConfig::default()
        }
    }

    /// Starts an integrated server on `map` (memory transport only, never
    /// opened to the network) with the player at `spawn`.
    ///
    /// # Errors
    /// [`TestError::Host`] if the server cannot start.
    pub fn start(map: &MapFile, spawn: Vec3) -> Result<Self, TestError> {
        let server = Self::server_config(map, spawn);
        let fingerprint = server.map.fingerprint;
        let mut config = HostConfig::new(server);
        config.name = format!("Test: {}", map.meta.name);
        config.map_name = map.meta.name.clone();
        let host = host_game(config, false)?;
        Ok(Self {
            host,
            spawn,
            fingerprint,
        })
    }

    /// Where the player appears.
    #[must_use]
    pub fn spawn(&self) -> Vec3 {
        self.spawn
    }

    /// The map fingerprint the server announces in the handshake.
    #[must_use]
    pub fn fingerprint(&self) -> u64 {
        self.fingerprint
    }

    /// A memory endpoint for the player and the server address to connect
    /// it to.
    #[must_use]
    pub fn connect_local(&self) -> (MemoryTransport, MemoryAddr) {
        self.host.connect_local()
    }

    /// Players currently in the test game.
    #[must_use]
    pub fn players(&self) -> usize {
        self.host.player_count()
    }

    /// Stops the server.
    pub fn stop(self) {
        self.host.stop();
    }
}

//! Lot 5.1 acceptance: a real server rotates from map A to map B. The
//! next map is prepared on a worker thread, `MapChange` goes out, a client
//! lacking map B downloads it on channel 3 and loads it, a tampered payload
//! is rejected, and a client that never reports ready does not hold the
//! warm-up back for more than 10 s.

use std::sync::Arc;
use std::time::Duration;

use faset_client::{load_map, MapCache, MapEvent, MapSync, MapSyncError};
use faset_core::glam::Vec3;
use faset_core::{NetId, DT};
use faset_minigame_api::{Match, MatchConfig, Phase, Registry};
use faset_mode_fps::register_modes;
use faset_net::{Channel, ClientEndpoint, MemoryAddr, MemoryNetwork, MemoryTransport};
use faset_protocol::{Event, Message, PROTOCOL_VERSION};
use faset_server::{
    build_fingerprint, map_fingerprint, MapConfig, MapProvider, PreparedMap, RotationConfig,
    RotationOrder, Server, ServerConfig,
};
use faset_sim::{MapCollision, Zones};

const MAP_A: &str = include_str!("../../../assets/testdata/example.ron");

/// A second, small map.
const MAP_B: &str = r#"Map(
    format_version: 1,
    meta: (name: "Yard", author: "Faset", description: "Lot 5.1 test yard.", modes: ["fps"], players: (2, 12)),
    theme: (ambience: "noon", sky: "clear", weather: [], palette: "industrial"),
    brushes: [
        Box(min: (-60, -2, -60), max: (60, 0, 60), material: "concrete"),
        Box(min: (-6, 0, -1), max: (6, 4, 1), material: "concrete"),
    ],
    spawns: [
        (at: (-12.0, 0.0, -8.0), yaw: 0.0, team: A),
        (at: (12.0, 0.0, 12.0), yaw: 0.0, team: B),
    ],
    pickups: [],
    zones: [],
)
"#;

const MAPS: [&str; 2] = [MAP_A, MAP_B];

/// Builds what the dedicated server's catalog builds: collision, spawns and
/// zones from the file bytes.
fn config_of(bytes: &[u8]) -> MapConfig {
    let map = faset_mapfmt::expand(&faset_mapfmt::parse(bytes).unwrap());
    MapConfig {
        statics: MapCollision::build(&map).world().clone(),
        fingerprint: map_fingerprint(bytes),
        spawn_points: map
            .spawns
            .iter()
            .map(|s| Vec3::from_array(s.at) + Vec3::Y * 0.91)
            .collect(),
        zones: Zones::from_map(&map),
    }
}

struct Maps;

impl MapProvider for Maps {
    fn count(&self) -> usize {
        MAPS.len()
    }

    fn prepare(&self, index: usize) -> Result<PreparedMap, String> {
        let bytes = MAPS.get(index).ok_or("no such map")?.as_bytes();
        Ok(PreparedMap {
            name: format!("map{index}"),
            file: bytes.to_vec(),
            config: config_of(bytes),
            payload: None,
        })
    }

    fn new_match(&self, map: &mut PreparedMap) -> Option<Match> {
        let mut registry = Registry::new();
        register_modes(&mut registry).unwrap();
        let entry = registry.find("fps.ffa")?.clone();
        Match::new(
            &entry,
            MatchConfig {
                spawn_points: map.config.spawn_points.clone(),
                ..MatchConfig::default()
            },
        )
        .ok()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Behaviour {
    /// Loads the map and reports ready.
    Honest,
    /// Flips a bit of every downloaded chunk.
    Tampering,
    /// Never answers a `MapChange`.
    Silent,
}

struct Client {
    endpoint: ClientEndpoint<MemoryTransport>,
    hello: bool,
    behaviour: Behaviour,
    sync: MapSync,
    /// Fingerprints announced by `MapChange`.
    changes: Vec<u64>,
    /// Fingerprints of the maps loaded into collision worlds.
    loaded: Vec<u64>,
    failures: Vec<MapSyncError>,
    progress: u32,
    player: Option<NetId>,
}

impl Client {
    fn new(net: &MemoryNetwork, server: MemoryAddr, behaviour: Behaviour, has_a: bool) -> Self {
        let mut cache = MapCache::new();
        if has_a {
            cache.insert(map_fingerprint(MAP_A.as_bytes()), MAP_A.as_bytes().to_vec());
        }
        Self {
            endpoint: ClientEndpoint::new(net.endpoint(), server),
            hello: false,
            behaviour,
            sync: MapSync::new(cache),
            changes: Vec::new(),
            loaded: Vec::new(),
            failures: Vec::new(),
            progress: 0,
            player: None,
        }
    }

    fn send(&mut self, msg: &Message) {
        self.endpoint
            .send(Channel::Event, &msg.encode().unwrap())
            .unwrap();
    }

    fn step(&mut self) {
        if !self.hello {
            self.hello = true;
            self.send(&Message::ClientHello {
                protocol_version: PROTOCOL_VERSION,
                build_fingerprint: build_fingerprint(),
            });
        }
        self.endpoint.update(Duration::from_secs_f32(DT)).unwrap();
        while let Some(bytes) = self.endpoint.recv(Channel::Event) {
            match Message::decode(&bytes).unwrap() {
                Message::ServerHello { player_id, .. } => {
                    self.player = Some(player_id);
                    self.send(&Message::Ready);
                }
                Message::Event(Event::MapChange { map_fingerprint }) => {
                    self.changes.push(map_fingerprint);
                    if self.behaviour != Behaviour::Silent {
                        if let Some(request) = self.sync.on_map_change(map_fingerprint) {
                            self.send(&request);
                        }
                    }
                }
                _ => {}
            }
        }
        while let Some(bytes) = self.endpoint.recv(Channel::MapTransfer) {
            let mut msg = Message::decode(&bytes).unwrap();
            if let (Behaviour::Tampering, Message::MapChunk { data, .. }) =
                (self.behaviour, &mut msg)
            {
                if let Some(b) = data.first_mut() {
                    *b ^= 0x55;
                }
            }
            self.sync.on_chunk(&msg);
        }
        while let Some(event) = self.sync.poll_event() {
            match event {
                MapEvent::Progress { .. } => self.progress += 1,
                MapEvent::Ready { fingerprint, bytes } => {
                    assert_eq!(map_fingerprint(&bytes), fingerprint);
                    let loaded = load_map(&bytes).expect("the map loads");
                    assert!(loaded.collision.volume_count() > 0);
                    self.loaded.push(fingerprint);
                    self.send(&MapSync::ready_message());
                }
                MapEvent::Failed { error, .. } => self.failures.push(error),
            }
        }
    }
}

struct Rig {
    server: Server,
    clients: Vec<Client>,
}

impl Rig {
    fn new(behaviours: &[(Behaviour, bool)]) -> Self {
        let net = MemoryNetwork::new();
        let end = net.endpoint();
        let addr = end.addr();
        let mut server = Server::new(
            ServerConfig {
                map: config_of(MAP_A.as_bytes()),
                ..ServerConfig::default()
            },
            vec![end.into()],
        )
        .unwrap();
        server.set_rotation(
            RotationConfig {
                rotation_seconds: 3,
                prepare_seconds: 2,
                scoreboard_seconds: 1,
                order: RotationOrder::Sequential,
                ..RotationConfig::default()
            },
            Arc::new(Maps),
            0,
        );
        let clients = behaviours
            .iter()
            .map(|&(b, has_a)| Client::new(&net, addr, b, has_a))
            .collect();
        Self { server, clients }
    }

    fn tick(&mut self) {
        for c in &mut self.clients {
            c.step();
        }
        self.server.tick();
    }

    /// Ticks until `done`, at most `limit` ticks; returns the ticks run.
    fn run(&mut self, limit: u32, done: impl Fn(&Self) -> bool) -> u32 {
        for n in 0..limit {
            if done(self) {
                return n;
            }
            self.tick();
        }
        panic!("not reached in {limit} ticks");
    }
}

fn fp_b() -> u64 {
    map_fingerprint(MAP_B.as_bytes())
}

#[test]
fn a_client_without_the_map_downloads_it_and_the_warm_up_starts() {
    let mut rig = Rig::new(&[(Behaviour::Honest, true)]);
    // The rotation ends the map at 3 s, shows the scoreboard 1 s, then
    // sends `MapChange`.
    rig.run(600, |r| !r.clients[0].changes.is_empty());
    assert!(rig.server.world().tick().0 >= 4 * 60);
    assert_eq!(rig.clients[0].changes, [fp_b()]);
    assert_eq!(rig.server.map_generation(), 1);
    assert_eq!(rig.server.config().map.fingerprint, fp_b());
    assert!(rig.server.game().is_none(), "no match before the warm-up");
    // The client lacks map B: it downloads it and reports ready; the
    // warm-up then starts well before the 10 s limit.
    let ticks = rig.run(600, |r| r.server.game().is_some());
    assert!(ticks < 120, "warm-up took {ticks} ticks");
    let c = &rig.clients[0];
    assert!(c.progress >= 1);
    assert_eq!(c.loaded, [fp_b()]);
    assert!(c.failures.is_empty());
    assert_eq!(rig.server.game().unwrap().phase(), Phase::Warmup);
    assert_eq!(rig.server.rotation_current(), Some(1));
}

#[test]
fn a_client_that_is_not_ready_after_10_s_does_not_block_the_warm_up() {
    let mut rig = Rig::new(&[(Behaviour::Honest, true), (Behaviour::Silent, true)]);
    rig.run(600, |r| !r.clients[0].changes.is_empty());
    let ticks = rig.run(1200, |r| r.server.game().is_some());
    assert!(
        (595..=610).contains(&ticks),
        "warm-up after {ticks} ticks, expected the 10 s limit"
    );
    assert_eq!(rig.clients[0].loaded, [fp_b()]);
    assert!(rig.clients[1].loaded.is_empty());
}

#[test]
fn a_tampered_download_is_rejected_by_the_fingerprint_check() {
    let mut rig = Rig::new(&[(Behaviour::Tampering, true)]);
    rig.run(600, |r| !r.clients[0].changes.is_empty());
    rig.run(600, |r| r.server.game().is_some());
    let c = &rig.clients[0];
    assert!(c.loaded.is_empty());
    assert_eq!(c.failures.len(), 1, "{:?}", c.failures);
}

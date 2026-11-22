//! Lot 4.11 acceptance: a `fps.team` match through the real dedicated
//! server and the in-memory transport. Two predicting human clients and
//! four bots (virtual clients through the same input path) play on a small
//! map; rockets (projectiles, explosions, knockback), rifle shots, a melee
//! swing and pickups all go through `Server::tick`, the hosted `Match` and
//! the protocol's event channel. The run is deterministic: two runs give
//! the same world and the same event log.

use std::time::Duration;

use dedicated::maps::Catalog;
use dedicated::{Dedicated, Settings};
use faset_client::{AuthoritativeState, PredictedState, Prediction};
use faset_core::{NetId, Tick, DT};
use faset_minigame_api::{GameEvent, MatchMirror, ModeId, ModeMessage, Phase, Registry};
use faset_mode_fps::{register_modes, ClassMsg, FpsRules, ServerMsg};
use faset_net::{Channel, ClientEndpoint, MemoryAddr, MemoryNetwork, MemoryTransport};
use faset_protocol::quant::{quantize_pitch_command, quantize_yaw};
use faset_protocol::{Event, InputCommand, Message, BUTTON_FIRE, PROTOCOL_VERSION};
use faset_server::{build_fingerprint, InputPacket, MapConfig, SnapshotDecoder};
use faset_sim::{MoveScale, StaticWorld, Zones};

/// World seed of the server, shared by the prediction.
const SEED: u64 = 7;

/// A flat 60 m yard with a wall in the middle, four spawns per team, an
/// armour pickup two metres in front (-Z) of every spawn and a health
/// pickup in the middle. Brush and zone coordinates are half metres.
const MAP: &str = r#"Map(
    format_version: 1,
    meta: (name: "Yard", author: "Faset", description: "Lot 4.11 test yard.", modes: ["fps"], players: (2, 12)),
    theme: (ambience: "noon", sky: "clear", weather: [], palette: "industrial"),
    brushes: [
        Box(min: (-60, -2, -60), max: (60, 0, 60), material: "concrete"),
        Box(min: (-6, 0, -1), max: (6, 4, 1), material: "concrete"),
    ],
    spawns: [
        (at: (-12.0, 0.0, -8.0), yaw: 0.0, team: A),
        (at: (-8.0, 0.0, -8.0), yaw: 0.0, team: A),
        (at: (-4.0, 0.0, -8.0), yaw: 0.0, team: A),
        (at: (-16.0, 0.0, -8.0), yaw: 0.0, team: A),
        (at: (12.0, 0.0, 12.0), yaw: 0.0, team: B),
        (at: (8.0, 0.0, 12.0), yaw: 0.0, team: B),
        (at: (4.0, 0.0, 12.0), yaw: 0.0, team: B),
        (at: (16.0, 0.0, 12.0), yaw: 0.0, team: B),
    ],
    pickups: [
        (kind: Health, at: (0.0, 0.9, 4.0), respawn_s: 30),
    ],
    zones: [
        Box(min: (-80, -20, -80), max: (80, -10, 80), kind: KillVolume),
        Box(min: (-25, 0, -21), max: (-23, 4, -19), kind: Objective(id: "pickup.armour")),
        Box(min: (-17, 0, -21), max: (-15, 4, -19), kind: Objective(id: "pickup.armour")),
        Box(min: (-9, 0, -21), max: (-7, 4, -19), kind: Objective(id: "pickup.armour")),
        Box(min: (-33, 0, -21), max: (-31, 4, -19), kind: Objective(id: "pickup.armour")),
        Box(min: (23, 0, 19), max: (25, 4, 21), kind: Objective(id: "pickup.armour")),
        Box(min: (15, 0, 19), max: (17, 4, 21), kind: Objective(id: "pickup.armour")),
        Box(min: (7, 0, 19), max: (9, 4, 21), kind: Objective(id: "pickup.armour")),
        Box(min: (31, 0, 19), max: (33, 4, 21), kind: Objective(id: "pickup.armour")),
    ],
)
"#;

const BOTS: usize = 4;
/// Ticks the client predicts ahead of the newest snapshot.
const LEAD: u32 = 3;

/// What a scripted human does on one tick.
#[derive(Clone, Copy, Default)]
struct Intent {
    move_y: i8,
    yaw: f32,
    pitch: f32,
    buttons: u16,
    slot: u8,
}

struct Human {
    endpoint: ClientEndpoint<MemoryTransport>,
    hello: bool,
    class: ClassMsg,
    player: Option<NetId>,
    mode: Option<ModeId>,
    decoder: SnapshotDecoder,
    prediction: Option<Prediction>,
    mirror: Option<MatchMirror>,
    registry: Registry,
    statics: StaticWorld,
    zones: Zones,
    /// Every event received, in order.
    events: Vec<GameEvent>,
    /// Prediction errors at each reconciled snapshot, and corrections.
    errors: Vec<f32>,
    corrections: u32,
}

impl Human {
    fn new(net: &MemoryNetwork, server: MemoryAddr, class: ClassMsg, map: &MapConfig) -> Self {
        let mut registry = Registry::new();
        register_modes(&mut registry).unwrap();
        Self {
            endpoint: ClientEndpoint::new(net.endpoint(), server),
            hello: false,
            class,
            player: None,
            mode: None,
            decoder: SnapshotDecoder::new(),
            prediction: None,
            mirror: None,
            registry,
            statics: map.statics.clone(),
            zones: map.zones.clone(),
            events: Vec::new(),
            errors: Vec::new(),
            corrections: 0,
        }
    }

    fn send(&mut self, msg: &Message) {
        self.endpoint
            .send(Channel::Event, &msg.encode().unwrap())
            .unwrap();
    }

    fn phase(&self) -> Option<Phase> {
        self.mirror.as_ref().and_then(MatchMirror::phase)
    }

    fn step(&mut self, intent: Intent) {
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
                Message::ServerHello {
                    player_id, mode, ..
                } => {
                    self.player = Some(player_id);
                    self.mode = Some(ModeId(mode));
                    let entry = self.registry.get(ModeId(mode)).unwrap().clone();
                    self.mirror = Some(MatchMirror::new(&entry));
                    self.send(&Message::Ready);
                    let class = ModeMessage {
                        mode: ModeId(mode),
                        bytes: self.class.encode(),
                    };
                    self.send(&Message::Event(Event::Mode(class.encode())));
                }
                Message::Event(Event::MatchState(bytes)) => {
                    self.mirror
                        .as_mut()
                        .unwrap()
                        .apply_state_bytes(&bytes)
                        .unwrap();
                }
                Message::Event(e) => {
                    let event = GameEvent::from_protocol(&e).expect("a game event");
                    self.events.push(event.clone());
                    self.mirror.as_mut().unwrap().on_event(event);
                }
                other => panic!("unexpected {other:?}"),
            }
        }
        while let Some(bytes) = self.endpoint.recv(Channel::Snapshot) {
            let Ok(snap) = self.decoder.decode_bytes(&bytes) else {
                continue;
            };
            let Some(me) = self.player else { continue };
            let Some(e) = snap.entity(me) else { continue };
            let server = AuthoritativeState {
                tick: snap.tick,
                last_command: snap.last_command,
                state: PredictedState {
                    position: e.position,
                    velocity: e.velocity,
                    yaw: e.yaw,
                    pitch: e.pitch,
                    weapon: e.weapon,
                    on_ground: e.on_ground,
                },
            };
            match self.prediction.as_mut() {
                None => {
                    let mut p = Prediction::new(me, SEED, self.statics.clone())
                        .with_zones(self.zones.clone());
                    p.start(Tick(snap.tick.0 + LEAD), server.state);
                    self.prediction = Some(p);
                }
                Some(p) => {
                    if let Some(out) = p.reconcile(&server) {
                        self.errors.push(out.position_error);
                        self.corrections += u32::from(out.corrected);
                    }
                }
            }
        }
        let (Some(me), Some(p), Some(mirror)) =
            (self.player, self.prediction.as_mut(), self.mirror.as_ref())
        else {
            self.endpoint.update(Duration::ZERO).unwrap();
            return;
        };
        let alive = mirror.vitals(me).is_none_or(|v| v.alive);
        p.set_frozen(!mirror.movement_allowed() || !alive);
        let (run, jump) = mirror.client().move_scale(me);
        p.set_move_scale(MoveScale { run, jump });
        if let Some(tick) = p.next_tick() {
            p.predict(InputCommand {
                tick,
                buttons: intent.buttons,
                move_x: 0,
                move_y: intent.move_y,
                yaw: quantize_yaw(intent.yaw),
                pitch: quantize_pitch_command(intent.pitch),
                weapon_slot: intent.slot,
            })
            .unwrap();
            for command in p.unacknowledged() {
                let packet = InputPacket {
                    command,
                    ack: self.decoder.ack(),
                };
                let _ = self.endpoint.send(Channel::Input, &packet.encode());
            }
        }
        self.endpoint.update(Duration::ZERO).unwrap();
    }

    fn mode_msgs(&self) -> Vec<ServerMsg> {
        self.events
            .iter()
            .filter_map(|e| match e {
                GameEvent::Mode(b) => ServerMsg::decode(b),
                _ => None,
            })
            .collect()
    }
}

/// Everything a run produced.
struct Outcome {
    world_hash: u64,
    events: Vec<Vec<u8>>,
    humans: [Human; 2],
    bots: Vec<NetId>,
    rocket: usize,
}

fn play() -> Outcome {
    let dir = std::env::temp_dir().join(format!("faset-4-11-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("yard.ron"), MAP).unwrap();
    let catalog = Catalog::scan(&dir);
    let map = catalog.load("yard").unwrap().config;
    let settings = Settings {
        game_mode: "fps.team".into(),
        rotation: vec!["yard".into()],
        rotation_seconds: 120,
        bots: BOTS,
        bot_level: 9,
        max_players: 8,
        seed: SEED,
        ..Settings::default()
    };
    let net = MemoryNetwork::new();
    let server_end = net.endpoint();
    let addr = server_end.addr();
    let mut server = Dedicated::new(settings, catalog, vec![server_end.into()]).unwrap();
    let rules = FpsRules::from_catalog(&faset_mode_fps::class::Catalog::builtin());
    let class = |id: &str| rules.classes().iter().position(|c| c.id == id).unwrap() as u8;
    let trooper = &rules.classes()[usize::from(class("trooper"))];
    let baguette = trooper
        .secondaries
        .iter()
        .position(|w| w == "baguette")
        .unwrap() as u8;
    let mut humans = [
        Human::new(
            &net,
            addr,
            ClassMsg {
                class: class("boomer"),
                secondary: None,
            },
            &map,
        ),
        Human::new(
            &net,
            addr,
            ClassMsg {
                class: class("trooper"),
                secondary: Some(baguette),
            },
            &map,
        ),
    ];

    let mut playing_since: Option<u32> = None;
    for _ in 0..60 * 60 {
        let tick = server.server().world().tick().0;
        let t = playing_since.map(|s| tick.saturating_sub(s));
        for (i, h) in humans.iter_mut().enumerate() {
            let intent = match t {
                // First second of play: walk two metres onto the armour.
                Some(t) if t < 30 => Intent {
                    move_y: 127,
                    ..Intent::default()
                },
                // The boomer fires rockets at its feet: blasts, knockback,
                // self damage and sooner or later a respawn.
                // (A semi-automatic weapon: one press per rocket.)
                Some(t) if i == 0 && (120..900).contains(&t) => Intent {
                    pitch: -1.2,
                    buttons: if t % 30 < 15 { BUTTON_FIRE } else { 0 },
                    ..Intent::default()
                },
                // The trooper sprays its rifle across the yard, then swings
                // the baguette.
                Some(t) if i == 1 && (120..600).contains(&t) => Intent {
                    yaw: 0.6 + (t as f32 * 0.01).sin(),
                    buttons: if t % 20 < 10 { BUTTON_FIRE } else { 0 },
                    ..Intent::default()
                },
                Some(t) if i == 1 && (600..700).contains(&t) => Intent {
                    buttons: if t % 40 < 20 { BUTTON_FIRE } else { 0 },
                    slot: 1,
                    ..Intent::default()
                },
                _ => Intent::default(),
            };
            h.step(intent);
        }
        server.tick();
        if playing_since.is_none() && humans.iter().all(|h| h.phase() == Some(Phase::Playing)) {
            playing_since = Some(server.server().world().tick().0);
        }
        if t.is_some_and(|t| t > 40 * 60) {
            break;
        }
    }
    std::fs::remove_dir_all(&dir).ok();
    assert!(playing_since.is_some(), "the match never started");
    let rocket = rules.weapon_index("boomer_rocket_launcher").unwrap();
    let events = humans[0].events.iter().map(GameEvent::encode).collect();
    Outcome {
        world_hash: server.server().world().state_hash(),
        events,
        bots: server.bots().to_vec(),
        humans,
        rocket,
    }
}

#[test]
fn a_team_match_with_humans_and_bots_runs_through_the_real_server() {
    let run = play();
    let [boomer, trooper] = &run.humans;
    let me = [boomer.player.unwrap(), trooper.player.unwrap()];

    // The bots alone start the countdown; the humans join during it (the
    // replicated match state tells them) and get the start of play over
    // the event channel.
    let phases: Vec<Phase> = boomer
        .events
        .iter()
        .filter_map(|e| match e {
            GameEvent::PhaseChanged { phase, .. } => Some(*phase),
            _ => None,
        })
        .collect();
    assert_eq!(phases.first(), Some(&Phase::Playing), "{phases:?}");

    // Rockets: typed projectile and explosion events, both clients.
    for h in &run.humans {
        let rockets = h
            .events
            .iter()
            .filter(|e| {
                matches!(e, GameEvent::Projectile(p)
                    if p.shooter == me[0] && usize::from(p.weapon) == run.rocket)
            })
            .count();
        let blasts = h
            .events
            .iter()
            .filter(|e| matches!(e, GameEvent::Explosion { owner, .. } if *owner == me[0]))
            .count();
        assert!(
            rockets >= 5 && blasts >= 5,
            "{rockets} rockets, {blasts} blasts"
        );
    }
    // The blasts hurt (and pushed) their owner: self damage hits.
    let msgs = boomer.mode_msgs();
    assert!(msgs
        .iter()
        .any(|m| matches!(m, ServerMsg::Hit { attacker, victim, .. } if *attacker == me[0] && *victim == me[0])));
    // Both humans walked onto an armour pickup, confirmed by the server.
    for id in me {
        assert!(
            msgs.iter()
                .any(|m| matches!(m, ServerMsg::PickupTaken { player, .. } if *player == id)),
            "{id:?} took no pickup"
        );
    }
    // The bots fought: some bot landed a hit on someone.
    assert!(
        msgs.iter()
            .any(|m| matches!(m, ServerMsg::Hit { attacker, .. } if run.bots.contains(attacker))),
        "no bot hit anyone"
    );
    // Prediction runs the server's controller: most snapshots confirm the
    // prediction exactly; corrections come only from what the client
    // cannot predict (knockback, death, respawn teleports, phase edges),
    // not from loops.
    for h in &run.humans {
        let mut sorted = h.errors.clone();
        sorted.sort_by(f32::total_cmp);
        let median = sorted[sorted.len() / 2];
        let p90 = sorted[sorted.len() * 9 / 10];
        let mean = h.errors.iter().sum::<f32>() / h.errors.len() as f32;
        println!(
            "{:?}: {} snapshots, median error {median:.4} m, p90 {p90:.4} m, mean {mean:.4} m, \
             {} corrections",
            h.player,
            h.errors.len(),
            h.corrections
        );
        assert!(h.errors.len() > 600);
        assert!(median < 0.005 && p90 < 0.01, "median {median}, p90 {p90}");
        assert!(h.corrections < 60, "{} corrections", h.corrections);
    }

    // Deterministic: the same run again gives the same world and events.
    let again = play();
    assert_eq!(again.world_hash, run.world_hash);
    assert_eq!(again.events, run.events);
}

//! Lot 4.1 acceptance: a race match on the real server with two in-memory
//! clients, through the whole phase cycle, timed in simulation ticks.

use std::time::Duration;

use faset_core::glam::Vec3;
use faset_core::{NetId, Tick, DT};
use faset_minigame_api::{
    EndReason, GameEvent, GameModeClient, HudCtx, Match, MatchConfig, MatchState, ObjectiveZone,
    Phase, Registry, COUNTDOWN_TICKS, LOADING_TICKS, MAP_CHANGE_TICKS, ROUND_END_TICKS,
};
use faset_mode_race::{RaceMode, Stage, ZONE_A, ZONE_B};
use faset_net::{Channel, ClientEndpoint, MemoryNetwork, MemoryTransport};
use faset_protocol::{InputCommand, Message};
use faset_server::{
    build_fingerprint, InputPacket, Server, ServerConfig, ServerNotice, SnapshotDecoder,
};

/// A minimal headless client: handshake, then one command per frame.
struct Client {
    endpoint: ClientEndpoint<MemoryTransport>,
    decoder: SnapshotDecoder,
    player: Option<NetId>,
    latest: Option<Tick>,
    since: u32,
}

impl Client {
    fn new(net: &MemoryNetwork, server: faset_net::MemoryAddr) -> Self {
        let mut endpoint = ClientEndpoint::new(net.endpoint(), server);
        let hello = Message::ClientHello {
            protocol_version: faset_protocol::PROTOCOL_VERSION,
            build_fingerprint: build_fingerprint(),
        };
        endpoint
            .send(Channel::Event, &hello.encode().unwrap())
            .unwrap();
        Self {
            endpoint,
            decoder: SnapshotDecoder::new(),
            player: None,
            latest: None,
            since: 0,
        }
    }

    /// One frame; `axes` is `(move_x, move_y)` with yaw 0 (-Z forward).
    fn step(&mut self, axes: Option<(i8, i8)>) {
        if let (Some((move_x, move_y)), Some(latest)) = (axes, self.latest) {
            let command = InputCommand {
                tick: Tick(latest.0 + self.since + 2),
                buttons: 0,
                move_x,
                move_y,
                yaw: 0,
                pitch: 0,
                weapon_slot: 0,
            };
            let packet = InputPacket {
                command,
                ack: self.decoder.ack(),
            };
            self.endpoint
                .send(Channel::Input, &packet.encode())
                .unwrap();
        }
        self.endpoint.update(Duration::from_secs_f32(DT)).unwrap();
        self.since += 1;
        while let Some(bytes) = self.endpoint.recv(Channel::Event) {
            if let Ok(Message::ServerHello { player_id, .. }) = Message::decode(&bytes) {
                self.player = Some(player_id);
                let ready = Message::Ready.encode().unwrap();
                self.endpoint.send(Channel::Event, &ready).unwrap();
            }
        }
        while let Some(bytes) = self.endpoint.recv(Channel::Snapshot) {
            let snap = self
                .decoder
                .decode(&Message::decode(&bytes).unwrap())
                .unwrap();
            if self.latest.is_none_or(|l| snap.tick > l) {
                self.latest = Some(snap.tick);
                self.since = 0;
            }
        }
    }
}

fn zone(id: &str, x: (f32, f32)) -> ObjectiveZone {
    ObjectiveZone {
        id: id.into(),
        min: Vec3::new(x.0, -1.0, -12.0),
        max: Vec3::new(x.1, 4.0, -8.0),
    }
}

/// Axes steering toward `(x, z)`, `None` once there.
fn steer(from: Vec3, to: (f32, f32)) -> Option<(i8, i8)> {
    // Wider than the ~3 ticks of command latency at 5 m/s.
    let axis = |d: f32| {
        if d.abs() < 0.4 {
            0
        } else {
            127 * d.signum() as i8
        }
    };
    let (mx, my) = (axis(to.0 - from.x), axis(from.z - to.1));
    (mx != 0 || my != 0).then_some((mx, my))
}

#[test]
fn race_match_runs_the_full_phase_cycle() {
    let mut registry = Registry::new();
    let race_id = registry.register::<RaceMode>().unwrap();
    let entry = registry.find("race").unwrap();
    assert_eq!(entry.id, race_id);

    let net = MemoryNetwork::new();
    let server_t = net.endpoint();
    let server_addr = server_t.addr();
    let config = ServerConfig {
        mode: race_id.0,
        ..ServerConfig::default()
    };
    let spawn_points = config.map.spawn_points.clone();
    let mut server = Server::new(config, vec![server_t.into()]).unwrap();
    let match_config = MatchConfig {
        spawn_points,
        objectives: vec![zone(ZONE_A, (-2.0, 2.0)), zone(ZONE_B, (5.0, 8.0))],
        rotation: vec!["race-1".into(), "race-2".into()],
        ..MatchConfig::default()
    };
    let mut game = Match::new(entry, match_config).unwrap();
    let mut mirror = entry.create_client();

    let mut clients = [
        Client::new(&net, server_addr),
        Client::new(&net, server_addr),
    ];
    let mut phases: Vec<(Phase, u32)> = Vec::new();
    let mut runner = None;
    let mut cheater_in_b = None;
    let mut playing_frames = 0u32;
    let mut cheater_turned = false;

    for _ in 0..3000 {
        // Scripted inputs: moves only in `Playing`, otherwise stand still
        // (the server repeats the last command when one is missing).
        let ids: Vec<Option<NetId>> = clients.iter().map(|c| c.player).collect();
        for (i, client) in clients.iter_mut().enumerate() {
            let axes = match (game.phase(), ids[i]) {
                (Phase::Playing, Some(id)) => {
                    let pos = server.world().position(id).unwrap();
                    if i == 0 {
                        // The runner waits 1.5 s, then goes A, then B.
                        if playing_frames < 90 {
                            None
                        } else if stage(&game, id) == Some(Stage::Start) {
                            steer(pos, (0.0, -10.0))
                        } else {
                            steer(pos, (6.5, -10.0))
                        }
                    } else {
                        // The cheater goes straight to B, skipping A.
                        if !cheater_turned {
                            let leg = steer(pos, (6.5, 4.0));
                            cheater_turned = leg.is_none();
                            leg
                        } else {
                            steer(pos, (6.5, -10.0))
                        }
                    }
                }
                _ => None,
            };
            client.step(Some(axes.unwrap_or((0, 0))));
        }
        if game.phase() == Phase::Playing {
            playing_frames += 1;
        }

        server.tick();
        while let Some(notice) = server.poll_notice() {
            match notice {
                ServerNotice::Joined(id) => game.player_joined(server.world(), id),
                ServerNotice::Left(id) => game.player_left(server.world(), id),
                ServerNotice::Rejected(r) => panic!("rejected: {r:?}"),
            }
        }
        game.tick(server.world());
        let now = server.world().tick().0;
        for event in game.drain_events() {
            if let GameEvent::PhaseChanged { phase, end } = &event {
                if let Some(end) = end {
                    assert!(end.0 > now);
                }
                phases.push((*phase, now));
            }
            if let GameEvent::MatchEnded(result) = &event {
                assert_eq!(result.reason, EndReason::WinCondition);
                runner = clients[0].player;
                assert_eq!(result.winners, vec![runner.unwrap()]);
            }
            mirror.on_event(&event);
        }
        // Replicate the state through its wire layout.
        let state = MatchState::decode(&game.state().encode()).unwrap();
        mirror.apply_state(&state.mode_state);

        if let Some(cheater) = clients[1].player {
            let in_b = server
                .world()
                .position(cheater)
                .is_some_and(|p| zone(ZONE_B, (5.0, 8.0)).contains(p));
            if in_b && game.phase() == Phase::Playing && cheater_in_b.is_none() {
                cheater_in_b = Some(now);
            }
        }
        if phases.len() == 6 {
            break;
        }
    }

    let order: Vec<Phase> = phases.iter().map(|p| p.0).collect();
    assert_eq!(
        order,
        [
            Phase::Countdown,
            Phase::Playing,
            Phase::RoundEnd,
            Phase::MapChange,
            Phase::Loading,
            Phase::Warmup,
        ],
        "{phases:?}"
    );
    let at = |p: Phase| phases.iter().find(|x| x.0 == p).unwrap().1;
    assert_eq!(at(Phase::Playing) - at(Phase::Countdown), COUNTDOWN_TICKS);
    assert_eq!(COUNTDOWN_TICKS, 3 * 60);
    assert_eq!(at(Phase::MapChange) - at(Phase::RoundEnd), ROUND_END_TICKS);
    assert_eq!(ROUND_END_TICKS, 8 * 60);
    assert_eq!(at(Phase::Loading) - at(Phase::MapChange), MAP_CHANGE_TICKS);
    assert_eq!(at(Phase::Warmup) - at(Phase::Loading), LOADING_TICKS);

    // The runner won; the cheater stood in B earlier, without touching A.
    let runner = runner.expect("match ended on a result");
    let cheater_in_b = cheater_in_b.expect("cheater reached B");
    assert!(
        cheater_in_b < at(Phase::RoundEnd),
        "{cheater_in_b} vs {phases:?}"
    );
    assert_eq!(game.current_map(), Some("race-2"));
    assert_eq!(game.phase(), Phase::Warmup);
    // Back in warmup with a fresh race: both players rejoined, no winner.
    let mut hud = HudCtx::new();
    mirror.draw_hud(&mut hud);
    assert_eq!(hud.lines().len(), 2, "{:?}", hud.lines());
    assert!(hud.lines().iter().all(|l| l.ends_with("go to A")));
    assert_ne!(Some(runner), clients[1].player);
}

/// A player's stage, read from the replicated race state.
fn stage(game: &Match, id: NetId) -> Option<Stage> {
    let mut race = RaceMode::default();
    race.apply_state(&game.state().mode_state);
    race.stage(id)
}

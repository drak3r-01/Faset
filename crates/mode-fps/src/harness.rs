//! Headless match harness and the lot 4.7 acceptance tests.
//!
//! No window, no GPU: a [`Match`] driven tick by tick over a [`World`], and N
//! client-side mode instances fed with what the server replicates (the
//! state bytes and the events), exactly as a transport would deliver them.
//!
//! Gap: the `server` crate does not host a `Match` yet (lot 4.1), so the
//! harness stands in for the host and the net memory transport is not in the
//! loop. Shots cannot reach the mode through the framework either (no client
//! message hook), so [`Scripted`] injects the damage events a shot would
//! have produced through a thread-local inbox, at the start of the tick.

use std::cell::RefCell;
use std::collections::BTreeMap;

use faset_core::glam::Vec3;
use faset_core::NetId;
use faset_minigame_api::{
    EndReason, GameEvent, GameMode, GameModeClient, HudCtx, Match, MatchConfig, MatchResult,
    ModeCtx, ModeDescriptor, ObjectiveZone, Phase, Registry, SettingValue, TeamId,
    TIME_LIMIT_SETTING,
};
use faset_sim::World;

use crate::damage::{DamageEvent, DamageType, HitZone};
use crate::message::ServerMsg;
use crate::pickup::{PickupKind, KINDS};
use crate::play::{
    register_modes, Ffa, FpsPlay, Team, Variant, FFA_ID, KILL_SCORE_SETTING, MAX_PLAYERS,
    PICKUP_DELAY_SETTING, SCORE_LIMIT_SETTING, TEAM_ID,
};

enum Cmd {
    Damage {
        attacker: NetId,
        victim: NetId,
        amount: f32,
    },
    DrainReserve(NetId),
}

thread_local! {
    static INBOX: RefCell<Vec<Cmd>> = const { RefCell::new(Vec::new()) };
}

fn send(cmd: Cmd) {
    INBOX.with(|i| i.borrow_mut().push(cmd));
}

/// The mode under test: [`FpsPlay`] plus the scripted inputs.
#[derive(Debug, Default)]
struct Scripted<V: Variant>(FpsPlay<V>);

impl<V: Variant + Default> GameMode for Scripted<V> {
    fn descriptor() -> ModeDescriptor {
        FpsPlay::<V>::descriptor()
    }
    fn on_match_start(&mut self, ctx: &mut ModeCtx) {
        self.0.on_match_start(ctx);
    }
    fn on_player_join(&mut self, ctx: &mut ModeCtx, player: NetId) {
        self.0.on_player_join(ctx, player);
    }
    fn on_player_leave(&mut self, ctx: &mut ModeCtx, player: NetId) {
        self.0.on_player_leave(ctx, player);
    }
    fn on_tick(&mut self, ctx: &mut ModeCtx) {
        for cmd in INBOX.with(|i| std::mem::take(&mut *i.borrow_mut())) {
            match cmd {
                Cmd::Damage {
                    attacker,
                    victim,
                    amount,
                } => self.0.rules_mut().queue_damage([DamageEvent {
                    attacker,
                    victim,
                    amount,
                    kind: DamageType::Bullet,
                    zone: HitZone::Body,
                    position: Vec3::ZERO,
                }]),
                Cmd::DrainReserve(p) => {
                    self.0.rules_mut().slot_mut(p).unwrap().weapon.reserve = 0;
                }
            }
        }
        self.0.on_tick(ctx);
    }
    fn on_event(&mut self, ctx: &mut ModeCtx, event: &GameEvent) {
        GameMode::on_event(&mut self.0, ctx, event);
    }
    fn result(&self) -> Option<MatchResult> {
        self.0.result()
    }
    fn replicated_state(&self) -> Vec<u8> {
        self.0.replicated_state()
    }
}

impl<V: Variant + Default> GameModeClient for Scripted<V> {
    fn apply_state(&mut self, bytes: &[u8]) {
        self.0.apply_state(bytes);
    }
    fn on_event(&mut self, event: &GameEvent) {
        GameModeClient::on_event(&mut self.0, event);
    }
    fn draw_hud(&mut self, ui: &mut HudCtx) {
        self.0.draw_hud(ui);
    }
}

/// A server and its clients, in one thread.
struct Headless<V: Variant + Default> {
    m: Match,
    world: World,
    clients: BTreeMap<NetId, FpsPlay<V>>,
    /// Every event of the match with the tick it was drained at.
    events: Vec<(u32, GameEvent)>,
    /// Whether clients receive what the server replicates.
    deliver: bool,
}

fn zone(kind: PickupKind, at: Vec3) -> ObjectiveZone {
    ObjectiveZone {
        id: format!("pickup.{}", kind.name()),
        min: at - Vec3::splat(0.25),
        max: at + Vec3::splat(0.25),
    }
}

fn config(settings: &[(&str, i32)]) -> MatchConfig {
    let mut c = MatchConfig {
        spawn_points: vec![Vec3::new(30.0, 0.9, 5.0), Vec3::new(30.0, 0.9, -5.0)],
        rotation: vec!["a".into(), "b".into()],
        ..MatchConfig::default()
    };
    for &(k, v) in settings {
        c.settings.set(k, SettingValue::Int(v));
    }
    c
}

impl<V: Variant + Default> Headless<V> {
    fn new(count: u16, config: MatchConfig) -> Self {
        INBOX.with(|i| i.borrow_mut().clear());
        let mut registry = Registry::new();
        registry.register::<Scripted<V>>().unwrap();
        let entry = registry.find(V::ID).unwrap().clone();
        let mut m = Match::new(&entry, config).unwrap();
        let mut world = World::with_statics(7, faset_sim::StaticWorld::test_arena());
        let mut clients = BTreeMap::new();
        for i in 1..=count {
            let id = NetId(i);
            world.spawn_player(id, Self::park(i));
            m.player_joined(&world, id);
            clients.insert(id, FpsPlay::<V>::default());
        }
        Self {
            m,
            world,
            clients,
            events: Vec::new(),
            deliver: true,
        }
    }

    /// A resting place far from every pickup.
    fn park(i: u16) -> Vec3 {
        Vec3::new(20.0 + f32::from(i % 20), 0.9, 8.0 + f32::from(i / 20) * 3.0)
    }

    fn tick(&self) -> u32 {
        self.world.tick().0
    }

    fn step(&mut self) {
        self.world.step(&[]);
        self.m.tick(&self.world);
        let state = self.m.state().mode_state;
        let events = self.m.drain_events();
        let t = self.tick();
        if self.deliver {
            for c in self.clients.values_mut() {
                c.apply_state(&state);
                for e in &events {
                    GameModeClient::on_event(c, e);
                }
            }
        }
        self.events.extend(events.into_iter().map(|e| (t, e)));
    }

    fn run(&mut self, ticks: u32) {
        for _ in 0..ticks {
            self.step();
        }
    }

    fn run_until(&mut self, what: &str, mut done: impl FnMut(&Self) -> bool) {
        for _ in 0..200_000 {
            if done(self) {
                return;
            }
            self.step();
        }
        panic!("never reached: {what}");
    }

    /// Runs to the `Playing` phase plus the spawn protection (1.5 s).
    fn start_playing(&mut self) {
        self.run_until("playing", |h| h.m.phase() == Phase::Playing);
        self.run(95);
    }

    fn phases(&self) -> Vec<Phase> {
        self.events
            .iter()
            .filter_map(|(_, e)| match e {
                GameEvent::PhaseChanged { phase, .. } => Some(*phase),
                _ => None,
            })
            .collect()
    }

    fn kill(&mut self, attacker: u16, victim: u16) {
        send(Cmd::Damage {
            attacker: NetId(attacker),
            victim: NetId(victim),
            amount: 1000.0,
        });
    }

    fn client(&self, id: u16) -> &FpsPlay<V> {
        &self.clients[&NetId(id)]
    }

    fn mode_msgs(&self) -> Vec<(u32, ServerMsg)> {
        self.events
            .iter()
            .filter_map(|(t, e)| match e {
                GameEvent::Mode(b) => Some((*t, ServerMsg::decode(b)?)),
                _ => None,
            })
            .collect()
    }
}

const FULL_CYCLE: [Phase; 6] = [
    Phase::Countdown,
    Phase::Playing,
    Phase::RoundEnd,
    Phase::MapChange,
    Phase::Loading,
    Phase::Warmup,
];

#[test]
fn full_match_headless_ffa() {
    // First to 3 kills wins; the match then runs RoundEnd, MapChange,
    // Loading and back to Warmup.
    let mut h = Headless::<Ffa>::new(4, config(&[(SCORE_LIMIT_SETTING, 3)]));
    assert_eq!(h.m.phase(), Phase::Warmup);
    h.start_playing();
    h.kill(1, 2);
    h.kill(1, 3);
    h.kill(1, 4);
    h.run_until("round end", |h| h.m.phase() == Phase::RoundEnd);
    let result = h.m.result().cloned().unwrap();
    assert_eq!(result.winners, [NetId(1)]);
    assert_eq!(result.reason, EndReason::WinCondition);
    assert_eq!(result.winning_team, None);
    assert!(h
        .events
        .iter()
        .any(|(_, e)| matches!(e, GameEvent::MatchEnded(r) if *r == result)));
    // Clients saw the scores and the kills.
    h.run(2);
    assert_eq!(h.m.services().scores.get(NetId(1)), 3);
    assert!(!h.client(2).view(NetId(2)).unwrap().alive);
    h.run_until("warmup", |h| h.phases().ends_with(&FULL_CYCLE));
    assert_eq!(h.phases()[..6], FULL_CYCLE);
    assert_eq!(h.m.current_map(), Some("b"));
}

#[test]
fn ffa_ends_on_the_time_limit_and_reports_the_leaders() {
    let mut h = Headless::<Ffa>::new(
        3,
        config(&[(TIME_LIMIT_SETTING, 30), (SCORE_LIMIT_SETTING, 0)]),
    );
    h.start_playing();
    h.kill(2, 3);
    h.run_until("round end", |h| h.m.phase() == Phase::RoundEnd);
    let r = h.m.result().cloned().unwrap();
    assert_eq!(
        (r.winners, r.reason),
        (vec![NetId(2)], EndReason::TimeLimit)
    );
    // 30 s of play, counted from the start of the Playing phase.
    let at = |p: Phase| {
        h.events
            .iter()
            .find_map(|(t, e)| {
                matches!(e, GameEvent::PhaseChanged { phase, .. } if *phase == p).then_some(*t)
            })
            .unwrap()
    };
    assert_eq!(at(Phase::RoundEnd) - at(Phase::Playing), 30 * 60);
}

#[test]
fn full_match_headless_team() {
    let mut h = Headless::<Team>::new(4, config(&[(SCORE_LIMIT_SETTING, 2)]));
    h.start_playing();
    // Teams: {1, 3} and {2, 4}.
    assert_eq!(h.client(1).view(NetId(1)).unwrap().team, Some(TeamId(0)));
    assert_eq!(h.client(1).view(NetId(2)).unwrap().team, Some(TeamId(1)));
    h.kill(1, 2);
    h.kill(3, 4);
    h.run_until("round end", |h| h.m.phase() == Phase::RoundEnd);
    let r = h.m.result().cloned().unwrap();
    assert_eq!(r.winning_team, Some(TeamId(0)));
    assert_eq!(r.winners, [NetId(1), NetId(3)]);
    assert_eq!(r.reason, EndReason::WinCondition);
    h.run(2);
    assert_eq!(h.client(2).team_score(TeamId(0)), 2);
    assert_eq!(h.client(2).team_score(TeamId(1)), 0);
    h.run_until("warmup", |h| h.phases().ends_with(&FULL_CYCLE));
    assert_eq!(h.phases()[..6], FULL_CYCLE);
}

#[test]
fn team_time_limit_picks_the_best_team_and_a_tie_has_no_winner() {
    let settings = [(TIME_LIMIT_SETTING, 30), (SCORE_LIMIT_SETTING, 0)];
    let mut h = Headless::<Team>::new(4, config(&settings));
    h.start_playing();
    h.kill(2, 1);
    h.run_until("round end", |h| h.m.phase() == Phase::RoundEnd);
    let r = h.m.result().cloned().unwrap();
    assert_eq!(r.winning_team, Some(TeamId(1)));
    assert_eq!(r.winners, [NetId(2), NetId(4)]);
    assert_eq!(r.reason, EndReason::TimeLimit);

    let mut h = Headless::<Team>::new(4, config(&settings));
    h.start_playing();
    h.run_until("round end", |h| h.m.phase() == Phase::RoundEnd);
    let r = h.m.result().cloned().unwrap();
    assert!(r.winners.is_empty() && r.winning_team.is_none());
}

#[test]
fn friendly_fire_is_off_in_teams_and_everyone_is_a_target_in_ffa() {
    let hit = |a, v| {
        send(Cmd::Damage {
            attacker: NetId(a),
            victim: NetId(v),
            amount: 30.0,
        })
    };
    let settings = [(SCORE_LIMIT_SETTING, 0)];

    let mut t = Headless::<Team>::new(4, config(&settings));
    t.start_playing();
    hit(1, 3); // same team
    hit(1, 2); // enemy
    t.run(2);
    assert_eq!(t.client(1).view(NetId(3)).unwrap().health, 100);
    assert_eq!(t.client(1).view(NetId(2)).unwrap().health, 70);

    let mut f = Headless::<Ffa>::new(4, config(&settings));
    f.start_playing();
    hit(1, 3);
    hit(1, 2);
    f.run(2);
    assert_eq!(f.client(1).view(NetId(3)).unwrap().health, 70);
    assert_eq!(f.client(1).view(NetId(2)).unwrap().health, 70);
}

#[test]
fn score_per_kill_is_a_setting() {
    let mut h = Headless::<Ffa>::new(
        2,
        config(&[(KILL_SCORE_SETTING, 5), (SCORE_LIMIT_SETTING, 0)]),
    );
    h.start_playing();
    h.kill(1, 2);
    h.run(2);
    assert_eq!(h.m.services().scores.get(NetId(1)), 5);
}

#[test]
fn a_full_server_holds_the_margin_of_thirty_two_players() {
    const { assert!(MAX_PLAYERS >= 32) };
    let mut h = Headless::<Team>::new(32, config(&[(SCORE_LIMIT_SETTING, 0)]));
    h.start_playing();
    let c = h.client(1);
    assert_eq!(
        (0..32).filter(|&i| c.view(NetId(i + 1)).is_some()).count(),
        32
    );
    let team0 = (1..=32)
        .filter(|&i| c.view(NetId(i)).unwrap().team == Some(TeamId(0)))
        .count();
    assert_eq!(team0, 16);
}

#[test]
fn descriptors_are_registered_with_bounds_and_defaults() {
    let mut r = Registry::new();
    register_modes(&mut r).unwrap();
    let ffa = r.find(FFA_ID).unwrap();
    let team = r.find(TEAM_ID).unwrap();
    for e in [ffa, team] {
        assert!(e.descriptor.bots);
        assert_eq!(e.descriptor.map_labels, ["fps"]);
        let s = e.descriptor.default_settings();
        assert_eq!(s.int(TIME_LIMIT_SETTING), Some(300));
        assert_eq!(s.int(KILL_SCORE_SETTING), Some(1));
        assert_eq!(s.int(PICKUP_DELAY_SETTING), Some(100));
        let mut bad = faset_minigame_api::Settings::new();
        bad.set(SCORE_LIMIT_SETTING, SettingValue::Int(1001));
        assert!(e.descriptor.resolve(&bad).is_err());
    }
    assert_eq!(
        ffa.descriptor.default_settings().int(SCORE_LIMIT_SETTING),
        Some(25)
    );
    assert_eq!(
        team.descriptor.default_settings().int(SCORE_LIMIT_SETTING),
        Some(50)
    );
    assert!(register_modes(&mut r).is_err(), "no double registration");
}

/// A match with one pickup of `kind` at the origin and two players.
fn pickup_match(kind: PickupKind) -> Headless<Ffa> {
    let mut c = config(&[(TIME_LIMIT_SETTING, 3600), (SCORE_LIMIT_SETTING, 0)]);
    c.objectives = vec![zone(kind, Vec3::new(0.0, 1.0, 0.0))];
    let mut h = Headless::<Ffa>::new(2, c);
    h.start_playing();
    h
}

fn stand_on_pickup(h: &mut Headless<Ffa>, id: u16) {
    h.world.spawn_player(NetId(id), Vec3::new(0.3, 1.0, 0.0));
}

fn taken_at(h: &Headless<Ffa>) -> Option<(u32, NetId)> {
    h.mode_msgs().into_iter().find_map(|(t, m)| match m {
        ServerMsg::PickupTaken { index: 0, player } => Some((t, player)),
        _ => None,
    })
}

#[test]
fn pickup_is_decided_by_the_server_and_shown_only_after_confirmation() {
    let mut h = pickup_match(PickupKind::Armour);
    // The server confirms, but the message has not reached the client yet.
    h.deliver = false;
    stand_on_pickup(&mut h, 1);
    h.run(2);
    assert!(taken_at(&h).is_some(), "the server took it");
    assert!(
        !h.client(1).pickup_taken(0),
        "the client waits for the server"
    );
    assert!(!h.client(2).pickup_taken(0));
    // A client never decides on its own, whatever its player's position.
    let mut lone = FpsPlay::<Ffa>::default();
    lone.apply_state(&h.m.state().mode_state);
    assert!(lone.pickup_taken(0), "the replicated state carries it");
    let mut blank = FpsPlay::<Ffa>::default();
    assert!(!blank.pickup_taken(0));
    GameModeClient::on_event(&mut blank, &GameEvent::Feed("x".into()));
    assert!(!blank.pickup_taken(0));
    // Confirmation arrives.
    h.deliver = true;
    h.step();
    assert!(h.client(1).pickup_taken(0));
    assert_eq!(h.client(1).view(NetId(1)).unwrap().armour, 50);
}

#[test]
fn only_the_confirmation_event_marks_a_pickup_taken_on_the_client() {
    let mut c = FpsPlay::<Ffa>::default();
    let mut h = pickup_match(PickupKind::Speed);
    c.apply_state(&h.m.state().mode_state);
    assert!(!c.pickup_taken(0));
    h.deliver = false;
    stand_on_pickup(&mut h, 1);
    h.run(2);
    let (_, player) = taken_at(&h).unwrap();
    let confirm = ServerMsg::PickupTaken { index: 0, player };
    GameModeClient::on_event(&mut c, &GameEvent::Mode(confirm.encode()));
    assert!(c.pickup_taken(0));
    GameModeClient::on_event(
        &mut c,
        &GameEvent::Mode(ServerMsg::PickupRespawned { index: 0 }.encode()),
    );
    assert!(!c.pickup_taken(0));
}

#[test]
fn each_pickup_kind_applies_its_effect_and_returns_after_its_delay() {
    for kind in KINDS {
        let mut h = pickup_match(kind);
        // A wasted pickup stays on the map: full health, full reserve.
        if matches!(kind, PickupKind::Heal | PickupKind::Ammo) {
            stand_on_pickup(&mut h, 1);
            h.run(5);
            assert!(taken_at(&h).is_none(), "{kind:?} is wasted at full");
            send(if kind == PickupKind::Heal {
                Cmd::Damage {
                    attacker: NetId(2),
                    victim: NetId(1),
                    amount: 60.0,
                }
            } else {
                Cmd::DrainReserve(NetId(1))
            });
        } else {
            stand_on_pickup(&mut h, 1);
        }
        h.run_until("taken", |h| taken_at(h).is_some());
        let (t0, who) = taken_at(&h).unwrap();
        assert_eq!(who, NetId(1));
        h.world.spawn_player(NetId(1), Vec3::new(30.0, 0.9, 0.0));
        let v = h.client(2).view(NetId(1)).unwrap();
        assert!(h.client(2).pickup_taken(0));

        match kind {
            PickupKind::Heal => assert_eq!(v.health, 90, "40 hp + 50"),
            PickupKind::Armour => assert_eq!(v.armour, 50),
            PickupKind::Ammo => {}
            PickupKind::Speed => assert_eq!(v.bonus_tenths, [100, 0, 0, 0]),
            PickupKind::Damage => assert_eq!(v.bonus_tenths, [0, 150, 0, 0]),
            PickupKind::SuperJump => assert_eq!(v.bonus_tenths, [0, 0, 150, 0]),
            PickupKind::BigHead => {
                assert_eq!(v.bonus_tenths, [0, 0, 0, 100]);
                assert!(h.client(2).heads_enlarged_for(NetId(1)));
                assert!(!h.client(2).heads_enlarged_for(NetId(2)));
            }
        }

        // Timed bonuses end after 10 or 15 s.
        if let Some(d) = kind.duration_seconds() {
            let slot = match kind {
                PickupKind::Speed => 0,
                PickupKind::Damage => 1,
                PickupKind::SuperJump => 2,
                _ => 3,
            };
            h.run_until("bonus end", |h| {
                h.client(2).view(NetId(1)).unwrap().bonus_tenths[slot] == 0
            });
            let ended = h.tick() - t0;
            let expected = (d * 60.0) as u32;
            assert!(
                ended.abs_diff(expected) <= 1,
                "{kind:?} lasted {ended} ticks"
            );
        }

        // The pickup comes back after its delay, no earlier.
        h.run_until("respawn", |h| {
            h.mode_msgs()
                .iter()
                .any(|(_, m)| matches!(m, ServerMsg::PickupRespawned { index: 0 }))
        });
        let back = h
            .mode_msgs()
            .iter()
            .find_map(|(t, m)| matches!(m, ServerMsg::PickupRespawned { index: 0 }).then_some(*t))
            .unwrap();
        let expected = (kind.respawn_seconds() * 60.0) as u32;
        assert!(
            (back - t0).abs_diff(expected) <= 1,
            "{kind:?} came back after {} ticks",
            back - t0
        );
        assert!(!h.client(2).pickup_taken(0));
    }
}

#[test]
fn damage_bonus_scales_damage_and_death_removes_it() {
    let mut h = pickup_match(PickupKind::Damage);
    stand_on_pickup(&mut h, 1);
    h.run_until("taken", |h| taken_at(h).is_some());
    h.world.spawn_player(NetId(1), Vec3::new(30.0, 0.9, 0.0));
    send(Cmd::Damage {
        attacker: NetId(1),
        victim: NetId(2),
        amount: 20.0,
    });
    h.run(2);
    assert_eq!(
        h.client(1).view(NetId(2)).unwrap().health,
        70,
        "20 x 1.5 = 30"
    );
    // Dying drops the bonus.
    send(Cmd::Damage {
        attacker: NetId(2),
        victim: NetId(1),
        amount: 1000.0,
    });
    h.run(2);
    assert_eq!(h.client(1).view(NetId(1)).unwrap().bonus_tenths, [0; 4]);
}

#[test]
fn the_pickup_delay_setting_stretches_the_return() {
    let mut c = config(&[
        (TIME_LIMIT_SETTING, 3600),
        (SCORE_LIMIT_SETTING, 0),
        (PICKUP_DELAY_SETTING, 200),
    ]);
    c.objectives = vec![zone(PickupKind::Armour, Vec3::new(0.0, 1.0, 0.0))];
    let mut h = Headless::<Ffa>::new(2, c);
    h.start_playing();
    stand_on_pickup(&mut h, 1);
    h.run_until("taken", |h| taken_at(h).is_some());
    let (t0, _) = taken_at(&h).unwrap();
    h.world.spawn_player(NetId(1), Vec3::new(30.0, 0.9, 0.0));
    h.run_until("respawn", |h| {
        h.mode_msgs()
            .iter()
            .any(|(_, m)| matches!(m, ServerMsg::PickupRespawned { .. }))
    });
    assert!((h.tick() - t0).abs_diff(90 * 60) <= 2);
}

#[test]
fn rules_apply_each_instant_effect_and_expose_the_multipliers() {
    use crate::rules::FpsRules;
    let mut r = FpsRules::default();
    let p = NetId(1);
    r.join(p, 0.0);
    // Ammo: the reserve goes back to full, and a full reserve refuses.
    let full = r.weapons()[0].reserve;
    assert!(!r.apply_pickup(p, PickupKind::Ammo, 0.0));
    r.slot_mut(p).unwrap().weapon.reserve = 3;
    assert!(r.apply_pickup(p, PickupKind::Ammo, 0.0));
    assert_eq!(r.slot(p).unwrap().weapon.reserve, full);
    // Heal is capped at max health; armour absorbs 50, capped at 100.
    r.slot_mut(p).unwrap().combatant.health = 80.0;
    assert!(r.apply_pickup(p, PickupKind::Heal, 0.0));
    assert_eq!(r.slot(p).unwrap().combatant.health, 100.0);
    assert!(r.apply_pickup(p, PickupKind::Armour, 0.0));
    assert!(r.apply_pickup(p, PickupKind::Armour, 0.0));
    assert!(!r.apply_pickup(p, PickupKind::Armour, 0.0));
    assert_eq!(r.slot(p).unwrap().combatant.armour, 100.0);
    // Movement bonuses.
    assert_eq!(
        (r.speed_multiplier(p, 1.0), r.jump_multiplier(p, 1.0)),
        (1.0, 1.0)
    );
    r.apply_pickup(p, PickupKind::Speed, 1.0);
    r.apply_pickup(p, PickupKind::SuperJump, 1.0);
    assert_eq!(r.speed_multiplier(p, 10.9), 1.3);
    assert_eq!(r.speed_multiplier(p, 11.1), 1.0);
    assert_eq!(r.jump_multiplier(p, 15.9), 1.5);
    assert_eq!(r.jump_multiplier(p, 16.1), 1.0);
    assert!(!r.apply_pickup(NetId(9), PickupKind::Speed, 0.0));
}

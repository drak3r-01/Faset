//! [`Match`]: the server-side driver of a mode and of the phase cycle.

use faset_core::glam::Vec3;
use faset_core::{NetId, Tick, TICK_RATE};
use faset_sim::World;

use crate::ctx::{ModeCtx, ObjectiveZone, Outbox};
use crate::descriptor::{ModeDescriptor, Settings, SettingsError, TIME_LIMIT_SETTING};
use crate::event::{EndReason, GameEvent, MatchResult};
use crate::message::{MatchState, ModeId, ModeMessage};
use crate::phase::Phase;
use crate::registry::ModeEntry;
use crate::services::{Services, SpawnPoints};
use crate::GameMode;

/// Players needed to leave `Warmup` (decision).
pub const DEFAULT_MIN_PLAYERS: usize = 2;
/// `Playing` time limit when the mode declares no
/// [`TIME_LIMIT_SETTING`]: 5 min (decision).
pub const DEFAULT_TIME_LIMIT_TICKS: u32 = 5 * 60 * TICK_RATE as u32;

/// How a match is set up by the host.
#[derive(Debug, Clone)]
pub struct MatchConfig {
    /// Players needed to start the countdown.
    pub min_players: usize,
    /// `Playing` time limit, overridden by the mode's
    /// [`TIME_LIMIT_SETTING`] (seconds) when it declares one.
    pub time_limit_ticks: u32,
    /// Spawn points of the map.
    pub spawn_points: Vec<Vec3>,
    /// Objective zones of the map.
    pub objectives: Vec<ObjectiveZone>,
    /// Map rotation; `MapChange` moves to the next entry, wrapping.
    pub rotation: Vec<String>,
    /// Setting values; missing keys take the schema defaults.
    pub settings: Settings,
}

impl Default for MatchConfig {
    fn default() -> Self {
        Self {
            min_players: DEFAULT_MIN_PLAYERS,
            time_limit_ticks: DEFAULT_TIME_LIMIT_TICKS,
            spawn_points: Vec::new(),
            objectives: Vec::new(),
            rotation: Vec::new(),
            settings: Settings::new(),
        }
    }
}

/// One running match: the mode instance, the phase cycle and the services.
///
/// The host calls [`tick`](Self::tick) once per simulation tick, after the
/// world step, so phase times are counted in world ticks. A timed phase
/// entered at tick `t` with duration `d` hands over at tick `t + d`.
/// After `Loading` the mode is recreated from its registry entry and the
/// present players join it again.
pub struct Match {
    entry: ModeEntry,
    mode: Box<dyn GameMode>,
    config: MatchConfig,
    settings: Settings,
    time_limit: u32,
    phase: Phase,
    phase_end: Option<Tick>,
    players: Vec<NetId>,
    services: Services,
    out: Outbox,
    result: Option<MatchResult>,
    map: usize,
}

impl std::fmt::Debug for Match {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Match")
            .field("mode", &self.entry.descriptor.id)
            .field("phase", &self.phase)
            .field("phase_end", &self.phase_end)
            .field("players", &self.players)
            .field("result", &self.result)
            .finish_non_exhaustive()
    }
}

impl Match {
    /// A match of `entry`'s mode, in `Warmup`.
    ///
    /// # Errors
    /// Settings that do not fit the mode's schema.
    pub fn new(entry: &ModeEntry, config: MatchConfig) -> Result<Self, SettingsError> {
        let settings = entry.descriptor.resolve(&config.settings)?;
        let time_limit = settings
            .int(TIME_LIMIT_SETTING)
            .and_then(|s| u32::try_from(s).ok())
            .map_or(config.time_limit_ticks, |s| {
                s.saturating_mul(u32::from(TICK_RATE))
            });
        let services = Services {
            spawns: SpawnPoints::new(config.spawn_points.clone()),
            ..Services::default()
        };
        Ok(Self {
            entry: entry.clone(),
            mode: entry.create_server(),
            config,
            settings,
            time_limit,
            phase: Phase::Warmup,
            phase_end: None,
            players: Vec::new(),
            services,
            out: Outbox::default(),
            result: None,
            map: 0,
        })
    }

    /// The mode's numeric id.
    #[must_use]
    pub fn mode_id(&self) -> ModeId {
        self.entry.id
    }

    /// The mode's descriptor.
    #[must_use]
    pub fn descriptor(&self) -> &ModeDescriptor {
        &self.entry.descriptor
    }

    /// Resolved settings.
    #[must_use]
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// `Playing` time limit in ticks.
    #[must_use]
    pub fn time_limit_ticks(&self) -> u32 {
        self.time_limit
    }

    /// Current phase.
    #[must_use]
    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// End tick of the current phase, if timed.
    #[must_use]
    pub fn phase_end(&self) -> Option<Tick> {
        self.phase_end
    }

    /// Whether players may move now (`Warmup`, `Playing`).
    #[must_use]
    pub fn movement_allowed(&self) -> bool {
        self.phase.movement_allowed()
    }

    /// Result of the last `Playing` phase, until the next one starts.
    #[must_use]
    pub fn result(&self) -> Option<&MatchResult> {
        self.result.as_ref()
    }

    /// Players in the match, in join order.
    #[must_use]
    pub fn players(&self) -> &[NetId] {
        &self.players
    }

    /// The map of the rotation being played, `None` without a rotation.
    #[must_use]
    pub fn current_map(&self) -> Option<&str> {
        self.config.rotation.get(self.map).map(String::as_str)
    }

    /// The services (score table, teams, ...).
    #[must_use]
    pub fn services(&self) -> &Services {
        &self.services
    }

    /// The state to replicate to every client.
    #[must_use]
    pub fn state(&self) -> MatchState {
        MatchState {
            phase: self.phase,
            phase_end: self.phase_end,
            mode_state: self.mode.replicated_state(),
        }
    }

    /// Events to replicate since the last call, in order.
    pub fn drain_events(&mut self) -> Vec<GameEvent> {
        std::mem::take(&mut self.out.log)
    }

    /// Mode messages to send since the last call.
    pub fn drain_messages(&mut self) -> Vec<ModeMessage> {
        std::mem::take(&mut self.out.messages)
    }

    /// Respawn requests the host applies to the world.
    pub fn take_respawns(&mut self) -> Vec<(NetId, Vec3)> {
        self.services.spawns.take_requests()
    }

    /// A player entered the match. Ignored if already in it.
    pub fn player_joined(&mut self, world: &World, player: NetId) {
        if self.players.contains(&player) {
            return;
        }
        self.players.push(player);
        self.services.scores.enter(player);
        self.framework_event(world, GameEvent::PlayerJoined(player));
        self.call(world, |m, c| m.on_player_join(c, player));
    }

    /// A player left the match. Ignored if not in it.
    pub fn player_left(&mut self, world: &World, player: NetId) {
        let Some(i) = self.players.iter().position(|&p| p == player) else {
            return;
        };
        self.players.remove(i);
        self.services.scores.remove(player);
        self.services.teams.remove(player);
        self.framework_event(world, GameEvent::PlayerLeft(player));
        self.call(world, |m, c| m.on_player_leave(c, player));
    }

    /// Advances one simulation tick, `world.tick()` being the current tick:
    /// dispatches the mode's events of the previous tick, then runs the
    /// phase. `Warmup` goes to `Countdown` once [`MatchConfig::min_players`]
    /// are in (back to `Warmup` if they drop below during the countdown);
    /// the mode's [`on_tick`](GameMode::on_tick) runs during `Playing` only.
    pub fn tick(&mut self, world: &World) {
        let now = world.tick();
        for event in std::mem::take(&mut self.out.emitted) {
            self.call(world, |m, c| m.on_event(c, &event));
        }
        let due = self.phase_end.is_some_and(|end| now >= end);
        let enough = self.players.len() >= self.config.min_players;
        match self.phase {
            Phase::Warmup if enough => self.enter(world, Phase::Countdown),
            Phase::Countdown if !enough => self.enter(world, Phase::Warmup),
            Phase::Countdown if due => self.enter(world, Phase::Playing),
            Phase::Playing => {
                self.call(world, |m, c| m.on_tick(c));
                if let Some(result) = self.mode.result() {
                    self.end(world, result);
                } else if due {
                    let result = MatchResult {
                        winners: self.services.scores.leaders(),
                        winning_team: None,
                        reason: EndReason::TimeLimit,
                    };
                    self.end(world, result);
                }
            }
            Phase::RoundEnd if due => {
                self.map = (self.map + 1) % self.config.rotation.len().max(1);
                let map = self.current_map().unwrap_or_default().to_owned();
                self.framework_event(world, GameEvent::MapChanged { map });
                self.enter(world, Phase::MapChange);
            }
            Phase::MapChange if due => self.enter(world, Phase::Loading),
            Phase::Loading if due => {
                self.reset(world);
                self.enter(world, Phase::Warmup);
            }
            _ => {}
        }
    }

    fn end(&mut self, world: &World, result: MatchResult) {
        self.result = Some(result.clone());
        self.framework_event(world, GameEvent::MatchEnded(result));
        self.enter(world, Phase::RoundEnd);
    }

    fn enter(&mut self, world: &World, phase: Phase) {
        let now = world.tick();
        let ticks = match phase {
            Phase::Playing => Some(self.time_limit),
            p => p.fixed_ticks(),
        };
        self.phase = phase;
        self.phase_end = ticks.map(|t| Tick(now.0.saturating_add(t)));
        tracing::debug!(
            mode = self.entry.descriptor.id,
            ?phase,
            tick = now.0,
            "match phase"
        );
        self.framework_event(
            world,
            GameEvent::PhaseChanged {
                phase,
                end: self.phase_end,
            },
        );
        if phase == Phase::Playing {
            self.result = None;
            self.call(world, |m, c| m.on_match_start(c));
        }
    }

    /// A fresh mode instance and services; present players join again.
    fn reset(&mut self, world: &World) {
        self.mode = self.entry.create_server();
        self.services = Services {
            spawns: SpawnPoints::new(self.config.spawn_points.clone()),
            ..Services::default()
        };
        self.out.emitted.clear();
        self.result = None;
        for player in self.players.clone() {
            self.services.scores.enter(player);
            self.call(world, |m, c| m.on_player_join(c, player));
        }
    }

    fn framework_event(&mut self, world: &World, event: GameEvent) {
        self.out.log.push(event.clone());
        self.call(world, |m, c| m.on_event(c, &event));
    }

    fn call(&mut self, world: &World, f: impl FnOnce(&mut dyn GameMode, &mut ModeCtx)) {
        let mut ctx = ModeCtx {
            tick: world.tick(),
            phase: self.phase,
            mode: self.entry.id,
            world,
            players: &self.players,
            objectives: &self.config.objectives,
            settings: &self.settings,
            services: &mut self.services,
            out: &mut self.out,
        };
        f(self.mode.as_mut(), &mut ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::{SettingKind, SettingSpec, SettingValue};
    use crate::{GameModeClient, HudCtx, Registry};

    /// Scores one point per tick for the first player; never wins.
    #[derive(Default)]
    struct Clock {
        joins: usize,
    }

    impl GameMode for Clock {
        fn descriptor() -> ModeDescriptor {
            ModeDescriptor {
                id: "clock",
                name: "Clock",
                map_labels: &[],
                bots: false,
                settings: vec![SettingSpec {
                    key: TIME_LIMIT_SETTING,
                    label: "Time limit (s)",
                    kind: SettingKind::Int {
                        min: 1,
                        max: 60,
                        default: 2,
                    },
                }],
            }
        }
        fn on_match_start(&mut self, _: &mut ModeCtx) {}
        fn on_player_join(&mut self, _: &mut ModeCtx, _: NetId) {
            self.joins += 1;
        }
        fn on_player_leave(&mut self, _: &mut ModeCtx, _: NetId) {}
        fn on_tick(&mut self, ctx: &mut ModeCtx) {
            let first = ctx.players()[0];
            ctx.add_score(first, 1);
        }
        fn on_event(&mut self, _: &mut ModeCtx, _: &GameEvent) {}
        fn result(&self) -> Option<MatchResult> {
            None
        }
        fn replicated_state(&self) -> Vec<u8> {
            vec![u8::try_from(self.joins).unwrap()]
        }
    }

    impl GameModeClient for Clock {
        fn apply_state(&mut self, _: &[u8]) {}
        fn on_event(&mut self, _: &GameEvent) {}
        fn draw_hud(&mut self, _: &mut HudCtx) {}
    }

    #[test]
    fn registry_rejects_duplicates() {
        let mut r = Registry::new();
        assert_eq!(r.register::<Clock>(), Ok(ModeId(0)));
        assert!(r.register::<Clock>().is_err());
        assert_eq!(r.find("clock").unwrap().id, ModeId(0));
    }

    #[test]
    fn time_limit_ends_on_the_leader_and_the_cycle_restarts() {
        let mut r = Registry::new();
        let id = r.register::<Clock>().unwrap();
        let mut config = MatchConfig {
            rotation: vec!["a".into(), "b".into()],
            ..MatchConfig::default()
        };
        config
            .settings
            .set(TIME_LIMIT_SETTING, SettingValue::Int(1));
        let mut m = Match::new(r.get(id).unwrap(), config).unwrap();
        assert_eq!(m.time_limit_ticks(), 60);
        let mut world = World::new(0);
        m.player_joined(&world, NetId(1));
        m.tick(&world);
        assert_eq!(m.phase(), Phase::Warmup);
        m.player_joined(&world, NetId(2));
        let mut changes = Vec::new();
        for _ in 0..2000 {
            m.tick(&world);
            for e in m.drain_events() {
                if let GameEvent::PhaseChanged { phase, .. } = e {
                    changes.push((phase, world.tick().0));
                }
            }
            world.step(&[]);
            if changes.len() == 6 {
                break;
            }
        }
        let phases: Vec<_> = changes.iter().map(|c| c.0).collect();
        assert_eq!(
            phases,
            [
                Phase::Countdown,
                Phase::Playing,
                Phase::RoundEnd,
                Phase::MapChange,
                Phase::Loading,
                Phase::Warmup
            ]
        );
        let ticks: Vec<_> = changes.windows(2).map(|w| w[1].1 - w[0].1).collect();
        assert_eq!(ticks, [180, 60, 480, 60, 60]);
        assert_eq!(m.current_map(), Some("b"));
        // The fresh instance saw both players join again.
        assert_eq!(m.state().mode_state, [2]);
        assert_eq!(m.result(), None);
    }

    #[test]
    fn countdown_falls_back_to_warmup() {
        let mut r = Registry::new();
        let id = r.register::<Clock>().unwrap();
        let mut m = Match::new(r.get(id).unwrap(), MatchConfig::default()).unwrap();
        let world = World::new(0);
        m.player_joined(&world, NetId(1));
        m.player_joined(&world, NetId(2));
        m.tick(&world);
        assert_eq!(m.phase(), Phase::Countdown);
        assert!(!m.movement_allowed());
        m.player_left(&world, NetId(2));
        m.tick(&world);
        assert_eq!(m.phase(), Phase::Warmup);
    }
}

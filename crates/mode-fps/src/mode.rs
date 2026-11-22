//! [`FpsMode`]: the FPS core as a `minigame-api` mode.
//!
//! It owns the damage chain, kill scoring (1 per kill) and respawns, with no
//! win condition of its own: the match ends on the time limit and the best
//! score. Rules of play (deathmatch, teams, pickups, bonuses) come with lot
//! 4.7 on top of [`FpsRules`].
//!
//! Gap (reported, not worked around): [`GameMode`] has no hook for messages
//! sent by clients and [`Match`](faset_minigame_api::Match) hides its mode
//! instance, so a host cannot hand a [`FireMsg`](crate::FireMsg) to the mode
//! it registered. Until the framework gets that hook, the host drives
//! [`FpsRules`] itself ([`FpsRules::fire`], [`FpsRules::step`]).

use std::collections::BTreeMap;

use faset_core::glam::Vec3;
use faset_core::NetId;
use faset_minigame_api::{
    GameEvent, GameMode, GameModeClient, HudCtx, MatchResult, ModeCtx, ModeDescriptor, SettingKind,
    SettingSpec, TeamId, TIME_LIMIT_SETTING,
};
use faset_sim::StaticWorld;

use crate::hud::{HudInput, HudModel, HudState};
use crate::message::ServerMsg;
use crate::rules::{FpsHost, FpsRules};

/// Descriptor id of the mode.
pub const MODE_ID: &str = "fps-core";

/// What a client knows about a player, from the replicated state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayerView {
    pub health: u16,
    pub armour: u16,
    pub alive: bool,
}

/// The FPS core mode, server and client side.
#[derive(Debug, Clone, Default)]
pub struct FpsMode {
    rules: FpsRules,
    /// Client mirror of the replicated state.
    view: BTreeMap<NetId, PlayerView>,
    hud: HudState,
}

impl FpsMode {
    /// The rules, read only.
    #[must_use]
    pub fn rules(&self) -> &FpsRules {
        &self.rules
    }

    /// The rules, to fire shots and read respawns.
    pub fn rules_mut(&mut self) -> &mut FpsRules {
        &mut self.rules
    }

    /// What the client knows about `player`.
    #[must_use]
    pub fn view(&self, player: NetId) -> Option<PlayerView> {
        self.view.get(&player).copied()
    }

    /// Sets what the host knows for the HUD: ammo, speed, dispersion, match
    /// timer and scores, scoreboard and class-choice state.
    pub fn set_hud_input(&mut self, input: HudInput) {
        self.hud.set_input(input);
    }

    /// Ages the HUD's feed, markers and damage numbers by `dt` seconds.
    pub fn advance_hud(&mut self, dt: f32) {
        self.hud.advance(dt);
    }

    /// The HUD data of this frame.
    #[must_use]
    pub fn hud_model(&self) -> HudModel {
        let own = self
            .hud
            .input()
            .player
            .and_then(|p| self.view.get(&p))
            .map(|v| (v.health, v.armour, v.alive));
        self.hud.model(own, Vec::new(), Vec::new())
    }

    /// Layout: player count (2), then per player id (2), health (2),
    /// armour (2), alive (1).
    fn encode(&self) -> Vec<u8> {
        let count = u16::try_from(self.rules.players().count()).unwrap_or(u16::MAX);
        let mut out = Vec::with_capacity(2 + 7 * usize::from(count));
        out.extend_from_slice(&count.to_le_bytes());
        for (id, slot) in self.rules.players().take(usize::from(count)) {
            let c = &slot.combatant;
            out.extend_from_slice(&id.0.to_le_bytes());
            out.extend_from_slice(&to_u16(c.health).to_le_bytes());
            out.extend_from_slice(&to_u16(c.armour).to_le_bytes());
            out.push(u8::from(c.alive()));
        }
        out
    }

    fn decode(bytes: &[u8]) -> Option<BTreeMap<NetId, PlayerView>> {
        let u16_at = |i: usize| Some(u16::from_le_bytes([*bytes.get(i)?, *bytes.get(i + 1)?]));
        let count = usize::from(u16_at(0)?);
        if bytes.len() != 2 + 7 * count {
            return None;
        }
        (0..count)
            .map(|i| {
                let at = 2 + 7 * i;
                Some((
                    NetId(u16_at(at)?),
                    PlayerView {
                        health: u16_at(at + 2)?,
                        armour: u16_at(at + 4)?,
                        alive: bytes[at + 6] != 0,
                    },
                ))
            })
            .collect()
    }
}

fn to_u16(v: f32) -> u16 {
    v.round().clamp(0.0, f32::from(u16::MAX)) as u16
}

/// [`FpsHost`] over a [`ModeCtx`].
struct CtxHost<'a, 'b>(&'a mut ModeCtx<'b>);

impl FpsHost for CtxHost<'_, '_> {
    fn team_of(&mut self, player: NetId) -> Option<TeamId> {
        self.0.teams().team_of(player)
    }
    fn position(&self, player: NetId) -> Option<Vec3> {
        self.0.position(player)
    }
    fn spawn_points(&mut self) -> Vec<Vec3> {
        self.0.spawns().points().to_vec()
    }
    fn statics(&self) -> &StaticWorld {
        self.0.world().statics()
    }
    fn seed(&self) -> u64 {
        self.0.world().seed()
    }
    fn add_score(&mut self, player: NetId, delta: i32) {
        self.0.add_score(player, delta);
    }
    fn feed(&mut self, text: String) {
        self.0.feed(text);
    }
    fn emit(&mut self, bytes: Vec<u8>) {
        self.0.emit(GameEvent::Mode(bytes));
    }
}

impl GameMode for FpsMode {
    fn descriptor() -> ModeDescriptor {
        ModeDescriptor {
            id: MODE_ID,
            name: "FPS core",
            map_labels: &["arena"],
            bots: false,
            settings: vec![SettingSpec {
                key: TIME_LIMIT_SETTING,
                label: "Time limit (s)",
                kind: SettingKind::Int {
                    min: 30,
                    max: 3600,
                    default: 600,
                },
            }],
        }
    }

    fn on_match_start(&mut self, ctx: &mut ModeCtx) {
        let now = ctx.tick().seconds();
        for &player in ctx.players() {
            if self.rules.slot(player).is_none() {
                self.rules.join(player, now);
            }
        }
        self.rules.reset(now);
        ctx.feed("FPS core: last one standing scores");
    }

    fn on_player_join(&mut self, ctx: &mut ModeCtx, player: NetId) {
        self.rules.join(player, ctx.tick().seconds());
    }

    fn on_player_leave(&mut self, _ctx: &mut ModeCtx, player: NetId) {
        self.rules.leave(player);
    }

    fn on_tick(&mut self, ctx: &mut ModeCtx) {
        let now = ctx.tick().seconds();
        self.rules.step(now, &mut CtxHost(ctx));
    }

    fn on_event(&mut self, _ctx: &mut ModeCtx, _event: &GameEvent) {}

    fn result(&self) -> Option<MatchResult> {
        None
    }

    fn replicated_state(&self) -> Vec<u8> {
        self.encode()
    }
}

impl GameModeClient for FpsMode {
    fn apply_state(&mut self, bytes: &[u8]) {
        if let Some(view) = Self::decode(bytes) {
            self.view = view;
        }
    }

    fn on_event(&mut self, event: &GameEvent) {
        let GameEvent::Mode(bytes) = event else {
            return;
        };
        if let Some(msg) = ServerMsg::decode(bytes) {
            self.hud.on_msg(&msg);
        }
    }

    fn draw_hud(&mut self, ui: &mut HudCtx) {
        for line in self.hud_model().text_lines() {
            ui.text(line);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use faset_core::Tick;
    use faset_minigame_api::{Match, MatchConfig, Phase, Registry, COUNTDOWN_TICKS};
    use faset_sim::World;

    #[test]
    fn registers_and_runs_inside_a_match_with_replicated_state() {
        let mut registry = Registry::new();
        registry.register::<FpsMode>().unwrap();
        let entry = registry.find(MODE_ID).unwrap().clone();
        let config = MatchConfig {
            spawn_points: vec![Vec3::new(0.0, 0.9, 0.0), Vec3::new(5.0, 0.9, 0.0)],
            ..MatchConfig::default()
        };
        let mut m = Match::new(&entry, config).unwrap();
        let mut world = World::with_statics(1, StaticWorld::test_arena());
        for id in [NetId(1), NetId(2)] {
            world.spawn_player(id, Vec3::new(0.0, 0.9, 0.0));
            m.player_joined(&world, id);
        }
        for _ in 0..COUNTDOWN_TICKS + 5 {
            world.step(&[]);
            m.tick(&world);
        }
        assert_eq!(m.phase(), Phase::Playing);
        assert!(world.tick() > Tick(0));

        let mut client = FpsMode::default();
        client.apply_state(&m.state().mode_state);
        assert_eq!(client.view(NetId(1)).unwrap().health, 100);
        assert!(client.view(NetId(2)).unwrap().alive);
        let mut hud = HudCtx::new();
        client.draw_hud(&mut hud);
        assert_eq!(hud.lines().len(), 2);

        GameModeClient::on_event(
            &mut client,
            &GameEvent::Mode(
                ServerMsg::Kill {
                    attacker: NetId(1),
                    victim: NetId(2),
                }
                .encode(),
            ),
        );
        let mut hud = HudCtx::new();
        client.draw_hud(&mut hud);
        assert_eq!(hud.lines()[0], "P1 killed P2");
        client.apply_state(&[1]);
        assert!(client.view(NetId(1)).is_some(), "bad state is ignored");
    }
}

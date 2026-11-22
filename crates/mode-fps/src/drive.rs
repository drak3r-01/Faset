//! The part of a tick both FPS modes share (lot 4.11): the commands the
//! players simulated, the kill volumes and the live projectiles, resolved
//! against the world and the server's hitbox history, before the damage
//! chain of [`FpsRules::step`].

use std::collections::BTreeMap;

use faset_core::glam::Vec3;
use faset_core::NetId;
use faset_mapfmt::ZoneKind;
use faset_minigame_api::{GameEvent, ModeCtx, TeamId};
use faset_server::ModeServices;
use faset_sim::{StaticWorld, World, ZoneEvent};

use crate::rules::{present_hitboxes, shooter_in, CommandEnv, FpsHost, FpsRules};

/// A host call recorded while the context is lent to the command
/// resolution, replayed into the mode's real host afterwards.
pub(crate) enum Call {
    Score(NetId, i32),
    Feed(String),
    Emit(Vec<u8>),
    Broadcast(GameEvent),
    Push(NetId, Vec3),
}

struct Recorder<'w> {
    world: &'w World,
    teams: BTreeMap<NetId, TeamId>,
    spawns: Vec<Vec3>,
    calls: Vec<Call>,
}

impl FpsHost for Recorder<'_> {
    fn team_of(&mut self, player: NetId) -> Option<TeamId> {
        self.teams.get(&player).copied()
    }
    fn position(&self, player: NetId) -> Option<Vec3> {
        self.world.position(player)
    }
    fn spawn_points(&mut self) -> Vec<Vec3> {
        self.spawns.clone()
    }
    fn statics(&self) -> &StaticWorld {
        self.world.statics()
    }
    fn seed(&self) -> u64 {
        self.world.seed()
    }
    fn add_score(&mut self, player: NetId, delta: i32) {
        self.calls.push(Call::Score(player, delta));
    }
    fn feed(&mut self, text: String) {
        self.calls.push(Call::Feed(text));
    }
    fn emit(&mut self, bytes: Vec<u8>) {
        self.calls.push(Call::Emit(bytes));
    }
    fn broadcast(&mut self, event: GameEvent) {
        self.calls.push(Call::Broadcast(event));
    }
    fn push(&mut self, player: NetId, delta_v: Vec3) {
        self.calls.push(Call::Push(player, delta_v));
    }
}

/// Replays recorded calls into `host`, in order.
pub(crate) fn replay(calls: Vec<Call>, host: &mut dyn FpsHost) {
    for call in calls {
        match call {
            Call::Score(p, d) => host.add_score(p, d),
            Call::Feed(t) => host.feed(t),
            Call::Emit(b) => host.emit(b),
            Call::Broadcast(e) => host.broadcast(e),
            Call::Push(p, v) => host.push(p, v),
        }
    }
}

/// Resolves this tick's kill volumes, commands and projectiles; returns
/// the host calls to replay before [`FpsRules::step`].
pub(crate) fn resolve(rules: &mut FpsRules, ctx: &mut ModeCtx) -> Vec<Call> {
    let world = ctx.world();
    let now = ctx.tick().seconds();
    let tick = ctx.tick().0;
    let alive = |rules: &FpsRules, id: NetId| rules.combatant(id).is_some_and(|c| c.alive());
    for (id, event) in ctx.zone_events() {
        if *event == ZoneEvent::Triggered(ZoneKind::KillVolume) && alive(rules, *id) {
            rules.kill_volume(*id, world.position(*id).unwrap_or(Vec3::ZERO));
        }
    }
    let living: Vec<NetId> = rules
        .players()
        .filter(|(_, s)| s.combatant.alive())
        .map(|(id, _)| id)
        .collect();
    let present = present_hitboxes(world, living);
    let players = ctx.players().to_vec();
    let teams = players
        .iter()
        .filter_map(|&p| Some((p, ctx.teams().team_of(p)?)))
        .collect();
    let spawns = ctx.spawns().points().to_vec();
    let mut recorder = Recorder {
        world,
        teams,
        spawns,
        calls: Vec::new(),
    };
    let commands = ctx.commands();
    let mut env = CommandEnv {
        now,
        tick,
        seed: world.seed(),
        statics: world.statics(),
        present: &present,
        services: ctx.host::<ModeServices>(),
    };
    for (id, cmd) in commands {
        if let Some(shooter) = shooter_in(world, *id) {
            rules.command(*id, cmd, shooter, &mut env, &mut recorder);
        }
    }
    rules.step_projectiles(&env, &mut recorder);
    recorder.calls
}

/// After [`FpsRules::step`]: respawns become spawn requests (the host moves
/// the player) and changed speed multipliers go to the host.
pub(crate) fn finish(rules: &mut FpsRules, ctx: &mut ModeCtx) {
    for (id, point) in rules.take_respawns() {
        ctx.spawns().request(id, point);
    }
    let now = ctx.tick().seconds();
    for (id, (run, jump)) in rules.changed_move_scales(now) {
        ctx.set_move_scale(id, run, jump);
    }
}

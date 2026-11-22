//! The two playable FPS modes of lot 4.7: free-for-all (`fps.ffa`) and team
//! deathmatch (`fps.team`), both [`FpsPlay`] over [`FpsRules`], with pickups,
//! timed bonuses, score limit and time limit.
//!
//! Register both with [`register_modes`] (or one by one with
//! `registry.register::<FfaMode>()`). Phases, the 300 s default timer and the
//! round cycle come from the framework's `Match`.
//!
//! Pickups sit at the objective zones of the map whose id is
//! `pickup.<kind>` (see [`crate::pickup::ZONE_PREFIX`]); the zone centre is
//! the trigger sphere centre.
//!
//! Gaps (reported, not worked around): `GameMode` has no hook for client
//! messages, so shots reach [`FpsRules`] through the host only; `sim` has no
//! per-player movement parameters, so the speed and jump bonuses are exposed
//! as multipliers ([`FpsRules::speed_multiplier`]) for the host to apply;
//! `SpawnPoints` cannot be asked for a chosen point, so respawn positions
//! stay in [`FpsRules::take_respawns`].

use std::collections::BTreeMap;
use std::marker::PhantomData;

use faset_core::glam::Vec3;
use faset_core::NetId;
use faset_minigame_api::{
    EndReason, GameEvent, GameMode, GameModeClient, HudCtx, MatchResult, ModeCtx, ModeDescriptor,
    Registry, RegistryError, SettingKind, SettingSpec, TeamId, TIME_LIMIT_SETTING,
};
use faset_sim::StaticWorld;

use crate::message::ServerMsg;
use crate::pickup::{Pickup, PickupEvent, PickupKind, Pickups, TIMED};
use crate::rules::{FpsHost, FpsRules};

/// Descriptor id of free-for-all.
pub const FFA_ID: &str = "fps.ffa";
/// Descriptor id of team deathmatch.
pub const TEAM_ID: &str = "fps.team";
/// Setting: score that wins the match at once, 0 for none.
pub const SCORE_LIMIT_SETTING: &str = "score_limit";
/// Setting: score of a kill.
pub const KILL_SCORE_SETTING: &str = "kill_score";
/// Setting: pickup respawn delays, percent of the table (higher = rarer).
pub const PICKUP_DELAY_SETTING: &str = "pickup_delay";
/// Match length, seconds (the plan's `rotation_seconds`, carried by the
/// framework's [`TIME_LIMIT_SETTING`] key).
pub const DEFAULT_ROTATION_SECONDS: i32 = 300;
/// Players a match is designed for.
pub const PLAYERS: usize = 16;
/// Players a match must still hold without breaking.
pub const MAX_PLAYERS: usize = 32;
/// Number of teams of team deathmatch.
pub const TEAM_COUNT: u8 = 2;

const FEED_LINES: usize = 5;
const NO_TEAM: u8 = u8::MAX;

/// What tells the two modes apart.
pub trait Variant: std::fmt::Debug + 'static {
    /// Descriptor id.
    const ID: &'static str;
    /// Name shown to players.
    const NAME: &'static str;
    /// Whether players are split in [`TEAM_COUNT`] teams.
    const TEAMS: bool;
    /// Default score limit.
    const SCORE_LIMIT: i32;
}

/// Free-for-all marker.
#[derive(Debug, Clone, Copy, Default)]
pub struct Ffa;
/// Team deathmatch marker.
#[derive(Debug, Clone, Copy, Default)]
pub struct Team;

impl Variant for Ffa {
    const ID: &'static str = FFA_ID;
    const NAME: &'static str = "Free for all";
    const TEAMS: bool = false;
    const SCORE_LIMIT: i32 = 25;
}

impl Variant for Team {
    const ID: &'static str = TEAM_ID;
    const NAME: &'static str = "Team deathmatch";
    const TEAMS: bool = true;
    const SCORE_LIMIT: i32 = 50;
}

/// Free-for-all: every player for themselves.
pub type FfaMode = FpsPlay<Ffa>;
/// Team deathmatch: two teams, friendly fire off.
pub type TeamMode = FpsPlay<Team>;

/// Registers `fps.ffa` then `fps.team`.
///
/// # Errors
/// A descriptor id already registered.
pub fn register_modes(registry: &mut Registry) -> Result<(), RegistryError> {
    registry.register::<FfaMode>()?;
    registry.register::<TeamMode>()?;
    Ok(())
}

/// What a client knows about a player, from the replicated state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayView {
    pub health: u16,
    pub armour: u16,
    pub alive: bool,
    pub team: Option<TeamId>,
    /// Tenths of a second left on each of [`TIMED`] (speed, damage, super
    /// jump, big head).
    pub bonus_tenths: [u16; 4],
}

/// A playable FPS mode, server and client side.
#[derive(Debug, Clone)]
pub struct FpsPlay<V: Variant> {
    rules: FpsRules,
    pickups: Pickups,
    /// Server record of team membership (replicated).
    members: BTreeMap<NetId, TeamId>,
    team_scores: BTreeMap<TeamId, i32>,
    /// Match start, seconds.
    start: f64,
    /// Match length, seconds.
    limit: f64,
    score_limit: i32,
    result: Option<MatchResult>,
    /// Client mirror of the replicated state.
    view: BTreeMap<NetId, PlayView>,
    kills: Vec<String>,
    _variant: PhantomData<fn() -> V>,
}

impl<V: Variant> Default for FpsPlay<V> {
    fn default() -> Self {
        Self {
            rules: FpsRules::default(),
            pickups: Pickups::default(),
            members: BTreeMap::new(),
            team_scores: BTreeMap::new(),
            start: 0.0,
            limit: f64::from(DEFAULT_ROTATION_SECONDS),
            score_limit: V::SCORE_LIMIT,
            result: None,
            view: BTreeMap::new(),
            kills: Vec::new(),
            _variant: PhantomData,
        }
    }
}

impl<V: Variant> FpsPlay<V> {
    /// The rules, read only.
    #[must_use]
    pub fn rules(&self) -> &FpsRules {
        &self.rules
    }

    /// The rules, to fire shots and read respawns.
    pub fn rules_mut(&mut self) -> &mut FpsRules {
        &mut self.rules
    }

    /// The pickups (server truth, or the client mirror).
    #[must_use]
    pub fn pickups(&self) -> &Pickups {
        &self.pickups
    }

    /// Whether the pickup `index` is shown as taken. A client only learns it
    /// from the server's confirmation.
    #[must_use]
    pub fn pickup_taken(&self, index: usize) -> bool {
        self.pickups
            .items()
            .get(index)
            .is_some_and(|p| !p.available())
    }

    /// Score of a team.
    #[must_use]
    pub fn team_score(&self, team: TeamId) -> i32 {
        self.team_scores.get(&team).copied().unwrap_or(0)
    }

    /// What the client knows about `player`.
    #[must_use]
    pub fn view(&self, player: NetId) -> Option<PlayView> {
        self.view.get(&player).copied()
    }

    /// Whether `viewer` has the big head bonus, so that enemy heads are drawn
    /// enlarged for them (cosmetic only: hit zones do not change).
    #[must_use]
    pub fn heads_enlarged_for(&self, viewer: NetId) -> bool {
        self.view
            .get(&viewer)
            .is_some_and(|v| v.bonus_tenths[3] > 0)
    }

    /// Layout: player count (2), per player id (2), health (2), armour (2),
    /// alive (1), team (1, 255 none), four bonus tenths (2 each); pickup
    /// count (2), per pickup kind (1), available (1), position (12); team
    /// count (1), per team score (4).
    fn encode(&self) -> Vec<u8> {
        let now = self.rules.now();
        let mut out = Vec::new();
        let count = u16::try_from(self.rules.players().count()).unwrap_or(u16::MAX);
        out.extend_from_slice(&count.to_le_bytes());
        for (id, slot) in self.rules.players().take(usize::from(count)) {
            let c = &slot.combatant;
            out.extend_from_slice(&id.0.to_le_bytes());
            out.extend_from_slice(&to_u16(c.health).to_le_bytes());
            out.extend_from_slice(&to_u16(c.armour).to_le_bytes());
            out.push(u8::from(c.alive()));
            out.push(self.team_byte(id));
            for kind in TIMED {
                // Rounded up, with a margin for the float error of `until - now`.
                let tenths =
                    to_u16((slot.bonuses.remaining(kind, now) * 10.0 - 1e-6).ceil() as f32);
                out.extend_from_slice(&tenths.to_le_bytes());
            }
        }
        let items = self.pickups.items();
        let n = u16::try_from(items.len()).unwrap_or(u16::MAX);
        out.extend_from_slice(&n.to_le_bytes());
        for p in items.iter().take(usize::from(n)) {
            out.push(p.kind.code());
            out.push(u8::from(p.available()));
            for c in p.position.to_array() {
                out.extend_from_slice(&c.to_le_bytes());
            }
        }
        out.push(u8::try_from(self.team_scores.len()).unwrap_or(u8::MAX));
        for (_, s) in self.team_scores.iter().take(usize::from(u8::MAX)) {
            out.extend_from_slice(&s.to_le_bytes());
        }
        out
    }

    fn team_byte(&self, player: NetId) -> u8 {
        self.members.get(&player).map_or(NO_TEAM, |t| t.0)
    }

    fn decode(bytes: &[u8]) -> Option<Decoded> {
        let mut r = Reader { bytes, at: 0 };
        let count = r.u16()?;
        let mut view = BTreeMap::new();
        for _ in 0..count {
            let id = NetId(r.u16()?);
            let health = r.u16()?;
            let armour = r.u16()?;
            let alive = r.u8()? != 0;
            let team = match r.u8()? {
                NO_TEAM => None,
                t => Some(TeamId(t)),
            };
            let mut bonus_tenths = [0; 4];
            for t in &mut bonus_tenths {
                *t = r.u16()?;
            }
            view.insert(
                id,
                PlayView {
                    health,
                    armour,
                    alive,
                    team,
                    bonus_tenths,
                },
            );
        }
        let n = r.u16()?;
        let mut pickups = Vec::new();
        for _ in 0..n {
            let kind = PickupKind::from_code(r.u8()?)?;
            let available = r.u8()? != 0;
            let position = Vec3::new(r.f32()?, r.f32()?, r.f32()?);
            pickups.push(Pickup {
                kind,
                position,
                respawn_at: if available { None } else { Some(f64::INFINITY) },
            });
        }
        let teams = r.u8()?;
        let mut team_scores = BTreeMap::new();
        for i in 0..teams {
            team_scores.insert(TeamId(i), r.i32()?);
        }
        (r.at == bytes.len()).then_some(Decoded {
            view,
            pickups,
            team_scores,
        })
    }

    fn setup_teams(&mut self, ctx: &mut ModeCtx, rebalance: bool) {
        if !V::TEAMS {
            return;
        }
        if rebalance || ctx.teams().count() == 0 {
            ctx.teams().set_count(TEAM_COUNT);
            self.members.clear();
            for &p in &ctx.players().to_vec() {
                self.assign(ctx, p);
            }
        }
    }

    fn assign(&mut self, ctx: &mut ModeCtx, player: NetId) {
        if let Some(team) = ctx.teams().auto_assign(player) {
            self.members.insert(player, team);
        }
    }

    fn finish(&mut self, ctx: &ModeCtx, reason: EndReason) {
        self.result = Some(if V::TEAMS {
            let best = self.team_scores.values().copied().max().unwrap_or(0);
            let leaders: Vec<TeamId> = self
                .team_scores
                .iter()
                .filter(|(_, &s)| s == best && best > 0)
                .map(|(&t, _)| t)
                .collect();
            if let [team] = leaders[..] {
                MatchResult {
                    winners: self
                        .members
                        .iter()
                        .filter(|(_, &t)| t == team)
                        .map(|(&p, _)| p)
                        .collect(),
                    winning_team: Some(team),
                    reason,
                }
            } else {
                MatchResult {
                    winners: Vec::new(),
                    winning_team: None,
                    reason,
                }
            }
        } else {
            MatchResult {
                winners: ctx.scores().leaders(),
                winning_team: None,
                reason,
            }
        });
    }

    /// Ends the match on the score limit or the time limit.
    fn check_end(&mut self, ctx: &ModeCtx, now: f64) {
        if self.result.is_some() {
            return;
        }
        let top = if V::TEAMS {
            self.team_scores.values().copied().max()
        } else {
            ctx.scores().ranking().first().map(|&(_, s)| s)
        };
        if self.score_limit > 0 && top.is_some_and(|s| s >= self.score_limit) {
            self.finish(ctx, EndReason::WinCondition);
        } else if now >= self.start + self.limit {
            self.finish(ctx, EndReason::TimeLimit);
        }
    }
}

struct Decoded {
    view: BTreeMap<NetId, PlayView>,
    pickups: Vec<Pickup>,
    team_scores: BTreeMap<TeamId, i32>,
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take<const N: usize>(&mut self) -> Option<[u8; N]> {
        let out = self.bytes.get(self.at..self.at + N)?.try_into().ok()?;
        self.at += N;
        Some(out)
    }
    fn u8(&mut self) -> Option<u8> {
        self.take::<1>().map(|b| b[0])
    }
    fn u16(&mut self) -> Option<u16> {
        self.take().map(u16::from_le_bytes)
    }
    fn i32(&mut self) -> Option<i32> {
        self.take().map(i32::from_le_bytes)
    }
    fn f32(&mut self) -> Option<f32> {
        self.take()
            .map(f32::from_le_bytes)
            .filter(|f| f.is_finite())
    }
}

fn to_u16(v: f32) -> u16 {
    v.round().clamp(0.0, f32::from(u16::MAX)) as u16
}

/// [`FpsHost`] over a [`ModeCtx`] that also tallies team scores.
struct PlayHost<'a, 'b> {
    ctx: &'a mut ModeCtx<'b>,
    members: &'a BTreeMap<NetId, TeamId>,
    team_scores: &'a mut BTreeMap<TeamId, i32>,
}

impl FpsHost for PlayHost<'_, '_> {
    fn team_of(&mut self, player: NetId) -> Option<TeamId> {
        self.ctx.teams().team_of(player)
    }
    fn position(&self, player: NetId) -> Option<Vec3> {
        self.ctx.position(player)
    }
    fn spawn_points(&mut self) -> Vec<Vec3> {
        self.ctx.spawns().points().to_vec()
    }
    fn statics(&self) -> &StaticWorld {
        self.ctx.world().statics()
    }
    fn seed(&self) -> u64 {
        self.ctx.world().seed()
    }
    fn add_score(&mut self, player: NetId, delta: i32) {
        self.ctx.add_score(player, delta);
        if let Some(team) = self.members.get(&player) {
            *self.team_scores.entry(*team).or_insert(0) += delta;
        }
    }
    fn feed(&mut self, text: String) {
        self.ctx.feed(text);
    }
    fn emit(&mut self, bytes: Vec<u8>) {
        self.ctx.emit(GameEvent::Mode(bytes));
    }
}

impl<V: Variant> GameMode for FpsPlay<V> {
    fn descriptor() -> ModeDescriptor {
        ModeDescriptor {
            id: V::ID,
            name: V::NAME,
            map_labels: &["fps"],
            bots: true,
            settings: vec![
                SettingSpec {
                    key: TIME_LIMIT_SETTING,
                    label: "Match length (s)",
                    kind: SettingKind::Int {
                        min: 30,
                        max: 3600,
                        default: DEFAULT_ROTATION_SECONDS,
                    },
                },
                SettingSpec {
                    key: SCORE_LIMIT_SETTING,
                    label: "Score limit (0 = none)",
                    kind: SettingKind::Int {
                        min: 0,
                        max: 1000,
                        default: V::SCORE_LIMIT,
                    },
                },
                SettingSpec {
                    key: KILL_SCORE_SETTING,
                    label: "Score per kill",
                    kind: SettingKind::Int {
                        min: 1,
                        max: 10,
                        default: 1,
                    },
                },
                SettingSpec {
                    key: PICKUP_DELAY_SETTING,
                    label: "Pickup respawn delay (%)",
                    kind: SettingKind::Int {
                        min: 25,
                        max: 400,
                        default: 100,
                    },
                },
            ],
        }
    }

    fn on_match_start(&mut self, ctx: &mut ModeCtx) {
        let now = ctx.tick().seconds();
        let int = |key: &str, default: i32| ctx.settings().int(key).unwrap_or(default);
        self.start = now;
        self.limit = f64::from(int(TIME_LIMIT_SETTING, DEFAULT_ROTATION_SECONDS));
        self.score_limit = int(SCORE_LIMIT_SETTING, V::SCORE_LIMIT);
        self.rules.kill_score = int(KILL_SCORE_SETTING, 1);
        self.rules.friendly_fire = false;
        let delay = int(PICKUP_DELAY_SETTING, 100);
        self.pickups = Pickups::new(
            ctx.objectives()
                .iter()
                .filter_map(|z| Some((PickupKind::from_zone_id(&z.id)?, (z.min + z.max) * 0.5))),
        );
        self.pickups
            .set_delay_percent(u32::try_from(delay).unwrap_or(100));
        self.result = None;
        for &player in &ctx.players().to_vec() {
            if self.rules.slot(player).is_none() {
                self.rules.join(player, now);
            }
        }
        self.rules.reset(now);
        self.team_scores = (0..if V::TEAMS { TEAM_COUNT } else { 0 })
            .map(|t| (TeamId(t), 0))
            .collect();
        self.setup_teams(ctx, true);
        ctx.feed(format!("{}: first to the score limit wins", V::NAME));
    }

    fn on_player_join(&mut self, ctx: &mut ModeCtx, player: NetId) {
        self.rules.join(player, ctx.tick().seconds());
        if V::TEAMS {
            self.setup_teams(ctx, false);
            if !self.members.contains_key(&player) {
                self.assign(ctx, player);
            }
        }
    }

    fn on_player_leave(&mut self, _ctx: &mut ModeCtx, player: NetId) {
        self.rules.leave(player);
        self.members.remove(&player);
    }

    fn on_tick(&mut self, ctx: &mut ModeCtx) {
        let now = ctx.tick().seconds();
        self.rules.step(
            now,
            &mut PlayHost {
                ctx: &mut *ctx,
                members: &self.members,
                team_scores: &mut self.team_scores,
            },
        );
        let present: Vec<(NetId, Vec3)> = self
            .rules
            .players()
            .filter(|(_, s)| s.combatant.alive())
            .filter_map(|(id, _)| Some((id, ctx.position(id)?)))
            .collect();
        let rules = &mut self.rules;
        let events = self
            .pickups
            .step(now, &present, |id, kind| rules.apply_pickup(id, kind, now));
        for event in events {
            let msg = match event {
                PickupEvent::Taken { index, player } => ServerMsg::PickupTaken { index, player },
                PickupEvent::Respawned { index } => ServerMsg::PickupRespawned { index },
            };
            ctx.emit(GameEvent::Mode(msg.encode()));
        }
        self.check_end(ctx, now);
    }

    fn on_event(&mut self, _ctx: &mut ModeCtx, _event: &GameEvent) {}

    fn result(&self) -> Option<MatchResult> {
        self.result.clone()
    }

    fn replicated_state(&self) -> Vec<u8> {
        self.encode()
    }
}

impl<V: Variant> GameModeClient for FpsPlay<V> {
    fn apply_state(&mut self, bytes: &[u8]) {
        if let Some(d) = Self::decode(bytes) {
            self.view = d.view;
            self.pickups.replace(d.pickups);
            self.team_scores = d.team_scores;
        }
    }

    fn on_event(&mut self, event: &GameEvent) {
        let GameEvent::Mode(bytes) = event else {
            return;
        };
        match ServerMsg::decode(bytes) {
            Some(ServerMsg::Kill { attacker, victim }) => {
                if self.kills.len() == FEED_LINES {
                    self.kills.remove(0);
                }
                self.kills
                    .push(format!("{} killed {}", attacker.0, victim.0));
            }
            Some(ServerMsg::PickupTaken { index, .. }) => {
                self.pickups.set_available(usize::from(index), false);
            }
            Some(ServerMsg::PickupRespawned { index }) => {
                self.pickups.set_available(usize::from(index), true);
            }
            _ => {}
        }
    }

    fn draw_hud(&mut self, ui: &mut HudCtx) {
        for line in &self.kills {
            ui.text(line.clone());
        }
        for (team, score) in &self.team_scores {
            ui.text(format!("Team {}: {score}", team.0));
        }
        for (id, v) in &self.view {
            let state = if v.alive { "alive" } else { "dead" };
            ui.text(format!(
                "Player {}: {} hp, {} armour, {state}",
                id.0, v.health, v.armour
            ));
        }
    }
}

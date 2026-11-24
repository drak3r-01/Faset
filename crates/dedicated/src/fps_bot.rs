//! [`FpsBot`]: a `faset-bots` brain as a virtual client of the server (lot
//! 4.11).
//!
//! Each tick the server hands the bot a [`BotContext`]; the bot copies what
//! a player knows into a [`RawState`] (its own controller state, the living
//! hostile players, the map's health pickups, its weapon numbers) and
//! returns the brain's [`InputCommand`], which goes through the same
//! command buffer as a human's. The fire intent is the protocol's
//! `BUTTON_FIRE` bit, read by the FPS mode for humans and bots alike. The
//! bot's class is sent once as a [`ClassMsg`] mode message.
//!
//! Decisions: health, alive flags and teams come from the hosted match's
//! vitals (what snapshots and the replicated state tell a client);
//! pickups are always reported available (the bot does not decode the
//! mode's state); shots are not heard (no hitscan event exists).

use std::sync::Arc;

use faset_bots::{
    Bot, Level, NavGraph, PickupInfo, PickupKind, RawEntity, RawState, SelfState, SightBlockers,
    WeaponParams,
};
use faset_core::glam::Vec3;
use faset_core::{NetId, Tick, DT};
use faset_minigame_api::MapPickup;
use faset_mode_fps::{ClassMsg, FpsRules, WeaponKind, GRAVITY};
use faset_protocol::quant::{dequantize_pitch_command, dequantize_yaw};
use faset_protocol::InputCommand;
use faset_server::BotContext;

/// Data shared by every bot of a map.
pub struct BotMap {
    pub nav: NavGraph,
    /// Health pickups (feet positions).
    pub health: Vec<Vec3>,
    /// The map's vegetation, which hides players from bots (lot 6.11).
    pub sight: Arc<SightBlockers>,
}

impl BotMap {
    /// The navigation graph, pickups and sight blockers of an expanded map
    /// file.
    #[must_use]
    pub fn build(file: &faset_mapfmt::MapFile, pickups: &[MapPickup]) -> Self {
        Self {
            nav: NavGraph::build(file, &faset_sim::MoveParams::default()),
            health: pickups
                .iter()
                .filter(|p| p.kind == "health")
                .map(|p| p.position)
                .collect(),
            sight: Arc::new(SightBlockers::from_map(file)),
        }
    }
}

/// The skill level of the `bot_level` setting (0 to 10).
#[must_use]
pub fn level_of(setting: u8) -> Level {
    match setting {
        0..=2 => Level::Easy,
        3..=5 => Level::Normal,
        6..=8 => Level::Hard,
        _ => Level::Expert,
    }
}

/// A brain driving one player.
pub struct FpsBot {
    bot: Bot,
    map: Arc<BotMap>,
    weapon: WeaponParams,
    class: Option<ClassMsg>,
    yaw: f32,
    pitch: f32,
    alive: bool,
}

impl FpsBot {
    /// A bot of `level` playing class `class` of the catalog behind
    /// `rules` (index, wrapped), seeded by `seed`.
    #[must_use]
    pub fn new(level: Level, seed: u64, map: Arc<BotMap>, rules: &FpsRules, class: usize) -> Self {
        let classes = rules.classes();
        let (class_msg, weapon) = if classes.is_empty() {
            (None, WeaponParams::hitscan(60.0))
        } else {
            let index = class % classes.len();
            let def = rules
                .weapon_index(&classes[index].main_weapon)
                .map(|i| &rules.weapons()[i]);
            let weapon = match def {
                Some(w) if w.kind == WeaponKind::Projectile => {
                    let p = w.projectile.as_ref();
                    WeaponParams {
                        slot: 0,
                        projectile_speed: p.map_or(30.0, |p| p.speed),
                        gravity: p.map_or(0.0, |p| p.gravity * -GRAVITY.y),
                        range: w.range.min(60.0),
                        lob: false,
                    }
                }
                Some(w) => WeaponParams::hitscan(w.range.min(80.0)),
                None => WeaponParams::hitscan(60.0),
            };
            let msg = ClassMsg {
                class: u8::try_from(index).unwrap_or(0),
                secondary: None,
            };
            (Some(msg), weapon)
        };
        let mut bot = Bot::new(level, seed);
        if !map.sight.is_empty() {
            bot.set_sight_blockers(Some(map.sight.clone()));
        }
        Self {
            bot,
            map,
            weapon,
            class: class_msg,
            yaw: 0.0,
            pitch: 0.0,
            alive: true,
        }
    }

    fn idle(&self, tick: Tick) -> InputCommand {
        InputCommand {
            tick,
            buttons: 0,
            move_x: 0,
            move_y: 0,
            yaw: faset_protocol::quant::quantize_yaw(self.yaw),
            pitch: 0,
            weapon_slot: self.weapon.slot,
        }
    }
}

impl faset_server::Bot for FpsBot {
    fn command_in(&mut self, ctx: &BotContext, me: NetId, tick: Tick) -> InputCommand {
        let world = ctx.world;
        let vitals = |id: NetId| ctx.game.and_then(|g| g.vitals(id));
        let mine = vitals(me);
        let alive = mine.is_none_or(|v| v.alive);
        if alive && !self.alive {
            // A new life: forget the old plans and memories.
            self.bot.reset();
        }
        self.alive = alive;
        let Some(state) = world.controller_state(me) else {
            return self.idle(tick);
        };
        if !alive || ctx.game.is_some_and(|g| !g.movement_allowed()) {
            return self.idle(tick);
        }
        let my_team = mine.and_then(|v| v.team);
        let enemies: Vec<RawEntity> = ctx
            .game
            .map(|g| g.players().to_vec())
            .unwrap_or_default()
            .into_iter()
            .filter(|&id| id != me)
            .filter_map(|id| {
                let v = vitals(id);
                if v.is_some_and(|v| !v.alive || (my_team.is_some() && v.team == my_team)) {
                    return None;
                }
                let s = world.controller_state(id)?;
                Some(RawEntity {
                    id,
                    pos: s.feet(),
                    vel: s.vel,
                })
            })
            .collect();
        let pickups: Vec<PickupInfo> = self
            .map
            .health
            .iter()
            .map(|&pos| PickupInfo {
                kind: PickupKind::Health,
                pos,
                available: true,
            })
            .collect();
        let raw = RawState {
            tick,
            time: tick.seconds(),
            me: SelfState {
                pos: state.feet(),
                vel: state.vel,
                eye: state.eye(),
                yaw: self.yaw,
                pitch: self.pitch,
                on_ground: state.on_ground,
                health: mine.map_or(100.0, |v| v.health),
                max_health: mine.map_or(100.0, |v| v.max_health),
            },
            enemies: &enemies,
            shots: &[],
            pickups: &pickups,
            objective: None,
            weapon: self.weapon,
        };
        let cmd = self.bot.tick(&raw, world.statics(), &self.map.nav, DT);
        self.yaw = dequantize_yaw(cmd.yaw);
        self.pitch = dequantize_pitch_command(cmd.pitch);
        cmd
    }

    fn messages(&mut self) -> Vec<Vec<u8>> {
        self.class.take().map(|m| m.encode()).into_iter().collect()
    }
}

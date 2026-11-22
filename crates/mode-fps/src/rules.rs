//! [`FpsRules`]: the per-player state of the FPS core and its rules (shots,
//! damage chain, kills, respawns), independent of the mode framework.
//!
//! The framework's `ModeCtx` cannot be built outside `minigame-api`, so the
//! rules reach the match through the small [`FpsHost`] trait: the mode
//! adapter implements it over `ModeCtx`, tests implement it over plain
//! data.

use std::collections::BTreeMap;

use faset_core::glam::Vec3;
use faset_core::{NetId, Tick, DT};
use faset_minigame_api::{GameEvent, TeamId};
use faset_protocol::{InputCommand, BUTTON_FIRE, BUTTON_RELOAD, BUTTON_SIGHT};
use faset_server::{Hitbox, ModeServices};
use faset_sim::components::STANDING_HEIGHT;
use faset_sim::rng::Pcg32;
use faset_sim::{StaticWorld, World};

use crate::class::{Catalog, ClassDef};
use crate::damage::{
    Combatant, DamageContext, DamageEvent, DamageOutcome, DamageType, HitZone, DEFAULT_HEALTH,
};
use crate::melee::{melee_attack, rewound_hitboxes};
use crate::message::{FireMsg, ServerMsg, FLAG_FRESH_PRESS, FLAG_SIGHTED};
use crate::pickup::{ActiveBonuses, PickupKind, ARMOUR_AMOUNT, HEAL_AMOUNT, MAX_ARMOUR};
use crate::projectile::{catch_up_time, launch, Detonation, ProjectileSystem};
use crate::respawn::choose_spawn;
use crate::shot::{
    fire, RayCaster, RewindCaster, SceneCaster, ShotInput, ShotReject, ShotReport, WeaponState,
};
use crate::spread::{AimBasis, Stance};
use crate::weapon::{WeaponDef, WeaponKind};

/// Lethal damage of a kill volume (lot 4.11, decision: any health dies).
pub const KILL_VOLUME_DAMAGE: f32 = 100_000.0;

/// What the rules need from the match around them.
pub trait FpsHost {
    /// Team of a player, if the mode has teams.
    fn team_of(&mut self, player: NetId) -> Option<TeamId>;
    /// Position (capsule centre) of a player.
    fn position(&self, player: NetId) -> Option<Vec3>;
    /// Every spawn point, as capsule centres.
    fn spawn_points(&mut self) -> Vec<Vec3>;
    /// The static map.
    fn statics(&self) -> &StaticWorld;
    /// Seed of the match (server seed of the spread and respawn draws).
    fn seed(&self) -> u64;
    /// Adds to a player's score.
    fn add_score(&mut self, player: NetId, delta: i32);
    /// Adds a line to the kill feed.
    fn feed(&mut self, text: String);
    /// Replicates mode bytes ([`ServerMsg`]) to every client.
    fn emit(&mut self, bytes: Vec<u8>);
    /// Replicates a typed framework event (projectile creation, explosion;
    /// lot 4.11). Ignored by default.
    fn broadcast(&mut self, _event: GameEvent) {}
    /// Adds `delta_v` to a player's velocity: knockback and the explosive
    /// jump (lot 4.11). Ignored by default.
    fn push(&mut self, _player: NetId, _delta_v: Vec3) {}
}

/// Where a shooter stands when its command is resolved (lot 4.11).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shooter {
    /// Eye position.
    pub eye: Vec3,
    /// Movement state (the sight flag comes from the command).
    pub stance: Stance,
}

/// What the rules need to resolve the commands of one tick (lot 4.11).
pub struct CommandEnv<'a> {
    /// Time of the tick, seconds.
    pub now: f64,
    /// The tick (projectile creation events carry it).
    pub tick: u32,
    /// Server seed of the match.
    pub seed: u64,
    pub statics: &'a StaticWorld,
    /// The living players as they stand now, ascending id.
    pub present: &'a [(NetId, Hitbox)],
    /// The server's hitbox history and round trips: hitscan and melee are
    /// resolved as the shooter saw the scene. Without it (headless tests)
    /// they use `present`.
    pub services: Option<&'a mut ModeServices>,
}

/// Mode messages a client sends to the FPS modes (lot 4.11).
///
/// Layout: tag 2 (1), class index (1, into the catalog's classes), secondary
/// (1, 255 for the class default, else an index into the class's
/// secondaries).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClassMsg {
    pub class: u8,
    pub secondary: Option<u8>,
}

impl ClassMsg {
    const TAG: u8 = 2;

    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        vec![Self::TAG, self.class, self.secondary.unwrap_or(u8::MAX)]
    }

    /// Inverse of [`encode`](Self::encode).
    #[must_use]
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        match *bytes {
            [Self::TAG, class, secondary] => Some(Self {
                class,
                secondary: (secondary != u8::MAX).then_some(secondary),
            }),
            _ => None,
        }
    }
}

/// Horizontal speed above which a shooter counts as moving for the spread,
/// m/s (decision).
pub const MOVING_SPEED: f32 = 1.0;

/// The hitboxes of the players in `world` among `ids`, as the server
/// records them (crouched and sliding players have the short body).
#[must_use]
pub fn present_hitboxes(
    world: &World,
    ids: impl IntoIterator<Item = NetId>,
) -> Vec<(NetId, Hitbox)> {
    let mut out: Vec<(NetId, Hitbox)> = ids
        .into_iter()
        .filter_map(|id| {
            let hb = match world.controller_state(id) {
                Some(s) => Hitbox {
                    position: s.feet() + Vec3::Y * (STANDING_HEIGHT * 0.5),
                    sliding: s.crouched,
                },
                None => Hitbox::standing(world.position(id)?),
            };
            Some((id, hb))
        })
        .collect();
    out.sort_by_key(|(id, _)| *id);
    out
}

/// Eye and stance of a player in `world`.
#[must_use]
pub fn shooter_in(world: &World, id: NetId) -> Option<Shooter> {
    match world.controller_state(id) {
        Some(s) => Some(Shooter {
            eye: s.eye(),
            stance: Stance {
                moving: s.vel.x.hypot(s.vel.z) > MOVING_SPEED,
                airborne: !s.on_ground,
                sighted: false,
            },
        }),
        None => Some(Shooter {
            eye: Hitbox::standing(world.position(id)?).eye(),
            stance: Stance::default(),
        }),
    }
}

/// One player of the FPS core.
#[derive(Debug, Clone, PartialEq)]
pub struct Slot {
    pub combatant: Combatant,
    pub weapon: WeaponState,
    /// Index into the weapon list.
    pub weapon_index: usize,
    /// Timed bonuses (lot 4.7); lost on death.
    pub bonuses: ActiveBonuses,
    /// Index of the class in the catalog, for catalog rules (lot 4.11).
    pub class: Option<usize>,
    /// Weapon indices the player carries: slot 0 the main weapon, slot 1
    /// the secondary (lot 4.11). `weapon_index` is one of them.
    pub loadout: Vec<usize>,
    /// Loadout slot in hand.
    pub active: usize,
    /// States of the holstered weapons, by loadout slot.
    pub stash: Vec<Option<WeaponState>>,
    /// Buttons of the previous command (fresh press detection).
    pub last_buttons: u16,
}

/// Circumstances of one shot, supplied by the host.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShotContext {
    /// Time of the shot, seconds (tick time).
    pub now: f64,
    /// Server seed of the match.
    pub seed: u64,
    /// Eye position of the shooter.
    pub origin: Vec3,
    /// Movement state of the shooter (the sight flag comes from the message).
    pub stance: Stance,
}

/// Why [`FpsRules::fire`] did nothing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FireError {
    /// Not a player of the match.
    UnknownPlayer,
    /// The shooter is dead.
    Dead,
    /// The weapon refused: cadence, ammo, reload, trigger.
    Rejected(ShotReject),
}

/// The FPS core rules: players, weapons, damage, respawns.
#[derive(Debug, Clone)]
pub struct FpsRules {
    weapons: Vec<WeaponDef>,
    slots: BTreeMap<NetId, Slot>,
    pending: Vec<DamageEvent>,
    respawned: Vec<(NetId, Vec3)>,
    /// Friendly fire on or off (default off, as in the plan).
    pub friendly_fire: bool,
    /// Score of a kill (lot 4.7 mode setting, default 1).
    pub kill_score: i32,
    /// Score of a suicide (default -1).
    pub suicide_score: i32,
    now: f64,
    /// Classes of the catalog (lot 4.11); empty for the plain weapon list.
    classes: Vec<ClassDef>,
    default_class: usize,
    /// Class and secondary weapon index chosen by each player, applied at
    /// its next spawn.
    choices: BTreeMap<NetId, (usize, Option<usize>)>,
    projectiles: ProjectileSystem,
    /// Movement multipliers last asked of the host.
    scales: BTreeMap<NetId, (f32, f32)>,
}

impl Default for FpsRules {
    fn default() -> Self {
        Self::new(WeaponDef::builtin())
    }
}

/// Class chosen when a player has not picked one (decision: the all-round
/// trooper).
pub const DEFAULT_CLASS: &str = "trooper";

impl FpsRules {
    /// Rules with `weapons` available; every player carries the first one
    /// (decision: loadouts are lot 4.6's). An empty list falls back to the
    /// built-in weapons.
    #[must_use]
    pub fn new(weapons: Vec<WeaponDef>) -> Self {
        let weapons = if weapons.is_empty() {
            WeaponDef::builtin()
        } else {
            weapons
        };
        Self {
            weapons,
            slots: BTreeMap::new(),
            pending: Vec::new(),
            respawned: Vec::new(),
            friendly_fire: false,
            kill_score: 1,
            suicide_score: -1,
            now: 0.0,
            classes: Vec::new(),
            default_class: 0,
            choices: BTreeMap::new(),
            projectiles: ProjectileSystem::new(),
            scales: BTreeMap::new(),
        }
    }

    /// Rules over a class catalog (lot 4.11): the weapon list is the
    /// catalog's (id order), each player carries its class's main weapon
    /// (slot 0) and a secondary (slot 1, the first allowed one unless it
    /// chose another), with the class's health and speed. Players start as
    /// [`DEFAULT_CLASS`].
    #[must_use]
    pub fn from_catalog(catalog: &Catalog) -> Self {
        let mut rules = Self::new(catalog.weapons().cloned().collect());
        rules.classes = catalog.classes().to_vec();
        rules.default_class = rules
            .classes
            .iter()
            .position(|c| c.id == DEFAULT_CLASS)
            .unwrap_or(0);
        rules
    }

    /// The classes of the catalog (empty without one).
    #[must_use]
    pub fn classes(&self) -> &[ClassDef] {
        &self.classes
    }

    /// Index of the weapon `id` in the weapon list.
    #[must_use]
    pub fn weapon_index(&self, id: &str) -> Option<usize> {
        self.weapons.iter().position(|w| w.id == id)
    }

    /// Records the class choice of a player (index into
    /// [`classes`](Self::classes), and optionally an index into that
    /// class's secondaries). It applies at the player's next spawn, or at
    /// once when `now_if_idle` is given (outside play). Returns whether the
    /// choice is valid.
    pub fn choose_class(
        &mut self,
        player: NetId,
        class: usize,
        secondary: Option<usize>,
        now_if_idle: Option<f64>,
    ) -> bool {
        let Some(def) = self.classes.get(class) else {
            return false;
        };
        let secondary = match secondary {
            None => None,
            Some(i) => match def.secondaries.get(i).and_then(|s| self.weapon_index(s)) {
                Some(w) => Some(w),
                None => return false,
            },
        };
        self.choices.insert(player, (class, secondary));
        if let (Some(now), true) = (now_if_idle, self.slots.contains_key(&player)) {
            let slot = self.fresh_slot(player, 0, now);
            self.slots.insert(player, slot);
        }
        true
    }

    /// Weapons, health and class a player spawns with.
    fn loadout_of(&self, player: NetId, weapon_index: usize) -> (Vec<usize>, f32, Option<usize>) {
        if self.classes.is_empty() {
            return (vec![weapon_index], DEFAULT_HEALTH, None);
        }
        let (c, secondary) = self
            .choices
            .get(&player)
            .copied()
            .unwrap_or((self.default_class, None));
        let class = &self.classes[c];
        let main = self.weapon_index(&class.main_weapon).unwrap_or(0);
        let mut loadout = vec![main];
        if let Some(s) =
            secondary.or_else(|| class.secondaries.first().and_then(|s| self.weapon_index(s)))
        {
            loadout.push(s);
        }
        (loadout, class.health, Some(c))
    }

    /// Run speed factor of a player's class (1 without classes).
    #[must_use]
    pub fn class_speed(&self, player: NetId) -> f32 {
        self.slots
            .get(&player)
            .and_then(|s| s.class)
            .and_then(|c| self.classes.get(c))
            .map_or(1.0, |c| c.speed_factor)
    }

    /// The weapon list.
    #[must_use]
    pub fn weapons(&self) -> &[WeaponDef] {
        &self.weapons
    }

    /// State of a player.
    #[must_use]
    pub fn slot(&self, player: NetId) -> Option<&Slot> {
        self.slots.get(&player)
    }

    /// State of a player, for tests that stage a situation.
    #[cfg(test)]
    pub(crate) fn slot_mut(&mut self, player: NetId) -> Option<&mut Slot> {
        self.slots.get_mut(&player)
    }

    /// Players in ascending id order.
    pub fn players(&self) -> impl Iterator<Item = (NetId, &Slot)> {
        self.slots.iter().map(|(&id, s)| (id, s))
    }

    /// A player enters, alive and protected, with a full first weapon.
    pub fn join(&mut self, player: NetId, now: f64) {
        let slot = self.fresh_slot(player, 0, now);
        self.slots.insert(player, slot);
    }

    /// A player leaves.
    pub fn leave(&mut self, player: NetId) {
        self.slots.remove(&player);
        self.choices.remove(&player);
        self.scales.remove(&player);
    }

    /// Everyone back to full health and ammo (a match starts).
    pub fn reset(&mut self, now: f64) {
        self.pending.clear();
        self.projectiles = ProjectileSystem::new();
        let ids: Vec<NetId> = self.slots.keys().copied().collect();
        for id in ids {
            let index = self.slots[&id].weapon_index;
            let slot = self.fresh_slot(id, index, now);
            self.slots.insert(id, slot);
        }
    }

    /// A new life for `player`: its class loadout, or `weapon_index` alone
    /// without classes.
    fn fresh_slot(&self, player: NetId, weapon_index: usize, now: f64) -> Slot {
        let (loadout, health, class) = self.loadout_of(player, weapon_index);
        let weapon_index = loadout[0];
        Slot {
            combatant: Combatant::spawned(now, health),
            weapon: WeaponState::new(&self.weapons[weapon_index], now),
            weapon_index,
            bonuses: ActiveBonuses::default(),
            class,
            stash: vec![None; loadout.len()],
            loadout,
            active: 0,
            last_buttons: 0,
        }
    }

    /// Starts a reload; `false` if the player cannot reload now.
    pub fn reload(&mut self, player: NetId, now: f64) -> bool {
        let Some(slot) = self.slots.get_mut(&player) else {
            return false;
        };
        slot.combatant.alive()
            && slot
                .weapon
                .start_reload(&self.weapons[slot.weapon_index], now)
    }

    /// Validates and resolves one shot of `shooter` at `now`: cadence, ammo
    /// and spread are recomputed here, from the server's own state. The
    /// damage it deals is queued for the next [`step`](Self::step).
    /// `shot` gives the time, the match seed, the shooter's eye (for
    /// client-side casters) and its stance; `caster` finds the hits.
    ///
    /// # Errors
    /// [`FireError`] when no shot happens.
    pub fn fire(
        &mut self,
        shooter: NetId,
        msg: &FireMsg,
        shot: &ShotContext,
        caster: &mut dyn RayCaster,
    ) -> Result<ShotReport, FireError> {
        let slot = self
            .slots
            .get_mut(&shooter)
            .ok_or(FireError::UnknownPlayer)?;
        if !slot.combatant.alive() {
            return Err(FireError::Dead);
        }
        let def = &self.weapons[slot.weapon_index];
        let input = ShotInput {
            now: shot.now,
            seed: shot.seed,
            shooter,
            origin: shot.origin,
            basis: AimBasis::from_quantized(msg.yaw, msg.pitch),
            stance: Stance {
                sighted: msg.sighted(),
                ..shot.stance
            },
            fresh_press: msg.fresh_press(),
        };
        let report = fire(def, &mut slot.weapon, &input, caster).map_err(FireError::Rejected)?;
        self.pending.extend(report.damage.iter().copied());
        Ok(report)
    }

    /// One simulation tick ending at `now`: advances every weapon, runs the
    /// damage chain on the queued events (scores, kill feed), and respawns
    /// the players whose time has come.
    pub fn step(&mut self, now: f64, host: &mut dyn FpsHost) {
        self.now = now;
        for slot in self.slots.values_mut() {
            slot.weapon.tick(&self.weapons[slot.weapon_index], now, DT);
        }
        for event in std::mem::take(&mut self.pending) {
            self.apply_damage(&event, now, host);
        }
        self.respawn_due(now, host);
    }

    fn apply_damage(&mut self, event: &DamageEvent, now: f64, host: &mut dyn FpsHost) {
        let same_team = event.attacker != event.victim
            && host
                .team_of(event.attacker)
                .zip(host.team_of(event.victim))
                .is_some_and(|(a, b)| a == b);
        // The damage bonus is the attacker's and feeds the chain's bonus factor.
        let bonus_multiplier = self
            .slots
            .get(&event.attacker)
            .map_or(1.0, |s| s.bonuses.damage_multiplier(now));
        let ctx = DamageContext {
            same_team,
            friendly_fire: self.friendly_fire,
            bonus_multiplier,
            ..DamageContext::default()
        };
        let Some(slot) = self.slots.get_mut(&event.victim) else {
            return;
        };
        let DamageOutcome::Applied {
            health_damage,
            killed,
            ..
        } = slot.combatant.apply(event, &ctx, now)
        else {
            return;
        };
        if killed {
            slot.bonuses.clear();
        }
        host.emit(
            ServerMsg::Hit {
                attacker: event.attacker,
                victim: event.victim,
                amount: health_damage,
                zone: event.zone,
            }
            .encode(),
        );
        if killed {
            // Decision: a kill scores `kill_score` (1), a suicide `suicide_score` (-1).
            if self.slots.contains_key(&event.attacker) {
                let delta = if event.attacker == event.victim {
                    self.suicide_score
                } else {
                    self.kill_score
                };
                host.add_score(event.attacker, delta);
            }
            host.feed(format!(
                "Player {} killed player {}",
                event.attacker.0, event.victim.0
            ));
            host.emit(
                ServerMsg::Kill {
                    attacker: event.attacker,
                    victim: event.victim,
                }
                .encode(),
            );
        }
    }

    fn respawn_due(&mut self, now: f64, host: &mut dyn FpsHost) {
        let due: Vec<NetId> = self
            .slots
            .iter()
            .filter(|(_, s)| s.combatant.respawn_due(now))
            .map(|(&id, _)| id)
            .collect();
        for id in due {
            let my_team = host.team_of(id);
            let mut enemies: Vec<Vec3> = Vec::new();
            for (&other, s) in &self.slots {
                if other == id || !s.combatant.alive() {
                    continue;
                }
                if my_team.is_some() && host.team_of(other) == my_team {
                    continue;
                }
                if let Some(p) = host.position(other) {
                    enemies.push(p);
                }
            }
            // Seeded by the match seed, the time in ticks and the player:
            // the same state always gives the same spawn.
            let mut rng = Pcg32::new(
                host.seed() ^ (now / f64::from(DT)).round() as u64,
                u64::from(id.0),
            );
            let Some(point) =
                choose_spawn(&host.spawn_points(), &enemies, host.statics(), &mut rng)
            else {
                tracing::warn!(player = id.0, "no spawn point: respawn postponed");
                continue;
            };
            // A new life: the class chosen meanwhile applies now; without
            // classes the weapon in hand is kept.
            let index = self.slots[&id].weapon_index;
            let fresh = self.fresh_slot(id, index, now);
            self.slots.insert(id, fresh);
            self.respawned.push((id, point));
            host.emit(
                ServerMsg::Respawn {
                    player: id,
                    position: point,
                }
                .encode(),
            );
        }
    }

    /// Queues damage that did not come from [`fire`](Self::fire) (blasts,
    /// melee, projectile contacts) for the next [`step`](Self::step), where
    /// it goes through the same chain.
    pub fn queue_damage(&mut self, events: impl IntoIterator<Item = DamageEvent>) {
        self.pending.extend(events);
    }

    /// Respawns the host must apply to the world (position of the new
    /// life), oldest first.
    pub fn take_respawns(&mut self) -> Vec<(NetId, Vec3)> {
        std::mem::take(&mut self.respawned)
    }

    /// Applies the effect of a pickup to a living player at `now`. Returns
    /// `false`, and changes nothing, when the pickup would be wasted: heal at
    /// full health, ammo with a full reserve, armour at [`MAX_ARMOUR`], or a
    /// dead or unknown player (decision: a useless instant pickup stays on
    /// the map). Timed bonuses always apply.
    pub fn apply_pickup(&mut self, player: NetId, kind: PickupKind, now: f64) -> bool {
        let Some(slot) = self.slots.get_mut(&player) else {
            return false;
        };
        if !slot.combatant.alive() {
            return false;
        }
        let c = &mut slot.combatant;
        match kind {
            PickupKind::Heal if c.health >= c.max_health => return false,
            PickupKind::Heal => c.health = (c.health + HEAL_AMOUNT).min(c.max_health),
            PickupKind::Ammo => {
                let full = self.weapons[slot.weapon_index].reserve;
                if slot.weapon.reserve >= full {
                    return false;
                }
                slot.weapon.reserve = full;
            }
            PickupKind::Armour if c.armour >= MAX_ARMOUR => return false,
            PickupKind::Armour => c.armour = (c.armour + ARMOUR_AMOUNT).min(MAX_ARMOUR),
            timed => slot.bonuses.activate(timed, now),
        }
        true
    }

    /// Run speed multiplier of a player at `now` (the host applies it to
    /// the controller; `sim` has no per-player movement parameters yet).
    #[must_use]
    pub fn speed_multiplier(&self, player: NetId, now: f64) -> f32 {
        self.slots
            .get(&player)
            .map_or(1.0, |s| s.bonuses.speed_multiplier(now))
    }

    /// Jump speed multiplier of a player at `now`.
    #[must_use]
    pub fn jump_multiplier(&self, player: NetId, now: f64) -> f32 {
        self.slots
            .get(&player)
            .map_or(1.0, |s| s.bonuses.jump_multiplier(now))
    }

    /// Time of the last [`step`](Self::step).
    #[must_use]
    pub fn now(&self) -> f64 {
        self.now
    }

    /// Projectiles in flight.
    #[must_use]
    pub fn projectiles(&self) -> &ProjectileSystem {
        &self.projectiles
    }

    /// Health and alive flag of a player.
    #[must_use]
    pub fn combatant(&self, player: NetId) -> Option<Combatant> {
        self.slots.get(&player).map(|s| s.combatant)
    }

    /// Resolves the command `player` simulated this tick (lot 4.11): weapon
    /// slot, reload and trigger, the weapon kind choosing the path
    /// (hitscan through [`fire`](Self::fire), projectiles through
    /// [`launch`] and the live [`ProjectileSystem`], melee through
    /// [`melee_attack`]). A fresh press is the fire bit set in this command
    /// and not in the previous one. An empty magazine starts a reload.
    /// Damage is queued for the next [`step`](Self::step); projectile
    /// creations and immediate detonations go to `host`.
    pub fn command(
        &mut self,
        player: NetId,
        cmd: &InputCommand,
        shooter: Shooter,
        env: &mut CommandEnv,
        host: &mut dyn FpsHost,
    ) {
        let now = env.now;
        let Some(slot) = self.slots.get_mut(&player) else {
            return;
        };
        let pressed = cmd.buttons & BUTTON_FIRE != 0;
        let fresh = pressed && slot.last_buttons & BUTTON_FIRE == 0;
        slot.last_buttons = cmd.buttons;
        if !slot.combatant.alive() {
            return;
        }
        // Weapon slot: 0 main, 1 secondary; anything else keeps the hand.
        let wanted = usize::from(cmd.weapon_slot);
        if wanted != slot.active && wanted < slot.loadout.len() {
            let stashed =
                std::mem::replace(&mut slot.weapon, WeaponState::new(&self.weapons[0], now));
            slot.stash[slot.active] = Some(stashed);
            let index = slot.loadout[wanted];
            let def = &self.weapons[index];
            slot.weapon = match slot.stash[wanted].take() {
                Some(mut w) => {
                    w.next_fire = w.next_fire.max(now + f64::from(def.draw_time));
                    w
                }
                None => WeaponState::new(def, now),
            };
            slot.active = wanted;
            slot.weapon_index = index;
        }
        if cmd.buttons & BUTTON_RELOAD != 0 {
            self.reload(player, now);
        }
        if !pressed {
            return;
        }
        let Some(slot) = self.slots.get_mut(&player) else {
            return;
        };
        let index = slot.weapon_index;
        let def = &self.weapons[index];
        let sighted = cmd.buttons & BUTTON_SIGHT != 0;
        let input = ShotInput {
            now,
            seed: env.seed,
            shooter: player,
            origin: shooter.eye,
            basis: AimBasis::from_quantized(cmd.yaw, cmd.pitch),
            stance: Stance {
                sighted,
                ..shooter.stance
            },
            fresh_press: fresh,
        };
        let view = env
            .services
            .as_deref()
            .map(|s| (s.view_instant(player, Tick(env.tick)), s.rtt(player)));
        let result = match def.kind {
            WeaponKind::Hitscan => {
                let msg = FireMsg {
                    view_tick: 0,
                    view_fraction: 0.0,
                    yaw: cmd.yaw,
                    pitch: cmd.pitch,
                    flags: (u8::from(fresh) * FLAG_FRESH_PRESS)
                        | (u8::from(sighted) * FLAG_SIGHTED),
                };
                let shot = ShotContext {
                    now,
                    seed: env.seed,
                    origin: shooter.eye,
                    stance: shooter.stance,
                };
                let outcome = match (env.services.as_deref_mut(), view) {
                    (Some(services), Some(((view_tick, view_fraction), rtt))) => {
                        let mut caster = RewindCaster {
                            lag: services.lag_mut(),
                            statics: env.statics,
                            shooter: player,
                            rtt,
                            view_tick,
                            view_fraction,
                        };
                        self.fire(player, &msg, &shot, &mut caster)
                    }
                    _ => {
                        let mut caster = SceneCaster {
                            statics: env.statics,
                            shooter: player,
                            targets: env.present,
                        };
                        self.fire(player, &msg, &shot, &mut caster)
                    }
                };
                outcome.map(|_| ()).map_err(|e| match e {
                    FireError::Rejected(r) => r,
                    _ => ShotReject::Reloading,
                })
            }
            WeaponKind::Projectile => {
                let slot = self.slots.get_mut(&player).expect("checked above");
                let weapon_byte = u8::try_from(index).unwrap_or(u8::MAX);
                launch(def, weapon_byte, &mut slot.weapon, &input, env.tick).map(|l| {
                    let catch_up = view.map_or(0.0, |(_, rtt)| catch_up_time(rtt));
                    for spawn in l.spawns {
                        host.broadcast(GameEvent::Projectile(faset_protocol::ProjectileSpawn {
                            shooter: spawn.shooter,
                            weapon: spawn.weapon,
                            tick: Tick(spawn.tick),
                            seed: spawn.seed,
                            origin: spawn.origin,
                            velocity: spawn.velocity,
                        }));
                        if let Some(d) = self.projectiles.spawn(
                            &self.weapons,
                            &spawn,
                            catch_up,
                            env.statics,
                            env.present,
                        ) {
                            self.detonate(&d, host);
                        }
                    }
                })
            }
            WeaponKind::Melee => {
                let targets = match (env.services.as_deref(), view) {
                    (Some(s), Some(((view_tick, view_fraction), rtt))) => rewound_hitboxes(
                        s.lag(),
                        player,
                        env.present.iter().map(|(id, _)| *id),
                        view_tick,
                        view_fraction,
                        rtt,
                    ),
                    _ => env.present.to_vec(),
                };
                let slot = self.slots.get_mut(&player).expect("checked above");
                melee_attack(def, &mut slot.weapon, &input, &targets, env.statics).map(|r| {
                    self.pending.extend(r.damage);
                })
            }
            WeaponKind::Beam => Ok(()),
        };
        if result == Err(ShotReject::Empty) {
            self.reload(player, now);
        }
    }

    /// One tick of the live projectiles against the living players as they
    /// stand now; detonations queue their damage and push the players
    /// they reach (lot 4.11).
    pub fn step_projectiles(&mut self, env: &CommandEnv, host: &mut dyn FpsHost) {
        for d in self
            .projectiles
            .step(&self.weapons, env.statics, env.present)
        {
            self.detonate(&d, host);
        }
    }

    fn detonate(&mut self, d: &Detonation, host: &mut dyn FpsHost) {
        self.pending.extend(d.damage.iter().copied());
        for &(player, dv) in &d.knockback {
            host.push(player, dv);
        }
        if d.exploded {
            host.broadcast(GameEvent::Explosion {
                owner: d.owner,
                weapon: u8::try_from(d.weapon).unwrap_or(u8::MAX),
                centre: d.centre,
            });
        }
    }

    /// A living player entered a kill volume: it dies (self-inflicted, so a
    /// suicide for the score).
    pub fn kill_volume(&mut self, player: NetId, position: Vec3) {
        self.pending.push(DamageEvent {
            attacker: player,
            victim: player,
            amount: KILL_VOLUME_DAMAGE,
            kind: DamageType::Explosion,
            zone: HitZone::Body,
            position,
        });
    }

    /// Run and jump multipliers of a player at `now`: its class speed and
    /// its timed bonuses.
    #[must_use]
    pub fn move_scale(&self, player: NetId, now: f64) -> (f32, f32) {
        (
            self.class_speed(player) * self.speed_multiplier(player, now),
            self.jump_multiplier(player, now),
        )
    }

    /// The multipliers that changed since the last call, for the host.
    pub fn changed_move_scales(&mut self, now: f64) -> Vec<(NetId, (f32, f32))> {
        let ids: Vec<NetId> = self.slots.keys().copied().collect();
        let mut out = Vec::new();
        for id in ids {
            let scale = self.move_scale(id, now);
            if self.scales.get(&id) != Some(&scale) {
                self.scales.insert(id, scale);
                out.push((id, scale));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::damage::RESPAWN_DELAY;
    use crate::message::FLAG_FRESH_PRESS;
    use crate::shot::SceneCaster;
    use faset_server::Hitbox;
    use faset_sim::Volume;

    struct Host {
        statics: StaticWorld,
        positions: BTreeMap<NetId, Vec3>,
        teams: BTreeMap<NetId, TeamId>,
        spawns: Vec<Vec3>,
        scores: BTreeMap<NetId, i32>,
        feed: Vec<String>,
        sent: Vec<ServerMsg>,
    }

    impl Host {
        fn new() -> Self {
            Self {
                // Floor plus a wall at z = 20 hiding the far spawn.
                statics: StaticWorld::new(vec![
                    Volume::cuboid(Vec3::new(0.0, -0.5, 0.0), Vec3::new(60.0, 0.5, 60.0)),
                    Volume::cuboid(Vec3::new(0.0, 5.0, 20.0), Vec3::new(40.0, 5.0, 0.25)),
                ]),
                positions: BTreeMap::new(),
                teams: BTreeMap::new(),
                spawns: vec![Vec3::new(0.0, 0.9, 8.0), Vec3::new(0.0, 0.9, 30.0)],
                scores: BTreeMap::new(),
                feed: Vec::new(),
                sent: Vec::new(),
            }
        }
    }

    impl FpsHost for Host {
        fn team_of(&mut self, p: NetId) -> Option<TeamId> {
            self.teams.get(&p).copied()
        }
        fn position(&self, p: NetId) -> Option<Vec3> {
            self.positions.get(&p).copied()
        }
        fn spawn_points(&mut self) -> Vec<Vec3> {
            self.spawns.clone()
        }
        fn statics(&self) -> &StaticWorld {
            &self.statics
        }
        fn seed(&self) -> u64 {
            99
        }
        fn add_score(&mut self, p: NetId, d: i32) {
            *self.scores.entry(p).or_insert(0) += d;
        }
        fn feed(&mut self, text: String) {
            self.feed.push(text);
        }
        fn emit(&mut self, bytes: Vec<u8>) {
            self.sent.push(ServerMsg::decode(&bytes).unwrap());
        }
    }

    const A: NetId = NetId(1);
    const B: NetId = NetId(2);

    fn msg() -> FireMsg {
        FireMsg {
            view_tick: 0,
            view_fraction: 0.0,
            yaw: 0,
            pitch: 0,
            flags: FLAG_FRESH_PRESS,
        }
    }

    /// Shooter A at the origin looking at -Z; B stands 5 m ahead.
    fn duel() -> (FpsRules, Host) {
        let mut rules = FpsRules::default();
        // A rifle with no spread so every shot connects.
        let mut weapons = WeaponDef::builtin();
        weapons[0].spread.sigma_base = 0.0;
        weapons[0].spread.delta_beta = 0.0;
        weapons[0].spread.sigma_max = 0.0;
        weapons[0].damage.base = 40.0;
        weapons[0].damage.head_multiplier = 1.0;
        rules.weapons = weapons;
        rules.join(A, 0.0);
        rules.join(B, 0.0);
        let mut host = Host::new();
        host.positions.insert(A, Vec3::new(0.0, 0.9, 0.0));
        host.positions.insert(B, Vec3::new(0.0, 0.9, -5.0));
        (rules, host)
    }

    fn shoot(rules: &mut FpsRules, host: &Host, now: f64) -> Result<ShotReport, FireError> {
        let targets: Vec<(NetId, Hitbox)> = host
            .positions
            .iter()
            .map(|(&id, &p)| (id, Hitbox::standing(p)))
            .collect();
        let mut caster = SceneCaster {
            statics: &host.statics,
            shooter: A,
            targets: &targets,
        };
        let eye = Hitbox::standing(host.positions[&A]).eye();
        let shot = ShotContext {
            now,
            seed: 1,
            origin: eye,
            stance: Stance::default(),
        };
        rules.fire(A, &msg(), &shot, &mut caster)
    }

    /// Steps once per tick from `from` to `to` seconds.
    fn run(rules: &mut FpsRules, host: &mut Host, from: f64, to: f64) {
        let mut t = from;
        while t <= to + 1e-9 {
            rules.step(t, host);
            t += f64::from(DT);
        }
    }

    #[test]
    fn spawn_protection_blocks_damage_for_one_and_a_half_seconds() {
        let (mut rules, mut host) = duel();
        // B joined at 0: protected until 1.5. A shoots at 1.0 (weapon drawn
        // at 0.5): the damage is queued, then ignored by the chain.
        run(&mut rules, &mut host, 0.0, 1.0);
        let r = shoot(&mut rules, &host, 1.0).unwrap();
        assert_eq!(r.damage.len(), 1);
        rules.step(1.0 + 1.0 / 60.0, &mut host);
        assert_eq!(rules.slot(B).unwrap().combatant.health, 100.0);
        assert!(host.sent.is_empty());
        // After the protection the same shot lands.
        run(&mut rules, &mut host, 1.0 + 1.0 / 60.0, 1.6);
        shoot(&mut rules, &host, 1.6).unwrap();
        rules.step(1.62, &mut host);
        assert_eq!(rules.slot(B).unwrap().combatant.health, 60.0);
    }

    #[test]
    fn kill_scores_feeds_and_respawns_three_seconds_later_out_of_sight() {
        let (mut rules, mut host) = duel();
        run(&mut rules, &mut host, 0.0, 2.0);
        // Three shots of 40 at 0.1 s apart kill B (100 health).
        let mut t = 2.0;
        for _ in 0..3 {
            shoot(&mut rules, &host, t).unwrap();
            rules.step(t, &mut host);
            t += 0.1;
        }
        rules.step(t, &mut host);
        let b = rules.slot(B).unwrap().combatant;
        assert!(!b.alive());
        assert_eq!(host.scores[&A], 1);
        assert_eq!(host.feed, vec!["Player 1 killed player 2".to_string()]);
        assert!(host.sent.iter().any(
            |m| matches!(m, ServerMsg::Kill { attacker, victim } if *attacker == A && *victim == B)
        ));
        // B died at 2.2 (the third shot): respawn at 5.2, not before.
        let died = b.respawn_at.unwrap() - RESPAWN_DELAY;
        assert!((2.2 - 1e-6..=2.4).contains(&died));
        run(&mut rules, &mut host, t, died + 2.9);
        assert!(!rules.slot(B).unwrap().combatant.alive());
        assert!(rules.take_respawns().is_empty());
        run(&mut rules, &mut host, died + 2.9, died + 3.1);
        let respawns = rules.take_respawns();
        assert_eq!(respawns.len(), 1);
        // Spawn (0, 30) is behind the wall, (0, 8) is in A's sight: the
        // hidden one is chosen.
        assert_eq!(respawns[0], (B, Vec3::new(0.0, 0.9, 30.0)));
        assert!(rules.slot(B).unwrap().combatant.alive());
        assert!(rules.slot(B).unwrap().combatant.protected(died + 3.2));
    }

    #[test]
    fn friendly_fire_is_off_in_teams_and_a_dead_shooter_cannot_fire() {
        let (mut rules, mut host) = duel();
        host.teams.insert(A, TeamId(0));
        host.teams.insert(B, TeamId(0));
        run(&mut rules, &mut host, 0.0, 2.0);
        shoot(&mut rules, &host, 2.0).unwrap();
        rules.step(2.02, &mut host);
        assert_eq!(rules.slot(B).unwrap().combatant.health, 100.0);
        rules.friendly_fire = true;
        shoot(&mut rules, &host, 2.1).unwrap();
        rules.step(2.12, &mut host);
        assert_eq!(rules.slot(B).unwrap().combatant.health, 60.0);

        // Kill A through a hit on A (no teams): then it cannot fire.
        let (mut rules, mut host) = duel();
        run(&mut rules, &mut host, 0.0, 2.0);
        rules.slots.get_mut(&A).unwrap().combatant.respawn_at = Some(9.0);
        assert_eq!(shoot(&mut rules, &host, 2.0).unwrap_err(), FireError::Dead);
        assert_eq!(
            rules
                .fire(
                    NetId(9),
                    &msg(),
                    &ShotContext {
                        now: 2.0,
                        seed: 1,
                        origin: Vec3::ZERO,
                        stance: Stance::default(),
                    },
                    &mut SceneCaster {
                        statics: &host.statics,
                        shooter: A,
                        targets: &[]
                    }
                )
                .unwrap_err(),
            FireError::UnknownPlayer
        );
    }

    #[test]
    fn server_rejects_a_shot_that_breaks_the_cadence() {
        let (mut rules, mut host) = duel();
        run(&mut rules, &mut host, 0.0, 2.0);
        shoot(&mut rules, &host, 2.0).unwrap();
        let ammo = rules.slot(A).unwrap().weapon.ammo;
        assert!(matches!(
            shoot(&mut rules, &host, 2.05),
            Err(FireError::Rejected(ShotReject::Cadence { .. }))
        ));
        assert_eq!(rules.slot(A).unwrap().weapon.ammo, ammo);
        assert!(shoot(&mut rules, &host, 2.1).is_ok());
    }
}

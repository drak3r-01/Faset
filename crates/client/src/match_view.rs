//! The client's copy of a match (lot 6.6): the mode's client side behind a
//! [`MatchMirror`], the events the HUD and the scene need (kills, shots,
//! blasts, projectiles) and the HUD model of each frame.
//!
//! It owns no window and no GPU object: the executable draws what it
//! reports. Everything here is cosmetic: the server stays authoritative for
//! health, ammunition and scores, and the client only mirrors them.

use std::collections::BTreeMap;

use faset_core::glam::Vec3;
use faset_core::{NetId, Tick};
use faset_minigame_api::{GameEvent, MatchMirror, ModeEntry, Phase, Registry};
use faset_mode_fps::hud::{HudInput, HudModel, ScoreRow};
use faset_mode_fps::{
    cone_sigma, decay_beta, fps_client, state_multiplier, ClientPickup, ClientPlayer, FpsClient,
    ServerMsg, Stance, MOVING_SPEED,
};
use faset_protocol::Event;
use faset_ui::display_name;

/// How long a blast stays on screen, seconds.
pub const BLAST_SECONDS: f64 = 0.6;
/// Longest a projectile is drawn without news of its end, seconds.
const PROJECTILE_MAX_SECONDS: f64 = 8.0;
/// Gravity of projectiles, m/s² (the sim's [`GRAVITY`](faset_mode_fps::GRAVITY)).
const GRAVITY: f32 = -faset_mode_fps::GRAVITY.y;

/// A projectile in flight, extrapolated from its creation event.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LiveProjectile {
    pub origin: Vec3,
    pub velocity: Vec3,
    /// Session time of its creation, seconds.
    pub born: f64,
    pub shooter: NetId,
    /// Index of the weapon in the match's weapon list.
    pub weapon: u8,
    /// Gravity multiplier of the weapon.
    pub gravity: f32,
    /// Seconds before it ends by itself.
    pub lifetime: f32,
}

impl LiveProjectile {
    /// Where it is at session time `now` (no collision: the server's
    /// explosion event removes it near its end).
    #[must_use]
    pub fn position(&self, now: f64) -> Vec3 {
        let t = (now - self.born).max(0.0) as f32;
        self.origin + self.velocity * t + Vec3::NEG_Y * (0.5 * GRAVITY * self.gravity * t * t)
    }
}

/// A blast that just went off.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Blast {
    pub centre: Vec3,
    /// Session time it went off, seconds.
    pub born: f64,
}

/// What the local player does this frame, for the HUD.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LocalFrame {
    /// Horizontal speed, m/s.
    pub speed: f32,
    pub airborne: bool,
    pub sighted: bool,
    /// The scoreboard key is held.
    pub scoreboard: bool,
}

/// The client's view of a match.
pub struct MatchView {
    registry: Registry,
    mirror: Option<MatchMirror>,
    local: Option<NetId>,
    /// `(kills, deaths)` per player, from the kill feed.
    tallies: BTreeMap<NetId, (u32, u32)>,
    projectiles: Vec<LiveProjectile>,
    blasts: Vec<Blast>,
    /// Ammunition of the weapon in hand at the last frame, to see shots.
    last_ammo: Option<(usize, u16)>,
    /// Bloom added by the local player's shots.
    beta: f32,
    /// Shots the local player fired since the last [`take_shots`](Self::take_shots).
    shots: u32,
    /// Latest server tick seen.
    server_tick: Tick,
}

impl MatchView {
    /// A view with no match yet, over the modes of `registry`.
    #[must_use]
    pub fn new(registry: Registry) -> Self {
        Self {
            registry,
            mirror: None,
            local: None,
            tallies: BTreeMap::new(),
            projectiles: Vec::new(),
            blasts: Vec::new(),
            last_ammo: None,
            beta: 0.0,
            shots: 0,
            server_tick: Tick(0),
        }
    }

    /// The server said hello: mirrors mode `mode` for player `local`.
    pub fn start(&mut self, mode: u8, local: NetId) {
        self.local = Some(local);
        let entry: Option<ModeEntry> = self.registry.get(faset_minigame_api::ModeId(mode)).cloned();
        self.mirror = entry.as_ref().map(MatchMirror::new);
        if self.mirror.is_none() {
            tracing::warn!(mode, "the server runs a mode this build does not know");
        }
        self.tallies.clear();
        self.projectiles.clear();
        self.blasts.clear();
    }

    /// Whether a mode is mirrored.
    #[must_use]
    pub fn active(&self) -> bool {
        self.mirror.is_some()
    }

    /// The mirror of the match, once the server said hello.
    #[must_use]
    pub fn mirror(&self) -> Option<&MatchMirror> {
        self.mirror.as_ref()
    }

    /// Notes the server tick of the latest snapshot.
    pub fn on_server_tick(&mut self, tick: Tick) {
        self.server_tick = tick;
    }

    /// Applies a replicated match state.
    pub fn on_state(&mut self, bytes: &[u8]) {
        if let Some(m) = self.mirror.as_mut() {
            if let Err(e) = m.apply_state_bytes(bytes) {
                tracing::debug!(error = %e, "bad match state ignored");
            }
        }
    }

    /// Applies a protocol event at session time `now`. Returns whether it
    /// was a match event.
    pub fn on_event(&mut self, event: &Event, now: f64) -> bool {
        if let Event::MatchState(bytes) = event {
            self.on_state(bytes);
            return true;
        }
        let Some(game) = GameEvent::from_protocol(event) else {
            return false;
        };
        match &game {
            GameEvent::Projectile(spawn) => {
                let (gravity, lifetime) = self.projectile_def(spawn.weapon);
                self.projectiles.push(LiveProjectile {
                    origin: spawn.origin,
                    velocity: spawn.velocity,
                    born: now,
                    shooter: spawn.shooter,
                    weapon: spawn.weapon,
                    gravity,
                    lifetime,
                });
            }
            GameEvent::Explosion { centre, .. } => {
                // The projectile that ended is the one nearest the blast.
                if let Some(i) = self
                    .projectiles
                    .iter()
                    .enumerate()
                    .min_by(|a, b| {
                        let da = a.1.position(now).distance_squared(*centre);
                        let db = b.1.position(now).distance_squared(*centre);
                        da.total_cmp(&db)
                    })
                    .map(|(i, _)| i)
                {
                    self.projectiles.swap_remove(i);
                }
                self.blasts.push(Blast {
                    centre: *centre,
                    born: now,
                });
            }
            GameEvent::Mode(bytes) => {
                if let Some(ServerMsg::Kill { attacker, victim }) = ServerMsg::decode(bytes) {
                    if attacker != victim {
                        self.tallies.entry(attacker).or_default().0 += 1;
                    }
                    self.tallies.entry(victim).or_default().1 += 1;
                }
            }
            GameEvent::PhaseChanged {
                phase: Phase::Playing,
                ..
            } => self.tallies.clear(),
            _ => {}
        }
        if let Some(m) = self.mirror.as_mut() {
            m.on_event(game);
        }
        true
    }

    fn projectile_def(&mut self, weapon: u8) -> (f32, f32) {
        let def = self
            .mirror
            .as_mut()
            .and_then(|m| fps_client(m.client_mut()))
            .and_then(|c| {
                c.weapons()
                    .get(usize::from(weapon))
                    .and_then(|w| w.projectile.as_ref())
                    .map(|p| (p.gravity, p.lifetime))
            });
        def.unwrap_or((1.0, 5.0))
    }

    /// The projectiles still in flight at session time `now`.
    pub fn projectiles(&mut self, now: f64) -> &[LiveProjectile] {
        self.projectiles.retain(|p| {
            let age = now - p.born;
            age < f64::from(p.lifetime).min(PROJECTILE_MAX_SECONDS)
        });
        &self.projectiles
    }

    /// The blasts still shown at session time `now`.
    pub fn blasts(&mut self, now: f64) -> &[Blast] {
        self.blasts.retain(|b| now - b.born < BLAST_SECONDS);
        &self.blasts
    }

    /// Shots the local player fired since the last call (seen as a drop of
    /// the magazine), for the weapon's recoil animation.
    pub fn take_shots(&mut self) -> u32 {
        std::mem::take(&mut self.shots)
    }

    fn client(&mut self) -> Option<&mut dyn FpsClient> {
        self.mirror
            .as_mut()
            .and_then(|m| fps_client(m.client_mut()))
    }

    /// Every player of the replicated state.
    pub fn players(&mut self) -> Vec<(NetId, ClientPlayer)> {
        self.client().map(|c| c.players()).unwrap_or_default()
    }

    /// The local player, as the replicated state shows them.
    pub fn local_player(&mut self) -> Option<ClientPlayer> {
        let me = self.local?;
        self.players()
            .into_iter()
            .find_map(|(id, p)| (id == me).then_some(p))
    }

    /// The pickups of the map.
    pub fn pickups(&mut self) -> Vec<ClientPickup> {
        self.client().map(|c| c.pickups()).unwrap_or_default()
    }

    /// Id of the weapon `index` of the match's list.
    pub fn weapon_id(&mut self, index: usize) -> Option<String> {
        self.client()
            .and_then(|c| c.weapons().get(index).map(|w| w.id.clone()))
    }

    /// Id of the class `index` of the match's catalog.
    pub fn class_id(&mut self, index: usize) -> Option<String> {
        self.client()
            .and_then(|c| c.classes().get(index).map(|c| c.id.clone()))
    }

    /// Whether the phase lets the local player move and shoot; unknown
    /// counts as yes.
    #[must_use]
    pub fn movement_allowed(&self) -> bool {
        self.mirror
            .as_ref()
            .is_none_or(MatchMirror::movement_allowed)
    }

    /// Whether the local player is alive (unknown counts as alive).
    #[must_use]
    pub fn local_alive(&self) -> bool {
        match (&self.mirror, self.local) {
            (Some(m), Some(me)) => m.vitals(me).is_none_or(|v| v.alive),
            _ => true,
        }
    }

    /// Speed multipliers of the local player as the server's state shows
    /// them.
    #[must_use]
    pub fn move_scale(&self) -> (f32, f32) {
        match (&self.mirror, self.local) {
            (Some(m), Some(me)) => m.client().move_scale(me),
            _ => (1.0, 1.0),
        }
    }

    /// Current phase, once known.
    #[must_use]
    pub fn phase(&self) -> Option<Phase> {
        self.mirror.as_ref().and_then(MatchMirror::phase)
    }

    /// The HUD model of this frame, `None` while no FPS mode is mirrored.
    /// `dt` ages the feed and the markers.
    pub fn hud_model(&mut self, frame: &LocalFrame, dt: f32) -> Option<HudModel> {
        let local = self.local?;
        let tick = self.server_tick;
        let ctx = self.mirror.as_mut()?.draw_hud(Some(local), tick);
        let me = self.local_player();
        let weapon = {
            let index = me.and_then(|p| p.weapon);
            let def = index.and_then(|i| self.client().and_then(|c| c.weapons().get(i).cloned()));
            def
        };
        // Shots: the magazine of the weapon in hand went down.
        if let (Some(p), Some(i)) = (me, me.and_then(|p| p.weapon)) {
            if let Some((was_index, was)) = self.last_ammo {
                if was_index == i && p.ammo < was {
                    self.shots += 1;
                    if let Some(def) = &weapon {
                        self.beta += def.spread.delta_beta;
                    }
                }
            }
            self.last_ammo = Some((i, p.ammo));
        }
        if let Some(def) = &weapon {
            self.beta = decay_beta(self.beta, dt, def.spread.tau);
        }
        let sigma = weapon.as_ref().map_or(0.0, |def| {
            let stance = Stance {
                moving: frame.speed > MOVING_SPEED,
                airborne: frame.airborne,
                sighted: frame.sighted,
            };
            cone_sigma(
                &def.spread,
                state_multiplier(&def.spread, &def.sight, stance),
                self.beta,
            )
        });
        let players = self.players();
        let mut rows: Vec<ScoreRow> = players
            .iter()
            .map(|(id, _)| {
                let (kills, deaths) = self.tallies.get(id).copied().unwrap_or_default();
                let score = self.mirror.as_ref().map_or(0, |m| m.score(*id));
                ScoreRow {
                    name: if *id == local {
                        "You".to_owned()
                    } else {
                        format!("P{}", id.0)
                    },
                    score,
                    kills,
                    deaths,
                    local: *id == local,
                }
            })
            .collect();
        rows.sort_by_key(|r| std::cmp::Reverse(r.score));
        let round_end = ctx.phase == Some(Phase::RoundEnd);
        let input = HudInput {
            player: Some(local),
            names: BTreeMap::from([(local, "You".to_owned())]),
            weapon: weapon.map(|w| display_name(&w.id)).unwrap_or_default(),
            ammo: me.map(|p| (p.ammo, p.reserve)),
            speed: frame.speed,
            sigma,
            time_left: ctx.time_left.filter(|_| ctx.phase == Some(Phase::Playing)),
            scores: rows,
            show_scoreboard: frame.scoreboard,
            round_end,
            class_choice: None,
        };
        let client = self.client()?;
        client.set_hud_input(input);
        client.advance_hud(dt);
        Some(client.hud_model())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use faset_mode_fps::register_modes;
    use faset_protocol::ProjectileSpawn;

    fn view() -> MatchView {
        let mut registry = Registry::new();
        register_modes(&mut registry).unwrap();
        let mut v = MatchView::new(registry);
        v.start(0, NetId(1));
        v
    }

    fn spawn(at: Vec3, velocity: Vec3) -> Event {
        GameEvent::Projectile(ProjectileSpawn {
            shooter: NetId(2),
            weapon: 0,
            tick: Tick(1),
            seed: 0,
            origin: at,
            velocity,
        })
        .to_protocol()
    }

    #[test]
    fn a_projectile_flies_on_its_ballistic_path_and_ends_with_its_lifetime() {
        let mut v = view();
        v.on_event(&spawn(Vec3::ZERO, Vec3::new(0.0, 0.0, -20.0)), 10.0);
        let p = v.projectiles(10.5)[0];
        assert!((p.position(10.5).z + 10.0).abs() < 1e-4);
        assert!(p.position(10.5).y <= 0.0, "gravity pulls it down");
        assert!(v.projectiles(10.0 + 60.0).is_empty());
    }

    #[test]
    fn an_explosion_removes_the_nearest_projectile_and_shows_a_blast() {
        let mut v = view();
        v.on_event(&spawn(Vec3::ZERO, Vec3::ZERO), 1.0);
        v.on_event(&spawn(Vec3::new(50.0, 0.0, 0.0), Vec3::ZERO), 1.0);
        let blast = GameEvent::Explosion {
            owner: NetId(2),
            weapon: 0,
            centre: Vec3::new(49.0, 0.0, 0.0),
        }
        .to_protocol();
        assert!(v.on_event(&blast, 1.1));
        assert_eq!(v.projectiles(1.1).len(), 1);
        assert_eq!(v.projectiles(1.1)[0].origin, Vec3::ZERO);
        assert_eq!(v.blasts(1.2).len(), 1);
        assert!(v.blasts(1.1 + BLAST_SECONDS + 0.1).is_empty());
    }

    #[test]
    fn events_that_are_not_the_matchs_are_left_alone() {
        let mut v = view();
        assert!(!v.on_event(&Event::MapChange { map_fingerprint: 1 }, 0.0));
    }

    #[test]
    fn kills_and_deaths_are_tallied_and_a_new_round_resets_them() {
        let mut v = view();
        let kill = |attacker, victim| {
            GameEvent::Mode(
                ServerMsg::Kill {
                    attacker: NetId(attacker),
                    victim: NetId(victim),
                }
                .encode(),
            )
            .to_protocol()
        };
        v.on_event(&kill(2, 3), 0.0);
        v.on_event(&kill(2, 3), 0.0);
        v.on_event(&kill(3, 3), 0.0);
        assert_eq!(v.tallies[&NetId(2)], (2, 0));
        assert_eq!(v.tallies[&NetId(3)], (0, 3), "a suicide is no kill");
        let playing = GameEvent::PhaseChanged {
            phase: Phase::Playing,
            end: None,
        }
        .to_protocol();
        v.on_event(&playing, 0.0);
        assert!(v.tallies.is_empty());
    }

    #[test]
    fn without_a_mode_nothing_is_reported() {
        let mut v = MatchView::new(Registry::new());
        v.start(9, NetId(1));
        assert!(!v.active());
        assert!(v.hud_model(&LocalFrame::default(), 0.016).is_none());
        assert!(v.players().is_empty());
        assert!(v.movement_allowed() && v.local_alive());
        assert_eq!(v.move_scale(), (1.0, 1.0));
    }
}

//! Utility AI: every tick each action gets a score, the best one wins and the
//! current one keeps a 15 % bonus against hesitation.

use serde::{Deserialize, Serialize};

use crate::view::{BotView, PickupKind, Sighting};

/// The actions a bot chooses from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Action {
    /// Shoot at a visible enemy in range.
    Attack,
    /// Go to where an enemy was seen less than 5 s ago.
    Pursue,
    /// Go to a health pickup.
    Heal,
    /// Go to another useful pickup.
    Pickup,
    /// Go to the objective the mode designates.
    Objective,
    /// Wander the map.
    Patrol,
}

impl Action {
    /// Every action, in score-table order.
    pub const ALL: [Action; 6] = [
        Action::Attack,
        Action::Pursue,
        Action::Heal,
        Action::Pickup,
        Action::Objective,
        Action::Patrol,
    ];
}

/// Tuning constants of the scores (decision: the plan gives only the form of
/// the heal score; these values are ours and live in this config).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UtilityConfig {
    /// Score of attacking a visible enemy in range.
    pub attack: f32,
    /// Score of pursuing an enemy seen just now; halves over `memory_s`, then ends.
    pub pursue: f32,
    /// How long an enemy is pursued after the last sighting, seconds.
    pub memory_s: f32,
    /// `w` of `U_heal = (1 - hp / hp_max) w / (1 + d / k)`.
    pub heal_weight: f32,
    /// `k` of the heal score, metres.
    pub heal_k: f32,
    /// Weight of a weapon or bonus pickup.
    pub pickup_weight: f32,
    /// Distance scale of the pickup score, metres.
    pub pickup_k: f32,
    /// Score of the mode objective.
    pub objective: f32,
    /// Score of patrolling, the fallback.
    pub patrol: f32,
    /// Bonus of the current action (0.15 = 15 %).
    pub current_bonus: f32,
}

impl Default for UtilityConfig {
    fn default() -> Self {
        Self {
            attack: 1.0,
            pursue: 0.7,
            memory_s: 5.0,
            heal_weight: 1.8,
            heal_k: 12.0,
            pickup_weight: 0.55,
            pickup_k: 20.0,
            objective: 0.5,
            patrol: 0.2,
            current_bonus: 0.15,
        }
    }
}

/// The scores of a tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scores(pub [f32; 6]);

impl Scores {
    /// Score of one action.
    #[must_use]
    pub fn of(&self, a: Action) -> f32 {
        self.0[a as usize]
    }
}

/// What the scores were computed from; the brain reuses it to act.
#[derive(Debug, Clone, Copy, Default)]
pub struct Facts {
    /// Nearest enemy seen in the latest delivered observation and within weapon
    /// range.
    pub target: Option<Sighting>,
    /// Most recent sighting (seen or heard) younger than the memory.
    pub recent: Option<Sighting>,
    /// Position of the health pickup the heal score used.
    pub heal_at: Option<faset_core::glam::Vec3>,
    /// Position of the other pickup the pickup score used.
    pub pickup_at: Option<faset_core::glam::Vec3>,
}

/// Scores every action for `view`.
#[must_use]
pub fn score(view: &BotView, cfg: &UtilityConfig) -> (Scores, Facts) {
    let me = &view.me;
    let mut facts = Facts::default();
    let mut s = [0.0_f32; 6];

    // Attack: an enemy is visible and within weapon range.
    let mut best = f32::INFINITY;
    for k in view.sightings.iter().filter(|k| k.visible) {
        let d = k.pos.distance(me.pos);
        if d <= view.weapon.range && d < best {
            best = d;
            facts.target = Some(*k);
        }
    }
    if facts.target.is_some() {
        s[Action::Attack as usize] = cfg.attack;
    }

    // Pursue: an enemy was seen (or heard) less than `memory_s` ago.
    let mut freshest = f64::NEG_INFINITY;
    for k in view.sightings {
        if k.seen_at > freshest && view.time - k.seen_at < f64::from(cfg.memory_s) {
            freshest = k.seen_at;
            facts.recent = Some(*k);
        }
    }
    if let Some(k) = facts.recent {
        let age = (view.time - k.seen_at) as f32;
        // Fades to half over the memory, then stops: a stale lead still beats patrol.
        s[Action::Pursue as usize] = cfg.pursue * (1.0 - 0.5 * age / cfg.memory_s);
    }

    // Heal: (1 - hp / hp_max) w / (1 + d / k), d to the nearest health pickup.
    let missing = (1.0 - me.health / me.max_health.max(1.0)).clamp(0.0, 1.0);
    let mut heal_d = f32::INFINITY;
    let mut pick_d = f32::INFINITY;
    for p in view.pickups.iter().filter(|p| p.available) {
        let d = p.pos.distance(me.pos);
        match p.kind {
            PickupKind::Health => {
                if d < heal_d {
                    heal_d = d;
                    facts.heal_at = Some(p.pos);
                }
            }
            PickupKind::Weapon | PickupKind::Bonus => {
                if d < pick_d {
                    pick_d = d;
                    facts.pickup_at = Some(p.pos);
                }
            }
        }
    }
    if heal_d.is_finite() {
        s[Action::Heal as usize] = missing * cfg.heal_weight / (1.0 + heal_d / cfg.heal_k);
    }
    if pick_d.is_finite() {
        s[Action::Pickup as usize] = cfg.pickup_weight / (1.0 + pick_d / cfg.pickup_k);
    }

    if view.objective.is_some() {
        s[Action::Objective as usize] = cfg.objective;
    }
    s[Action::Patrol as usize] = cfg.patrol;
    (Scores(s), facts)
}

/// The best action; the current one has its score raised by the bonus.
#[must_use]
pub fn choose(scores: &Scores, current: Option<Action>, cfg: &UtilityConfig) -> Action {
    let mut best = (Action::Patrol, f32::NEG_INFINITY);
    for a in Action::ALL {
        let mut v = scores.of(a);
        if Some(a) == current {
            v *= 1.0 + cfg.current_bonus;
        }
        // Strictly greater: earlier actions win ties.
        if v > best.1 {
            best = (a, v);
        }
    }
    best.0
}

#[cfg(test)]
mod tests {
    use faset_core::glam::Vec3;
    use faset_core::{NetId, Tick};

    use super::*;
    use crate::view::{PickupInfo, SelfState, WeaponParams};
    use crate::NavGraph;

    fn nav() -> NavGraph {
        NavGraph::from_parts(Vec::new(), Vec::new())
    }

    fn view<'a>(
        nav: &'a NavGraph,
        health: f32,
        sightings: &'a [Sighting],
        pickups: &'a [PickupInfo],
    ) -> BotView<'a> {
        BotView {
            tick: Tick(600),
            time: 10.0,
            me: SelfState {
                pos: Vec3::ZERO,
                vel: Vec3::ZERO,
                eye: Vec3::Y * 1.6,
                yaw: 0.0,
                pitch: 0.0,
                on_ground: true,
                health,
                max_health: 100.0,
            },
            sightings,
            pickups,
            objective: None,
            weapon: WeaponParams::hitscan(40.0),
            nav,
        }
    }

    fn sighting(at: Vec3, seen_at: f64, visible: bool) -> Sighting {
        Sighting {
            id: NetId(2),
            pos: at,
            vel: Vec3::ZERO,
            seen_at,
            seen: true,
            visible,
        }
    }

    #[test]
    fn idle_bot_patrols() {
        let n = nav();
        let (s, _) = score(&view(&n, 100.0, &[], &[]), &UtilityConfig::default());
        assert_eq!(choose(&s, None, &UtilityConfig::default()), Action::Patrol);
    }

    #[test]
    fn visible_enemy_in_range_is_attacked_out_of_range_is_pursued() {
        let n = nav();
        let cfg = UtilityConfig::default();
        let near = [sighting(Vec3::new(0.0, 0.0, -20.0), 10.0, true)];
        let (s, f) = score(&view(&n, 100.0, &near, &[]), &cfg);
        assert_eq!(choose(&s, None, &cfg), Action::Attack);
        assert!(f.target.is_some());
        let far = [sighting(Vec3::new(0.0, 0.0, -60.0), 10.0, true)];
        let (s, _) = score(&view(&n, 100.0, &far, &[]), &cfg);
        assert_eq!(choose(&s, None, &cfg), Action::Pursue);
    }

    #[test]
    fn pursue_lasts_five_seconds() {
        let n = nav();
        let cfg = UtilityConfig::default();
        let at = Vec3::new(0.0, 0.0, -20.0);
        let seen = |ago: f64| [sighting(at, 10.0 - ago, false)];
        let (s, _) = score(&view(&n, 100.0, &seen(4.9), &[]), &cfg);
        assert_eq!(choose(&s, None, &cfg), Action::Pursue);
        let (s, _) = score(&view(&n, 100.0, &seen(5.1), &[]), &cfg);
        assert_eq!(choose(&s, None, &cfg), Action::Patrol);
    }

    #[test]
    fn heal_score_follows_the_formula() {
        let n = nav();
        let cfg = UtilityConfig::default();
        let pk = [PickupInfo {
            kind: PickupKind::Health,
            pos: Vec3::new(6.0, 0.0, 0.0),
            available: true,
        }];
        let (s, _) = score(&view(&n, 25.0, &[], &pk), &cfg);
        let want = (1.0 - 0.25) * cfg.heal_weight / (1.0 + 6.0 / cfg.heal_k);
        assert!((s.of(Action::Heal) - want).abs() < 1e-5);
        assert_eq!(choose(&s, None, &cfg), Action::Heal);
        // Full health: no urge to heal.
        let (s, _) = score(&view(&n, 100.0, &[], &pk), &cfg);
        assert_eq!(s.of(Action::Heal), 0.0);
        // A spent pickup does not count.
        let spent = [PickupInfo {
            available: false,
            ..pk[0]
        }];
        let (s, _) = score(&view(&n, 25.0, &[], &spent), &cfg);
        assert_eq!(s.of(Action::Heal), 0.0);
    }

    #[test]
    fn low_health_beats_a_fight_only_when_the_heal_is_close() {
        let n = nav();
        let cfg = UtilityConfig::default();
        let enemy = [sighting(Vec3::new(0.0, 0.0, -20.0), 10.0, true)];
        let near = [PickupInfo {
            kind: PickupKind::Health,
            pos: Vec3::new(2.0, 0.0, 0.0),
            available: true,
        }];
        let far = [PickupInfo {
            pos: Vec3::new(80.0, 0.0, 0.0),
            ..near[0]
        }];
        let (s, _) = score(&view(&n, 10.0, &enemy, &near), &cfg);
        assert_eq!(choose(&s, None, &cfg), Action::Heal);
        let (s, _) = score(&view(&n, 10.0, &enemy, &far), &cfg);
        assert_eq!(choose(&s, None, &cfg), Action::Attack);
    }

    #[test]
    fn current_action_keeps_a_fifteen_percent_bonus() {
        let cfg = UtilityConfig::default();
        let mut raw = [0.0; 6];
        raw[Action::Pursue as usize] = 0.60;
        raw[Action::Pickup as usize] = 0.65; // 8 % better: not enough
        let s = Scores(raw);
        assert_eq!(choose(&s, Some(Action::Pursue), &cfg), Action::Pursue);
        assert_eq!(choose(&s, None, &cfg), Action::Pickup);
        raw[Action::Pickup as usize] = 0.72; // 20 % better: switches
        assert_eq!(
            choose(&Scores(raw), Some(Action::Pursue), &cfg),
            Action::Pickup
        );
    }
}

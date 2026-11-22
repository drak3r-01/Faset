//! Pickups and timed bonuses (plan lot 4.7).
//!
//! A pickup is a trigger sphere of [`RADIUS`] metres. The server alone
//! decides who collects it ([`Pickups::step`]); clients only learn it from
//! the confirmation message. Timed bonuses ([`ActiveBonuses`]) each keep
//! their own timer, so they stack in time.

use faset_core::glam::Vec3;
use faset_core::NetId;

/// Trigger sphere radius, metres.
pub const RADIUS: f32 = 1.0;
/// Health given by a heal pickup.
pub const HEAL_AMOUNT: f32 = 50.0;
/// Damage absorbed by an armour pickup.
pub const ARMOUR_AMOUNT: f32 = 50.0;
/// Highest armour a player can hold (decision: two armour pickups).
pub const MAX_ARMOUR: f32 = 100.0;
/// Speed bonus multiplier.
pub const SPEED_MULTIPLIER: f32 = 1.3;
/// Damage bonus multiplier.
pub const DAMAGE_MULTIPLIER: f32 = 1.5;
/// Super jump multiplier of the jump speed.
pub const JUMP_MULTIPLIER: f32 = 1.5;
/// Prefix of the objective zone ids that place pickups on a map, followed
/// by [`PickupKind::name`]: `pickup.heal`, `pickup.speed`, ... (decision:
/// the map format's own pickup kinds do not cover the seven bonuses, so the
/// mode reads them from objective zones; the zone centre is the sphere
/// centre).
pub const ZONE_PREFIX: &str = "pickup.";

/// What a pickup gives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PickupKind {
    /// +50 health.
    Heal,
    /// Reserve ammunition refilled to full.
    Ammo,
    /// Armour absorbing 50 damage.
    Armour,
    /// Speed x1.3 for 10 s.
    Speed,
    /// Damage x1.5 for 15 s.
    Damage,
    /// Jump speed x1.5 for 15 s.
    SuperJump,
    /// Enemy heads enlarged for 10 s (comic).
    BigHead,
}

/// Every kind, in wire order.
pub const KINDS: [PickupKind; 7] = [
    PickupKind::Heal,
    PickupKind::Ammo,
    PickupKind::Armour,
    PickupKind::Speed,
    PickupKind::Damage,
    PickupKind::SuperJump,
    PickupKind::BigHead,
];

impl PickupKind {
    /// Wire code.
    #[must_use]
    pub fn code(self) -> u8 {
        self as u8
    }

    /// Inverse of [`code`](Self::code).
    #[must_use]
    pub fn from_code(code: u8) -> Option<Self> {
        KINDS.get(usize::from(code)).copied()
    }

    /// Short name, used in map zone ids.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Heal => "heal",
            Self::Ammo => "ammo",
            Self::Armour => "armour",
            Self::Speed => "speed",
            Self::Damage => "damage",
            Self::SuperJump => "superjump",
            Self::BigHead => "bighead",
        }
    }

    /// The kind named by a zone id such as `pickup.heal`.
    #[must_use]
    pub fn from_zone_id(id: &str) -> Option<Self> {
        let name = id.strip_prefix(ZONE_PREFIX)?;
        KINDS.into_iter().find(|k| k.name() == name)
    }

    /// Seconds before the pickup comes back, at the default spawn rate.
    #[must_use]
    pub fn respawn_seconds(self) -> f64 {
        match self {
            Self::Heal => 30.0,
            Self::Ammo => 20.0,
            Self::Armour | Self::Speed | Self::SuperJump => 45.0,
            Self::Damage | Self::BigHead => 60.0,
        }
    }

    /// Seconds a timed bonus lasts; `None` for instant pickups.
    #[must_use]
    pub fn duration_seconds(self) -> Option<f64> {
        match self {
            Self::Heal | Self::Ammo | Self::Armour => None,
            Self::Speed | Self::BigHead => Some(10.0),
            Self::Damage | Self::SuperJump => Some(15.0),
        }
    }

    fn timer_slot(self) -> Option<usize> {
        TIMED.iter().position(|&k| k == self)
    }
}

/// The kinds that last for a while, in wire order.
pub const TIMED: [PickupKind; 4] = [
    PickupKind::Speed,
    PickupKind::Damage,
    PickupKind::SuperJump,
    PickupKind::BigHead,
];

/// The timed bonuses of one player, each with its own end time.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ActiveBonuses {
    until: [f64; 4],
}

impl ActiveBonuses {
    /// Starts (or restarts) the timer of `kind` at `now`. Instant kinds do
    /// nothing. Collecting a bonus again restarts its full duration
    /// (decision); other bonuses are untouched.
    pub fn activate(&mut self, kind: PickupKind, now: f64) {
        if let (Some(slot), Some(d)) = (kind.timer_slot(), kind.duration_seconds()) {
            self.until[slot] = now + d;
        }
    }

    /// Seconds left on `kind` (0 when inactive).
    #[must_use]
    pub fn remaining(&self, kind: PickupKind, now: f64) -> f64 {
        kind.timer_slot()
            .map_or(0.0, |s| (self.until[s] - now).max(0.0))
    }

    /// Whether `kind` is active at `now`.
    #[must_use]
    pub fn active(&self, kind: PickupKind, now: f64) -> bool {
        self.remaining(kind, now) > 0.0
    }

    /// Drops every bonus (death).
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Run speed multiplier, 1 without the bonus.
    #[must_use]
    pub fn speed_multiplier(&self, now: f64) -> f32 {
        if self.active(PickupKind::Speed, now) {
            SPEED_MULTIPLIER
        } else {
            1.0
        }
    }

    /// Jump speed multiplier, 1 without the bonus.
    #[must_use]
    pub fn jump_multiplier(&self, now: f64) -> f32 {
        if self.active(PickupKind::SuperJump, now) {
            JUMP_MULTIPLIER
        } else {
            1.0
        }
    }

    /// Damage dealt multiplier, 1 without the bonus; feeds the bonus factor
    /// of the lot 4.4 damage chain.
    #[must_use]
    pub fn damage_multiplier(&self, now: f64) -> f32 {
        if self.active(PickupKind::Damage, now) {
            DAMAGE_MULTIPLIER
        } else {
            1.0
        }
    }

    /// Whether the owner sees enlarged enemy heads.
    #[must_use]
    pub fn big_head(&self, now: f64) -> bool {
        self.active(PickupKind::BigHead, now)
    }
}

/// One pickup of the map.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pickup {
    pub kind: PickupKind,
    /// Sphere centre.
    pub position: Vec3,
    /// Time it comes back, `None` while available.
    pub respawn_at: Option<f64>,
}

impl Pickup {
    /// Whether a player can take it now.
    #[must_use]
    pub fn available(&self) -> bool {
        self.respawn_at.is_none()
    }
}

/// What happened to a pickup during a step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickupEvent {
    /// `player` took pickup `index`.
    Taken { index: u16, player: NetId },
    /// Pickup `index` is back.
    Respawned { index: u16 },
}

/// Every pickup of the match, and the server rule that collects them.
#[derive(Debug, Clone, PartialEq)]
pub struct Pickups {
    items: Vec<Pickup>,
    /// Respawn delays in percent of the table (200 = twice as long).
    delay_percent: u32,
}

impl Default for Pickups {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            delay_percent: 100,
        }
    }
}

impl Pickups {
    /// Pickups at `places`, all available.
    #[must_use]
    pub fn new(places: impl IntoIterator<Item = (PickupKind, Vec3)>) -> Self {
        Self {
            items: places
                .into_iter()
                .map(|(kind, position)| Pickup {
                    kind,
                    position,
                    respawn_at: None,
                })
                .collect(),
            delay_percent: 100,
        }
    }

    /// Sets the respawn delay scale, in percent of the table.
    pub fn set_delay_percent(&mut self, percent: u32) {
        self.delay_percent = percent.max(1);
    }

    /// Every pickup.
    #[must_use]
    pub fn items(&self) -> &[Pickup] {
        &self.items
    }

    /// Makes every pickup available (a match starts).
    pub fn reset(&mut self) {
        for p in &mut self.items {
            p.respawn_at = None;
        }
    }

    /// Replaces the pickups with the given state (client mirror).
    pub fn replace(&mut self, items: Vec<Pickup>) {
        self.items = items;
    }

    /// Marks one pickup taken or back (client mirror of a confirmation).
    pub fn set_available(&mut self, index: usize, available: bool) {
        if let Some(p) = self.items.get_mut(index) {
            p.respawn_at = if available { None } else { Some(f64::INFINITY) };
        }
    }

    /// One server step at `now`: brings back the pickups whose delay is
    /// over, then lets each available pickup be taken by the first player
    /// (ascending id) within [`RADIUS`] of its centre for whom `accept`
    /// returns `true` (it applies the effect; it may refuse a useless
    /// pickup, such as a heal at full health).
    pub fn step(
        &mut self,
        now: f64,
        players: &[(NetId, Vec3)],
        mut accept: impl FnMut(NetId, PickupKind) -> bool,
    ) -> Vec<PickupEvent> {
        let mut events = Vec::new();
        for (i, p) in self.items.iter_mut().enumerate() {
            let index = u16::try_from(i).unwrap_or(u16::MAX);
            if p.respawn_at.is_some_and(|t| now >= t) {
                p.respawn_at = None;
                events.push(PickupEvent::Respawned { index });
            }
            if !p.available() {
                continue;
            }
            let taker = players
                .iter()
                .filter(|(_, pos)| pos.distance_squared(p.position) <= RADIUS * RADIUS)
                .find(|&&(id, _)| accept(id, p.kind));
            if let Some(&(player, _)) = taker {
                let delay = p.kind.respawn_seconds() * f64::from(self.delay_percent) / 100.0;
                p.respawn_at = Some(now + delay);
                events.push(PickupEvent::Taken { index, player });
            }
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: NetId = NetId(1);
    const B: NetId = NetId(2);

    #[test]
    fn table_matches_the_plan() {
        let table: Vec<(f64, Option<f64>)> = KINDS
            .iter()
            .map(|k| (k.respawn_seconds(), k.duration_seconds()))
            .collect();
        assert_eq!(
            table,
            [
                (30.0, None),
                (20.0, None),
                (45.0, None),
                (45.0, Some(10.0)),
                (60.0, Some(15.0)),
                (45.0, Some(15.0)),
                (60.0, Some(10.0)),
            ]
        );
        for k in KINDS {
            assert_eq!(PickupKind::from_code(k.code()), Some(k));
            let id = format!("{ZONE_PREFIX}{}", k.name());
            assert_eq!(PickupKind::from_zone_id(&id), Some(k));
        }
        assert_eq!(PickupKind::from_code(7), None);
        assert_eq!(PickupKind::from_zone_id("pickup.nope"), None);
        assert_eq!(PickupKind::from_zone_id("goal"), None);
    }

    #[test]
    fn each_bonus_has_its_multiplier_and_duration() {
        let mut b = ActiveBonuses::default();
        assert_eq!(b.speed_multiplier(0.0), 1.0);
        b.activate(PickupKind::Speed, 100.0);
        b.activate(PickupKind::Damage, 100.0);
        b.activate(PickupKind::SuperJump, 100.0);
        b.activate(PickupKind::BigHead, 100.0);
        assert_eq!(b.speed_multiplier(109.9), 1.3);
        assert_eq!(b.speed_multiplier(110.0), 1.0);
        assert_eq!(b.damage_multiplier(114.9), 1.5);
        assert_eq!(b.damage_multiplier(115.0), 1.0);
        assert_eq!(b.jump_multiplier(114.9), 1.5);
        assert_eq!(b.jump_multiplier(115.0), 1.0);
        assert!(b.big_head(109.9));
        assert!(!b.big_head(110.0));
        // Each timer runs on its own: speed is over while damage goes on.
        assert!(!b.active(PickupKind::Speed, 112.0));
        assert!(b.active(PickupKind::Damage, 112.0));
    }

    #[test]
    fn collecting_again_restarts_the_timer_and_instants_have_none() {
        let mut b = ActiveBonuses::default();
        b.activate(PickupKind::Speed, 0.0);
        b.activate(PickupKind::Speed, 6.0);
        assert!(b.active(PickupKind::Speed, 15.9));
        b.activate(PickupKind::Heal, 0.0);
        assert_eq!(b, {
            let mut c = ActiveBonuses::default();
            c.activate(PickupKind::Speed, 6.0);
            c
        });
        b.clear();
        assert!(!b.active(PickupKind::Speed, 7.0));
    }

    #[test]
    fn server_picks_the_first_player_in_the_sphere_and_respawns_after_the_delay() {
        for kind in KINDS {
            let mut p = Pickups::new([(kind, Vec3::new(0.0, 1.0, 0.0))]);
            let near = Vec3::new(0.9, 1.0, 0.0);
            let far = Vec3::new(1.1, 1.0, 0.0);
            let delay = kind.respawn_seconds();
            assert!(p.step(5.0, &[(A, far)], |_, _| true).is_empty());
            let ev = p.step(5.0, &[(B, near), (A, near)], |_, _| true);
            assert_eq!(
                ev,
                [PickupEvent::Taken {
                    index: 0,
                    player: B
                }]
            );
            assert!(!p.items()[0].available());
            // Still gone one tick before the delay, back right at it.
            assert!(p
                .step(5.0 + delay - 0.01, &[(A, near)], |_, _| true)
                .is_empty());
            let ev = p.step(5.0 + delay, &[], |_, _| true);
            assert_eq!(ev, [PickupEvent::Respawned { index: 0 }]);
            assert!(p.items()[0].available());
        }
    }

    #[test]
    fn a_refusing_player_leaves_the_pickup_and_the_next_one_takes_it() {
        let mut p = Pickups::new([(PickupKind::Heal, Vec3::ZERO)]);
        let ev = p.step(0.0, &[(A, Vec3::ZERO), (B, Vec3::ZERO)], |id, _| id == B);
        assert_eq!(
            ev,
            [PickupEvent::Taken {
                index: 0,
                player: B
            }]
        );
        let mut q = Pickups::new([(PickupKind::Heal, Vec3::ZERO)]);
        assert!(q.step(0.0, &[(A, Vec3::ZERO)], |_, _| false).is_empty());
        assert!(q.items()[0].available());
    }

    #[test]
    fn delay_scale_stretches_the_respawn() {
        let mut p = Pickups::new([(PickupKind::Heal, Vec3::ZERO)]);
        p.set_delay_percent(200);
        p.step(0.0, &[(A, Vec3::ZERO)], |_, _| true);
        assert!(p.step(59.9, &[], |_, _| true).is_empty());
        assert_eq!(
            p.step(60.0, &[], |_, _| true),
            [PickupEvent::Respawned { index: 0 }]
        );
    }
}

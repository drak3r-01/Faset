//! Map rotation (lot 5.1): the order maps are played in, the rotation
//! clock and the provider of prepared maps.
//!
//! [`Rotation`] is a pure state machine counted in ticks, so tests run it
//! on a simulated clock; [`Server::set_rotation`](crate::Server::set_rotation)
//! drives it, prepares the next map on a worker thread
//! ([`MapProvider::prepare`]), sends `MapChange`, serves the map to clients
//! lacking it and starts warm-up once clients are ready.
//!
//! Timeline of one map, with the defaults: the map is played for
//! `rotation_seconds` (300); at 270 s the next map is prepared; at 300 s the
//! match ends and the scoreboard shows for 8 s; the server then sends
//! `MapChange` and waits until every client reported ready, at most 10 s;
//! then the new match starts in warm-up.

use std::any::Any;

use faset_core::TICK_RATE;
use faset_minigame_api::Match;

use crate::config::MapConfig;

const SECOND: u32 = TICK_RATE as u32;

/// How the next map is chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RotationOrder {
    /// The maps in list order, wrapping around.
    #[default]
    Sequential,
    /// Random, but every map is played once before any repeats.
    RandomNoRepeat,
}

/// Rotation timing and order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RotationConfig {
    /// Seconds each map is played (default 300).
    pub rotation_seconds: u32,
    /// How the next map is chosen.
    pub order: RotationOrder,
    /// The next map is prepared this long before the end (default 30 s).
    pub prepare_seconds: u32,
    /// The scoreboard stays this long (default 8 s).
    pub scoreboard_seconds: u32,
    /// Longest wait for clients to report ready (default 10 s).
    pub ready_timeout_seconds: u32,
    /// Seed of the random order.
    pub seed: u64,
}

impl Default for RotationConfig {
    fn default() -> Self {
        Self {
            rotation_seconds: 300,
            order: RotationOrder::Sequential,
            prepare_seconds: 30,
            scoreboard_seconds: 8,
            ready_timeout_seconds: 10,
            seed: 0,
        }
    }
}

/// Chooses the index of the next map.
#[derive(Debug, Clone)]
pub struct MapOrder {
    order: RotationOrder,
    count: usize,
    current: usize,
    /// Maps already played in the current cycle (random order).
    played: Vec<bool>,
    rng: u64,
}

impl MapOrder {
    /// `count` maps, currently on `current`.
    ///
    /// # Panics
    /// If `count` is 0 or `current` is out of range.
    #[must_use]
    pub fn new(order: RotationOrder, count: usize, current: usize, seed: u64) -> Self {
        assert!(count > 0 && current < count, "invalid rotation list");
        let mut played = vec![false; count];
        played[current] = true;
        Self {
            order,
            count,
            current,
            played,
            rng: seed ^ 0x9E37_79B9_7F4A_7C15,
        }
    }

    /// The map being played.
    #[must_use]
    pub fn current(&self) -> usize {
        self.current
    }

    fn random(&mut self) -> u64 {
        // xorshift64*; the state is kept odd so it never reaches zero.
        self.rng |= 1;
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        self.rng.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Chooses the next map and makes it the current one.
    pub fn take_next(&mut self) -> usize {
        let next = match self.order {
            RotationOrder::Sequential => (self.current + 1) % self.count,
            RotationOrder::RandomNoRepeat => {
                if self.played.iter().all(|&p| p) {
                    // A new cycle: the map just played cannot come first.
                    self.played = vec![false; self.count];
                    self.played[self.current] = true;
                }
                let left: Vec<usize> = (0..self.count).filter(|&i| !self.played[i]).collect();
                if left.is_empty() {
                    self.current
                } else {
                    left[(self.random() % left.len() as u64) as usize]
                }
            }
        };
        self.played[next] = true;
        self.current = next;
        next
    }
}

/// What the server must do now, from [`Rotation::tick`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RotationAction {
    /// Start preparing map `index` on a worker thread.
    Prepare(usize),
    /// The match ends: show the scoreboard.
    Scoreboard,
    /// Switch to map `index` (the prepared one): send `MapChange`.
    Switch(usize),
    /// Clients are ready (or the wait ran out): start the new match.
    Warmup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Playing,
    Scoreboard,
    WaitReady,
}

/// The rotation clock.
#[derive(Debug, Clone)]
pub struct Rotation {
    config: RotationConfig,
    order: MapOrder,
    stage: Stage,
    ticks: u32,
    next: Option<usize>,
}

impl Rotation {
    /// A rotation over `count` maps, starting on map `current`.
    ///
    /// # Panics
    /// If `count` is 0 or `current` is out of range.
    #[must_use]
    pub fn new(config: RotationConfig, count: usize, current: usize) -> Self {
        Self {
            order: MapOrder::new(config.order, count, current, config.seed),
            config,
            stage: Stage::Playing,
            ticks: 0,
            next: None,
        }
    }

    /// The map being played (or switched to).
    #[must_use]
    pub fn current(&self) -> usize {
        self.order.current()
    }

    /// Whether the match is over: scoreboard or switch in progress.
    #[must_use]
    pub fn between_maps(&self) -> bool {
        self.stage != Stage::Playing
    }

    /// Whether clients are being waited for (`MapChange` was sent).
    #[must_use]
    pub fn waiting_ready(&self) -> bool {
        self.stage == Stage::WaitReady
    }

    /// Advances one tick. `all_ready` tells whether every client reported
    /// ready since the `MapChange`.
    pub fn tick(&mut self, all_ready: bool) -> Option<RotationAction> {
        self.ticks += 1;
        let c = self.config;
        match self.stage {
            Stage::Playing => {
                let end = c.rotation_seconds.max(1) * SECOND;
                let prepare_at = end.saturating_sub(c.prepare_seconds * SECOND).min(end - 1);
                if self.ticks == prepare_at && self.next.is_none() {
                    let next = self.order.take_next();
                    self.next = Some(next);
                    return Some(RotationAction::Prepare(next));
                }
                if self.ticks >= end {
                    self.stage = Stage::Scoreboard;
                    self.ticks = 0;
                    return Some(RotationAction::Scoreboard);
                }
            }
            Stage::Scoreboard => {
                if self.ticks >= c.scoreboard_seconds * SECOND {
                    self.stage = Stage::WaitReady;
                    self.ticks = 0;
                    let next = self.next.take().unwrap_or_else(|| self.order.take_next());
                    return Some(RotationAction::Switch(next));
                }
            }
            Stage::WaitReady => {
                if all_ready || self.ticks >= c.ready_timeout_seconds * SECOND {
                    self.stage = Stage::Playing;
                    self.ticks = 0;
                    return Some(RotationAction::Warmup);
                }
            }
        }
        None
    }

    /// The switch failed (the map could not be prepared): keep playing map
    /// `current` with a fresh clock.
    pub fn abort_switch(&mut self, current: usize) {
        let count = self.order.count;
        self.order = MapOrder::new(
            self.config.order,
            count,
            current.min(count - 1),
            self.config.seed ^ u64::from(self.ticks),
        );
        self.stage = Stage::Playing;
        self.ticks = 0;
        self.next = None;
    }
}

/// A map ready to be switched to.
pub struct PreparedMap {
    /// Display name (log only; names are never used as paths).
    pub name: String,
    /// The map file as clients load it: the transfer payload, hashed by
    /// [`map_fingerprint`](crate::map_fingerprint). Empty for a map that
    /// cannot be transferred (embedded in the binary).
    pub file: Vec<u8>,
    /// Collision, spawns and zones for the server.
    pub config: MapConfig,
    /// Whatever the provider needs back in [`MapProvider::new_match`].
    pub payload: Option<Box<dyn Any + Send>>,
}

impl std::fmt::Debug for PreparedMap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedMap")
            .field("name", &self.name)
            .field("file_bytes", &self.file.len())
            .field("config", &self.config)
            .finish()
    }
}

/// The maps of the rotation. The dedicated server runs load steps 1, 3 and
/// 4 in [`prepare`](Self::prepare) (read, migrate, symmetry and prefabs;
/// collisions; navigation graph); meshes and the height map are client-only.
pub trait MapProvider: Send + Sync {
    /// Number of maps in the rotation.
    fn count(&self) -> usize;

    /// Loads map `index` completely. Runs on a worker thread.
    ///
    /// # Errors
    /// A message for the log when the map cannot be loaded.
    fn prepare(&self, index: usize) -> Result<PreparedMap, String>;

    /// The match to host on a freshly prepared map; `None` for no match.
    fn new_match(&self, _map: &mut PreparedMap) -> Option<Match> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs `ticks` ticks, collecting `(tick, action)`.
    fn run(
        r: &mut Rotation,
        ticks: u32,
        ready: impl Fn(u32) -> bool,
    ) -> Vec<(u32, RotationAction)> {
        (1..=ticks)
            .filter_map(|t| r.tick(ready(t)).map(|a| (t, a)))
            .collect()
    }

    #[test]
    fn rotation_fires_at_300_s_and_prepares_at_270_s() {
        let mut r = Rotation::new(RotationConfig::default(), 3, 0);
        let actions = run(&mut r, 300 * 60 + 8 * 60 + 5, |_| false);
        assert_eq!(
            actions,
            [
                (270 * 60, RotationAction::Prepare(1)),
                (300 * 60, RotationAction::Scoreboard),
                (308 * 60, RotationAction::Switch(1)),
            ]
        );
        assert!(r.waiting_ready());
    }

    #[test]
    fn warm_up_waits_for_ready_clients_but_at_most_10_s() {
        let switch = 308 * 60;
        let ready_at = 123;
        let mut r = Rotation::new(RotationConfig::default(), 2, 0);
        let mut ticks = run(&mut r, switch, |_| false);
        assert_eq!(ticks.last(), Some(&(switch, RotationAction::Switch(1))));
        ticks = (1..=2000)
            .filter_map(|t| r.tick(t >= ready_at).map(|a| (t, a)))
            .collect();
        assert_eq!(ticks, [(ready_at, RotationAction::Warmup)]);

        // Never ready: warm-up starts after 10 s and the clock restarts.
        let mut r = Rotation::new(RotationConfig::default(), 2, 0);
        run(&mut r, switch, |_| false);
        let late = run(&mut r, 700, |_| false);
        assert_eq!(late, [(600, RotationAction::Warmup)]);
        assert!(!r.between_maps());
        assert_eq!(r.current(), 1);
        let next = run(&mut r, 270 * 60, |_| false);
        assert_eq!(next, [(270 * 60 - 100, RotationAction::Prepare(0))]);
    }

    #[test]
    fn sequential_order_wraps() {
        let mut o = MapOrder::new(RotationOrder::Sequential, 3, 1, 0);
        assert_eq!((o.take_next(), o.take_next(), o.take_next()), (2, 0, 1));
    }

    #[test]
    fn random_order_covers_every_map_before_a_repeat() {
        for seed in 0..20 {
            let mut o = MapOrder::new(RotationOrder::RandomNoRepeat, 4, 0, seed);
            let mut played = vec![0usize];
            for _ in 0..11 {
                played.push(o.take_next());
            }
            // The first cycle is the start map and three others; each later
            // cycle is the map before it plus the three others.
            let mut cycles = vec![&played[0..4]];
            cycles.extend(
                (4..played.len())
                    .step_by(3)
                    .map(|s| &played[s - 1..(s + 3).min(played.len())]),
            );
            for cycle in cycles {
                let mut w = cycle.to_vec();
                w.sort_unstable();
                w.dedup();
                assert_eq!(w.len(), cycle.len(), "seed {seed}: {played:?}");
            }
        }
        assert_eq!(
            MapOrder::new(RotationOrder::RandomNoRepeat, 1, 0, 1).take_next(),
            0
        );
    }
}

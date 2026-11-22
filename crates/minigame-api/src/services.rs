//! Common services given to modes through [`ModeCtx`](crate::ModeCtx).
//!
//! This lot gives the minimum: spawn points, teams, score table, timers,
//! event feed and a bot control stub. Damage, pickups and weapons arrive
//! with lots 4.4 and 4.7.

use std::collections::{BTreeMap, VecDeque};

use faset_core::glam::Vec3;
use faset_core::{NetId, Tick};

/// Every service of a match.
#[derive(Debug, Clone, Default)]
pub struct Services {
    pub spawns: SpawnPoints,
    pub teams: Teams,
    pub scores: ScoreTable,
    pub timers: Timers,
    pub feed: EventFeed,
    pub bots: BotControl,
}

/// Spawn and respawn points of the map, used round-robin, and the respawn
/// requests the host applies to the world.
#[derive(Debug, Clone, Default)]
pub struct SpawnPoints {
    points: Vec<Vec3>,
    next: usize,
    requests: Vec<(NetId, Vec3)>,
}

impl SpawnPoints {
    /// Points as capsule centres.
    #[must_use]
    pub fn new(points: Vec<Vec3>) -> Self {
        Self {
            points,
            next: 0,
            requests: Vec::new(),
        }
    }

    /// Every point.
    #[must_use]
    pub fn points(&self) -> &[Vec3] {
        &self.points
    }

    /// The next point of the rotation, `None` without points.
    pub fn pick(&mut self) -> Option<Vec3> {
        if self.points.is_empty() {
            return None;
        }
        let p = self.points[self.next % self.points.len()];
        self.next = (self.next + 1) % self.points.len();
        Some(p)
    }

    /// Asks the host to move `player` to the next point. Returns the point.
    pub fn respawn(&mut self, player: NetId) -> Option<Vec3> {
        let p = self.pick()?;
        self.requests.push((player, p));
        Some(p)
    }

    /// Pending respawn requests, oldest first.
    pub fn take_requests(&mut self) -> Vec<(NetId, Vec3)> {
        std::mem::take(&mut self.requests)
    }
}

/// A team number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TeamId(pub u8);

/// Team membership. A mode with no teams leaves it empty.
#[derive(Debug, Clone, Default)]
pub struct Teams {
    count: u8,
    members: BTreeMap<NetId, TeamId>,
}

impl Teams {
    /// Sets the number of teams; clears membership.
    pub fn set_count(&mut self, count: u8) {
        self.count = count;
        self.members.clear();
    }

    /// Number of teams.
    #[must_use]
    pub fn count(&self) -> u8 {
        self.count
    }

    /// Puts `player` in `team`.
    pub fn assign(&mut self, player: NetId, team: TeamId) {
        self.members.insert(player, team);
    }

    /// Puts `player` in the smallest team (lowest id on a tie); `None` with
    /// no teams.
    pub fn auto_assign(&mut self, player: NetId) -> Option<TeamId> {
        self.members.remove(&player);
        let team = (0..self.count)
            .map(TeamId)
            .min_by_key(|&t| (self.members.values().filter(|&&m| m == t).count(), t))?;
        self.members.insert(player, team);
        Some(team)
    }

    /// Removes `player` from its team.
    pub fn remove(&mut self, player: NetId) {
        self.members.remove(&player);
    }

    /// The team of `player`.
    #[must_use]
    pub fn team_of(&self, player: NetId) -> Option<TeamId> {
        self.members.get(&player).copied()
    }

    /// Members of `team`, ascending.
    pub fn members(&self, team: TeamId) -> impl Iterator<Item = NetId> + '_ {
        self.members
            .iter()
            .filter(move |(_, &t)| t == team)
            .map(|(&p, _)| p)
    }
}

/// Score of every player in the match.
#[derive(Debug, Clone, Default)]
pub struct ScoreTable {
    scores: BTreeMap<NetId, i32>,
}

impl ScoreTable {
    /// Score of `player` (0 if absent).
    #[must_use]
    pub fn get(&self, player: NetId) -> i32 {
        self.scores.get(&player).copied().unwrap_or(0)
    }

    /// Adds a player at 0 if absent.
    pub fn enter(&mut self, player: NetId) {
        self.scores.entry(player).or_insert(0);
    }

    /// Removes a player.
    pub fn remove(&mut self, player: NetId) {
        self.scores.remove(&player);
    }

    /// Sets a score.
    pub fn set(&mut self, player: NetId, score: i32) {
        self.scores.insert(player, score);
    }

    /// Adds `delta` and returns the new score.
    pub fn add(&mut self, player: NetId, delta: i32) -> i32 {
        let s = self.scores.entry(player).or_insert(0);
        *s = s.saturating_add(delta);
        *s
    }

    /// Players by descending score, ascending id on a tie.
    #[must_use]
    pub fn ranking(&self) -> Vec<(NetId, i32)> {
        let mut r: Vec<_> = self.scores.iter().map(|(&p, &s)| (p, s)).collect();
        r.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        r
    }

    /// Players sharing the best score; empty if nobody scored above 0.
    #[must_use]
    pub fn leaders(&self) -> Vec<NetId> {
        let Some(best) = self.scores.values().copied().max().filter(|&b| b > 0) else {
            return Vec::new();
        };
        self.scores
            .iter()
            .filter(|(_, &s)| s == best)
            .map(|(&p, _)| p)
            .collect()
    }
}

/// Named countdown timers in simulation ticks.
#[derive(Debug, Clone, Default)]
pub struct Timers {
    ends: BTreeMap<String, Tick>,
}

impl Timers {
    /// Starts (or restarts) `name` for `ticks` from `now`.
    pub fn start(&mut self, name: impl Into<String>, now: Tick, ticks: u32) {
        self.ends
            .insert(name.into(), Tick(now.0.saturating_add(ticks)));
    }

    /// Stops `name`.
    pub fn cancel(&mut self, name: &str) {
        self.ends.remove(name);
    }

    /// Ticks left on `name` (0 once expired), `None` if not running.
    #[must_use]
    pub fn remaining(&self, name: &str, now: Tick) -> Option<u32> {
        self.ends.get(name).map(|end| end.0.saturating_sub(now.0))
    }

    /// Whether `name` runs and has reached its end.
    #[must_use]
    pub fn expired(&self, name: &str, now: Tick) -> bool {
        self.remaining(name, now) == Some(0)
    }
}

/// A line of the event feed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedEntry {
    pub tick: Tick,
    pub text: String,
}

/// The last [`EventFeed::CAPACITY`] feed lines.
#[derive(Debug, Clone, Default)]
pub struct EventFeed {
    entries: VecDeque<FeedEntry>,
}

impl EventFeed {
    /// Lines kept (decision).
    pub const CAPACITY: usize = 32;

    /// Appends a line, dropping the oldest beyond capacity.
    pub fn push(&mut self, tick: Tick, text: impl Into<String>) {
        if self.entries.len() == Self::CAPACITY {
            self.entries.pop_front();
        }
        self.entries.push_back(FeedEntry {
            tick,
            text: text.into(),
        });
    }

    /// Lines, oldest first.
    pub fn entries(&self) -> impl Iterator<Item = &FeedEntry> {
        self.entries.iter()
    }
}

/// Bot control interface (stub): a mode states how many bots it wants; the
/// host's bot lot (4.9, 4.10) fulfils it.
#[derive(Debug, Clone, Default)]
pub struct BotControl {
    wanted: Option<usize>,
}

impl BotControl {
    /// Asks for `count` bots in the match.
    pub fn request(&mut self, count: usize) {
        self.wanted = Some(count);
    }

    /// The last request.
    #[must_use]
    pub fn wanted(&self) -> Option<usize> {
        self.wanted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawns_rotate() {
        let mut s = SpawnPoints::new(vec![Vec3::X, Vec3::Y]);
        assert_eq!(s.pick(), Some(Vec3::X));
        assert_eq!(s.respawn(NetId(4)), Some(Vec3::Y));
        assert_eq!(s.pick(), Some(Vec3::X));
        assert_eq!(s.take_requests(), vec![(NetId(4), Vec3::Y)]);
        assert!(SpawnPoints::default().pick().is_none());
    }

    #[test]
    fn teams_balance() {
        let mut t = Teams::default();
        assert_eq!(t.auto_assign(NetId(1)), None);
        t.set_count(2);
        assert_eq!(t.auto_assign(NetId(1)), Some(TeamId(0)));
        assert_eq!(t.auto_assign(NetId(2)), Some(TeamId(1)));
        assert_eq!(t.auto_assign(NetId(3)), Some(TeamId(0)));
        assert_eq!(
            t.members(TeamId(0)).collect::<Vec<_>>(),
            [NetId(1), NetId(3)]
        );
    }

    #[test]
    fn scores_rank_and_lead() {
        let mut s = ScoreTable::default();
        s.enter(NetId(1));
        assert!(s.leaders().is_empty());
        s.add(NetId(2), 3);
        s.add(NetId(1), 3);
        s.add(NetId(3), 1);
        assert_eq!(s.leaders(), [NetId(1), NetId(2)]);
        assert_eq!(s.ranking()[2], (NetId(3), 1));
    }

    #[test]
    fn timers_count_ticks() {
        let mut t = Timers::default();
        t.start("x", Tick(10), 5);
        assert_eq!(t.remaining("x", Tick(12)), Some(3));
        assert!(!t.expired("x", Tick(14)));
        assert!(t.expired("x", Tick(15)));
        assert_eq!(t.remaining("y", Tick(0)), None);
    }

    #[test]
    fn feed_is_bounded() {
        let mut f = EventFeed::default();
        for i in 0..40 {
            f.push(Tick(i), format!("{i}"));
        }
        assert_eq!(f.entries().count(), EventFeed::CAPACITY);
        assert_eq!(f.entries().next().unwrap().text, "8");
    }
}

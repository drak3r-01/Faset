//! Difficulty levels: a table of four rows, loaded from RON.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

/// The default table, `data/levels.ron`.
pub const DEFAULT_LEVELS_RON: &str = include_str!("../data/levels.ron");

/// One of the four difficulty levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Level {
    Easy,
    Normal,
    Hard,
    Expert,
}

impl Level {
    /// Every level, easiest first.
    pub const ALL: [Level; 4] = [Level::Easy, Level::Normal, Level::Hard, Level::Expert];

    /// The built-in parameters of the level (`data/levels.ron`).
    #[must_use]
    pub fn params(self) -> LevelParams {
        default_table().get(self).clone()
    }
}

/// Movement capabilities of a level, used by the path follower.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MoveFlags {
    /// Holds jump on straight lines (the controller's auto hop chains hops).
    pub bunny_hop: bool,
    /// Slides on straight lines at speed.
    pub slide: bool,
    /// Jumps out of a slide to keep its speed.
    pub slide_hop: bool,
    /// Strafes sideways while fighting.
    pub dodge: bool,
}

/// Everything that makes a level easier or harder.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LevelParams {
    /// Delay between an observation and the moment the bot can use it, seconds.
    pub reaction_s: f32,
    /// Standard deviation of the aim error, radians.
    pub aim_sigma: f32,
    /// Correlation time of the aim error, seconds.
    pub aim_tau: f32,
    /// Stiffness of the angular spring, rad/s per radian of error.
    pub aim_spring: f32,
    /// Cap of the rotation speed, rad/s.
    pub aim_max_speed: f32,
    /// Multiplier of the hit radius under which the bot opens fire.
    pub fire_tolerance: f32,
    /// Fraction of the target velocity used for the lead, 0 to 1.
    pub lead: f32,
    /// Movement capabilities.
    pub moves: MoveFlags,
}

impl Default for LevelParams {
    /// The Normal row, spelled out: the table itself is parsed with this
    /// default as the base of missing fields, so it cannot be read from it.
    fn default() -> Self {
        Self {
            reaction_s: 0.4,
            aim_sigma: 0.035,
            aim_tau: 0.3,
            aim_spring: 11.0,
            aim_max_speed: 6.0,
            fire_tolerance: 1.4,
            lead: 0.8,
            moves: MoveFlags {
                dodge: true,
                ..MoveFlags::default()
            },
        }
    }
}

/// The four rows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LevelTable {
    pub easy: LevelParams,
    pub normal: LevelParams,
    pub hard: LevelParams,
    pub expert: LevelParams,
}

/// A level table that cannot be read.
#[derive(Debug, thiserror::Error)]
pub enum LevelError {
    #[error("invalid level table: {0}")]
    Ron(#[from] ron::error::SpannedError),
}

impl LevelTable {
    /// Parses a RON table.
    ///
    /// # Errors
    /// [`LevelError::Ron`] on malformed RON.
    pub fn from_ron(text: &str) -> Result<Self, LevelError> {
        Ok(ron::from_str(text)?)
    }

    /// The row of a level.
    #[must_use]
    pub fn get(&self, level: Level) -> &LevelParams {
        match level {
            Level::Easy => &self.easy,
            Level::Normal => &self.normal,
            Level::Hard => &self.hard,
            Level::Expert => &self.expert,
        }
    }
}

impl Default for LevelTable {
    fn default() -> Self {
        default_table().clone()
    }
}

fn default_table() -> &'static LevelTable {
    static TABLE: OnceLock<LevelTable> = OnceLock::new();
    TABLE.get_or_init(|| {
        // The file is part of the crate: a parse failure is a build defect,
        // caught by the unit test below.
        ron::from_str(DEFAULT_LEVELS_RON).expect("data/levels.ron is valid")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_matches_the_plan() {
        let t = LevelTable::from_ron(DEFAULT_LEVELS_RON).unwrap();
        assert_eq!(t.get(Level::Easy).reaction_s, 0.6);
        assert_eq!(t.get(Level::Normal).reaction_s, 0.4);
        assert_eq!(t.get(Level::Hard).reaction_s, 0.25);
        assert_eq!(t.get(Level::Expert).reaction_s, 0.15);
        assert!(!t.easy.moves.dodge && t.normal.moves.dodge);
        assert!(t.hard.moves.slide && !t.hard.moves.slide_hop);
        assert!(t.expert.moves.bunny_hop && t.expert.moves.slide_hop);
        // Error shrinks and speed grows with the level.
        for w in Level::ALL.windows(2) {
            let (a, b) = (t.get(w[0]), t.get(w[1]));
            assert!(a.aim_sigma > b.aim_sigma);
            assert!(a.aim_spring < b.aim_spring);
            assert!(a.reaction_s > b.reaction_s);
        }
    }

    #[test]
    fn table_round_trips_through_ron() {
        let t = LevelTable::default();
        let text = ron::to_string(&t).unwrap();
        assert_eq!(LevelTable::from_ron(&text).unwrap(), t);
    }
}

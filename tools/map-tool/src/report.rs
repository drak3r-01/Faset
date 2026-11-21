//! The validation report: JSON schema `report_version: 1`.

use serde::{Deserialize, Serialize};

/// Current version of the JSON report schema.
pub const REPORT_VERSION: u32 = 1;

/// One rule violation or warning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Issue {
    /// Rule number (1 to 9), as listed in the lot file.
    pub rule: u8,
    /// Map element, e.g. `spawn[3]`, `pickup[1]`, `map`.
    pub element: String,
    /// What is wrong, precisely.
    pub message: String,
}

impl std::fmt::Display for Issue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "rule {} {}: {}", self.rule, self.element, self.message)
    }
}

/// Informative statistics; never errors.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Stats {
    pub triangles: u32,
    pub spawns: u32,
    pub pickups: u32,
    /// Area of the cells walkable from the first spawn, in square metres.
    pub playable_surface_m2: f32,
    /// Longest unobstructed line between two spawns, pickups or objectives.
    pub longest_line_of_sight_m: f32,
    /// Share of walkable cells next to an obstacle (0 to 1).
    pub cover_density: f32,
}

/// Result of validating one map.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub report_version: u32,
    /// BLAKE3 fingerprint of the canonical map (hex); `None` if the file
    /// could not be parsed.
    pub map_id: Option<String>,
    pub errors: Vec<Issue>,
    pub warnings: Vec<Issue>,
    pub stats: Stats,
}

impl Report {
    pub(crate) fn new(map_id: Option<String>) -> Self {
        Self {
            report_version: REPORT_VERSION,
            map_id,
            errors: Vec::new(),
            warnings: Vec::new(),
            stats: Stats::default(),
        }
    }

    /// True when no rule is violated.
    #[must_use]
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }

    /// Whether some error belongs to `rule`.
    #[must_use]
    pub fn has_rule(&self, rule: u8) -> bool {
        self.errors.iter().any(|e| e.rule == rule)
    }

    pub(crate) fn error(&mut self, rule: u8, element: impl Into<String>, msg: impl Into<String>) {
        self.errors.push(Issue {
            rule,
            element: element.into(),
            message: msg.into(),
        });
    }

    pub(crate) fn warn(&mut self, rule: u8, element: impl Into<String>, msg: impl Into<String>) {
        self.warnings.push(Issue {
            rule,
            element: element.into(),
            message: msg.into(),
        });
    }

    /// The report as pretty JSON.
    #[must_use]
    pub fn to_json(&self) -> String {
        // Plain data: serialisation cannot fail.
        serde_json::to_string_pretty(self).expect("report serialises") + "\n"
    }
}

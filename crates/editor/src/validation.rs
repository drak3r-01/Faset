//! Continuous validation: the `map-tool` rules run on the document as it is
//! edited, and clicking an issue moves the camera onto it.
//!
//! The rules are the validator's own (`map_tool::validate`); this module only
//! re-runs them when the document changes and gives each issue a place in
//! the world. Decision: a change is noticed through the document's
//! fingerprint, and the run waits [`DEBOUNCE_S`] after the last change so a
//! handle drag does not validate on every mouse move.

use faset_core::glam::Vec3;
use faset_mapfmt::{
    expand, fingerprint, grid_to_metres, MapFile, PickupKind, Team, Zone, ZoneKind,
};
use map_tool::{Report, Stats};

use crate::camera::EditorCamera;
use crate::element::brush_aabb;

/// Quiet time after the last edit before the rules run again, in seconds.
pub const DEBOUNCE_S: f32 = 0.25;

/// How serious an issue is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// The map breaks a rule and cannot be published.
    Error,
    /// Worth a look, never blocking.
    Warning,
}

/// One entry of the panel.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidationIssue {
    /// Rule number, 1 to 9 (the validator's ids).
    pub rule: u8,
    pub severity: Severity,
    /// Map element, such as `spawn[3]` or `map`.
    pub element: String,
    pub message: String,
    /// Where the camera goes when the issue is clicked.
    pub location: Option<Vec3>,
}

/// The validation results of the document and the logic that keeps them up
/// to date.
#[derive(Debug, Clone, Default)]
pub struct ValidationPanel {
    issues: Vec<ValidationIssue>,
    stats: Stats,
    checked: Option<[u8; 32]>,
    seen: Option<[u8; 32]>,
    quiet_s: f32,
    /// Index of the issue the user clicked last.
    pub selected: Option<usize>,
}

impl ValidationPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The issues: errors first (in rule order), then warnings.
    #[must_use]
    pub fn issues(&self) -> &[ValidationIssue] {
        &self.issues
    }

    /// Informative statistics of the last run: longest line of sight,
    /// playable area, cover density, triangles.
    #[must_use]
    pub fn stats(&self) -> &Stats {
        &self.stats
    }

    /// True when the last run found no error.
    #[must_use]
    pub fn is_ok(&self) -> bool {
        !self.issues.iter().any(|i| i.severity == Severity::Error)
    }

    /// Whether some issue breaks `rule`.
    #[must_use]
    pub fn has_rule(&self, rule: u8) -> bool {
        self.issues
            .iter()
            .any(|i| i.rule == rule && i.severity == Severity::Error)
    }

    /// Whether the results are stale (the document changed since the last
    /// run).
    #[must_use]
    pub fn is_stale(&self) -> bool {
        self.seen != self.checked
    }

    /// Runs every rule on `map` now.
    pub fn refresh(&mut self, map: &MapFile) {
        let fp = fingerprint(map);
        let report = map_tool::validate(map);
        self.set_report(map, &report);
        self.checked = Some(fp);
        self.seen = Some(fp);
        self.quiet_s = 0.0;
    }

    /// Call once per frame with the frame time. Validates when the document
    /// changed and has been still for [`DEBOUNCE_S`] (or has never been
    /// checked). Returns whether it ran.
    pub fn update(&mut self, map: &MapFile, dt: f32) -> bool {
        let fp = fingerprint(map);
        if self.seen == Some(fp) {
            self.quiet_s += dt;
        } else {
            self.seen = Some(fp);
            self.quiet_s = 0.0;
        }
        let first = self.checked.is_none();
        if self.checked != Some(fp) && (first || self.quiet_s >= DEBOUNCE_S) {
            self.refresh(map);
            return true;
        }
        false
    }

    /// The click on issue `index`: selects it and frames its location with
    /// `camera`. Returns the framed point.
    pub fn click(&mut self, index: usize, camera: &mut EditorCamera) -> Option<Vec3> {
        let at = self.issues.get(index)?.location?;
        self.selected = Some(index);
        camera.focus(at);
        Some(at)
    }

    fn set_report(&mut self, map: &MapFile, report: &Report) {
        self.issues = issues_of(report, Some(&expand(map)));
        self.stats = report.stats.clone();
        self.selected = None;
    }
}

fn issues_of(report: &Report, expanded: Option<&MapFile>) -> Vec<ValidationIssue> {
    report
        .errors
        .iter()
        .map(|i| (Severity::Error, i))
        .chain(report.warnings.iter().map(|i| (Severity::Warning, i)))
        .map(|(severity, i)| ValidationIssue {
            rule: i.rule,
            severity,
            element: i.element.clone(),
            message: i.message.clone(),
            location: expanded.and_then(|m| locate(m, &i.element)),
        })
        .collect()
}

/// Validation of a file's bytes, for imports: rules 1 and 2, then all the
/// others when the file parses.
#[must_use]
pub fn validate_bytes(bytes: &[u8]) -> Vec<ValidationIssue> {
    let report = map_tool::validate_bytes(bytes);
    let expanded = faset_mapfmt::parse(bytes).ok().map(|m| expand(&m));
    issues_of(&report, expanded.as_ref())
}

#[allow(clippy::cast_precision_loss)]
fn centroid(points: impl Iterator<Item = Vec3>) -> Option<Vec3> {
    let (sum, n) = points.fold((Vec3::ZERO, 0u32), |(s, n), p| (s + p, n + 1));
    (n > 0).then(|| sum / n as f32)
}

/// `name[3]` to (`name`, 3).
fn indexed(element: &str) -> Option<(&str, usize)> {
    let (name, rest) = element.split_once('[')?;
    let index = rest.strip_suffix(']')?.parse().ok()?;
    Some((name, index))
}

/// The world position of a validator element, in the expanded map.
fn locate(map: &MapFile, element: &str) -> Option<Vec3> {
    if let Some((name, i)) = indexed(element) {
        return match name {
            "spawn" => map.spawns.get(i).map(|s| Vec3::from_array(s.at)),
            "pickup" => map.pickups.get(i).map(|p| Vec3::from_array(p.at)),
            _ => None,
        };
    }
    let spawns = |team: Option<Team>| {
        centroid(
            map.spawns
                .iter()
                .filter(move |s| team.is_none_or(|t| s.team == t))
                .map(|s| Vec3::from_array(s.at)),
        )
    };
    match element {
        "team A" => spawns(Some(Team::A)),
        "team B" => spawns(Some(Team::B)),
        "spawns" => spawns(None),
        "objectives" => map.zones.iter().find_map(|Zone::Box { min, max, kind }| {
            matches!(kind, ZoneKind::Objective { .. })
                .then(|| (grid_to_metres(*min) + grid_to_metres(*max)) * 0.5)
        }),
        "major pickups" => map
            .pickups
            .iter()
            .find(|p| matches!(p.kind, PickupKind::Health | PickupKind::Armor))
            .map(|p| Vec3::from_array(p.at)),
        // `map`, `file`: the middle of the volumes, or the origin.
        _ => Some(map_centre(map)),
    }
}

fn map_centre(map: &MapFile) -> Vec3 {
    let mut lo = Vec3::splat(f32::INFINITY);
    let mut hi = Vec3::splat(f32::NEG_INFINITY);
    for b in &map.brushes {
        let a = brush_aabb(b);
        lo = lo.min(a.min);
        hi = hi.max(a.max);
    }
    if lo.x > hi.x {
        Vec3::ZERO
    } else {
        (lo + hi) * 0.5
    }
}

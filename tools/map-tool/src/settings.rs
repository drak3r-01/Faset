//! Validator thresholds, kept in `settings.ron` next to the crate manifest.

use serde::Deserialize;

/// Thresholds of the validation rules.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename = "Settings", deny_unknown_fields)]
pub struct Settings {
    /// Rule 3: whole-map triangle budget.
    pub max_triangles: u32,
    /// Rule 5: players in a full match; a map needs half per team.
    pub match_players: u8,
    /// Rule 4: free height above a spawn, in metres.
    pub spawn_free_height_m: f32,
    /// Rule 7: radius within which enemy spawns must not see each other.
    pub enemy_sight_radius_m: f32,
    /// Rule 9: largest relative gap between the teams' distances.
    pub balance_tolerance: f32,
    /// Rule 6: highest step (and per-cell rise) of the walk graph, in metres.
    pub step_m: f32,
}

impl Default for Settings {
    fn default() -> Self {
        ron::from_str(include_str!("../settings.ron")).expect("settings.ron is valid")
    }
}

#[cfg(test)]
mod tests {
    use super::Settings;

    #[test]
    fn bundled_settings_parse() {
        let s = Settings::default();
        assert_eq!(s.max_triangles, 150_000);
        assert_eq!(s.match_players, 16);
    }
}

//! The look of a map (lot 6.8): what the map's theme asks of the renderer.
//!
//! A map file names an ambience profile, a sky preset and a weather calendar
//! ([`faset_mapfmt::Theme`]) and may hold point lights and water zones.
//! [`MapLook::of`] turns them into renderer parameters, with no GPU needed;
//! [`MapLook::apply`] writes them into the frame's [`RenderWorld`]. Names the
//! renderer does not know fall back to the `day` profile and a clear sky.
//!
//! Decisions: the weather follows the calendar in steps (no blend between
//! two slots); the sea level is the highest top of the map's `Water` zones;
//! the sky hour comes from the ambience name (a night map shows stars, a
//! sunset map a low sun).

use faset_mapfmt::{grid_to_metres, MapFile, Zone, ZoneKind};
use faset_render::glam::{Vec2, Vec3};
use faset_render::{
    AmbienceProfile, GerstnerWave, PointLight, RenderWorld, SkyParams, WaterParams, WeatherState,
    MAX_POINT_LIGHTS,
};

/// The renderer parameters of one map.
#[derive(Debug, Clone)]
pub struct MapLook {
    pub ambience: AmbienceProfile,
    pub sky: SkyParams,
    /// Weather calendar: `(from_s, state)` ordered by start time.
    pub weather: Vec<(f32, WeatherState)>,
    pub water: Option<WaterParams>,
    pub lights: Vec<PointLight>,
}

/// Hour of the day the sky shows for an ambience profile.
#[must_use]
pub fn sky_hour(ambience: &str) -> f32 {
    match ambience {
        "sunset" => 18.2,
        "night" | "neon" | "space" => 0.5,
        "desert" => 13.0,
        "snow" => 11.0,
        "storm" => 16.0,
        _ => 14.5,
    }
}

/// Cloud cover of a sky preset name.
#[must_use]
pub fn cloud_cover(sky: &str) -> f32 {
    match sky {
        "overcast" => 0.75,
        "dusk" => 0.35,
        "night" | "stars" => 0.05,
        _ => 0.12,
    }
}

/// The weather a calendar name stands for.
#[must_use]
pub fn weather_state(name: &str) -> WeatherState {
    match name {
        "rain" => WeatherState {
            rain: 0.7,
            humidity: 0.8,
            wind: Vec3::new(3.0, 0.0, 1.0),
            ..WeatherState::CLEAR
        },
        "snow" => WeatherState {
            snow: 0.6,
            wind: Vec3::new(1.5, 0.0, 0.5),
            ..WeatherState::CLEAR
        },
        "storm" => WeatherState {
            rain: 1.0,
            humidity: 1.0,
            storm: 0.8,
            wind: Vec3::new(10.0, 0.0, 4.0),
            ..WeatherState::CLEAR
        },
        "sand" => WeatherState {
            sand: 0.6,
            wind: Vec3::new(8.0, 0.0, 2.0),
            ..WeatherState::CLEAR
        },
        _ => WeatherState::CLEAR,
    }
}

/// Gentle waves for a sea or a lagoon.
fn calm_sea(height: f32) -> WaterParams {
    WaterParams {
        height,
        waves: vec![
            GerstnerWave {
                amplitude: 0.10,
                steepness: 0.4,
                direction: Vec2::new(1.0, 0.3),
                wavelength: 14.0,
            },
            GerstnerWave {
                amplitude: 0.05,
                steepness: 0.4,
                direction: Vec2::new(-0.4, 1.0),
                wavelength: 6.0,
            },
        ],
    }
}

impl Default for MapLook {
    /// The look of a map that asks for nothing: day, clear sky.
    fn default() -> Self {
        Self::from_names("day", "clear", &[])
    }
}

impl MapLook {
    fn from_names(ambience: &str, sky: &str, calendar: &[(f32, String)]) -> Self {
        let profile = AmbienceProfile::builtin(ambience);
        let name = if profile.is_some() { ambience } else { "day" };
        let mut weather: Vec<(f32, WeatherState)> = calendar
            .iter()
            .map(|(from, w)| (*from, weather_state(w)))
            .collect();
        weather.sort_by(|a, b| a.0.total_cmp(&b.0));
        Self {
            ambience: profile.unwrap_or_else(|| {
                AmbienceProfile::builtin("day").expect("the day profile is built in")
            }),
            sky: SkyParams {
                time_h: sky_hour(name),
                latitude_deg: 45.0,
                cloud_cover: cloud_cover(sky),
                ..SkyParams::default()
            },
            weather,
            water: None,
            lights: Vec::new(),
        }
    }

    /// The look of an expanded map.
    #[must_use]
    pub fn of(map: &MapFile) -> Self {
        let calendar: Vec<(f32, String)> = map
            .theme
            .weather
            .iter()
            .map(|s| (s.from_s, s.weather.clone()))
            .collect();
        let mut look = Self::from_names(&map.theme.ambience, &map.theme.sky, &calendar);
        // The sea level is the highest top of the water zones.
        let level = map
            .zones
            .iter()
            .filter(|Zone::Box { kind, .. }| matches!(kind, ZoneKind::Water))
            .map(|Zone::Box { max, .. }| grid_to_metres(*max).y)
            .reduce(f32::max);
        look.water = level.map(calm_sea);
        look.lights = map
            .lights
            .iter()
            .take(MAX_POINT_LIGHTS)
            .map(|l| PointLight {
                position: Vec3::from_array(l.at),
                color: Vec3::from_array(l.color) * l.intensity,
                radius: l.range,
            })
            .collect();
        look
    }

    /// The weather `seconds` into the match.
    #[must_use]
    pub fn weather_at(&self, seconds: f32) -> WeatherState {
        self.weather
            .iter()
            .rev()
            .find(|(from, _)| *from <= seconds)
            .map_or(WeatherState::CLEAR, |(_, w)| *w)
    }

    /// Writes the look into a frame: ambience, sky, weather, water, lights.
    /// Call [`RenderWorld::update_underwater`] once the camera is set.
    pub fn apply(&self, world: &mut RenderWorld, seconds: f32) {
        world.ambience = self.ambience;
        world.sky = self.sky;
        world.draw_sky = true;
        world.weather = self.weather_at(seconds);
        world.water = self.water.clone();
        world.lights = self.lights.clone();
        world.update_underwater();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(text: &str) -> MapFile {
        faset_mapfmt::expand(&faset_mapfmt::parse(text.as_bytes()).unwrap())
    }

    const HARBOR: &str = include_str!("../../../assets/maps/rust-harbor.ron");
    const HOLLOW: &str = include_str!("../../../assets/maps/pine-hollow.ron");
    const MESA: &str = include_str!("../../../assets/maps/red-mesa.ron");
    const COVE: &str = include_str!("../../../assets/maps/driftwood-cove.ron");

    #[test]
    fn every_map_theme_names_a_real_profile() {
        for text in [HARBOR, HOLLOW, MESA, COVE] {
            let m = map(text);
            assert!(
                AmbienceProfile::builtin(&m.theme.ambience).is_some(),
                "{}: unknown ambience {}",
                m.meta.name,
                m.theme.ambience
            );
        }
    }

    #[test]
    fn the_harbor_is_a_night_scene_with_lamps_and_a_sea() {
        let look = MapLook::of(&map(HARBOR));
        assert!(look.sky.time_h < 3.0, "a night sky");
        assert!(!look.lights.is_empty() && look.lights.len() <= MAX_POINT_LIGHTS);
        let sea = look.water.expect("the water zone gives a sea");
        assert!((sea.height + 1.0).abs() < 1e-6, "sea level {}", sea.height);
        assert!(sea.validate().is_ok());
    }

    #[test]
    fn the_hollow_snows_and_has_no_water() {
        let look = MapLook::of(&map(HOLLOW));
        assert!(look.water.is_none());
        assert!(look.weather_at(0.0).snow > 0.0);
        assert!(look.weather_at(1000.0).snow > 0.0);
    }

    #[test]
    fn the_calendar_switches_slots_in_order() {
        let look = MapLook::from_names(
            "day",
            "clear",
            &[(300.0, "rain".to_owned()), (0.0, "clear".to_owned())],
        );
        assert!(look.weather_at(10.0).is_clear());
        assert!(look.weather_at(300.0).rain > 0.0);
    }

    #[test]
    fn unknown_names_fall_back_to_the_day_profile() {
        let look = MapLook::from_names("mars", "nope", &[(0.0, "hail".to_owned())]);
        assert_eq!(look.ambience, AmbienceProfile::builtin("day").unwrap());
        assert!(look.weather_at(1.0).is_clear());
    }

    #[test]
    fn apply_fills_the_frame() {
        let look = MapLook::of(&map(COVE));
        let mut world = RenderWorld::default();
        look.apply(&mut world, 5.0);
        assert!(world.draw_sky);
        assert!(world.water.is_none() || world.water.as_ref().is_some_and(|w| w.height < 0.0));
    }
}

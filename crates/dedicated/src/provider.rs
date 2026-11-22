//! The rotation's map provider (lot 5.1): prepares maps of the catalog for
//! the server's worker thread and builds the match of each one.
//!
//! Preparation runs load steps 1, 3 and 4 of the pipeline: read, migrate,
//! symmetry and prefabs (`mapfmt`), collisions (`sim`) and the navigation
//! graph of the bots. Meshes and the height map are client-only.

use std::sync::{Arc, Mutex};

use faset_minigame_api::{
    Match, MatchConfig, ModeEntry, SettingValue, Settings as MatchSettings, TIME_LIMIT_SETTING,
};
use faset_server::{MapProvider, PreparedMap};

use crate::fps_bot::BotMap;
use crate::maps::{Catalog, LoadedMap};
use crate::settings::Settings;

/// What a prepared map carries from the worker thread to the match.
struct Payload {
    loaded: LoadedMap,
    bot_map: Option<Arc<BotMap>>,
}

/// The bot map of the latest [`CatalogMaps::new_match`], shared with the
/// host so it can give the bots their new navigation graph.
pub type SharedBotMap = Arc<Mutex<Option<Arc<BotMap>>>>;

/// The maps of `settings.rotation`, loaded from a [`Catalog`].
pub struct CatalogMaps {
    catalog: Catalog,
    names: Vec<String>,
    seconds: u32,
    entry: Option<ModeEntry>,
    with_bot_map: bool,
    latest_bot_map: SharedBotMap,
}

/// Builds the match of a loaded map.
///
/// # Errors
/// Invalid match settings.
pub fn build_match(
    entry: &ModeEntry,
    settings: &Settings,
    loaded: &LoadedMap,
) -> Result<Match, faset_minigame_api::SettingsError> {
    let mut match_settings = MatchSettings::new();
    match_settings.set(
        TIME_LIMIT_SETTING,
        SettingValue::Int(i32::try_from(settings.rotation_seconds).unwrap_or(i32::MAX)),
    );
    Match::new(
        entry,
        MatchConfig {
            spawn_points: loaded.config.spawn_points.clone(),
            objectives: loaded.objectives.clone(),
            pickups: loaded.pickups.clone(),
            rotation: settings.rotation.clone(),
            settings: match_settings,
            ..MatchConfig::default()
        },
    )
}

impl CatalogMaps {
    /// Maps named by `settings.rotation`, hosting `entry` (if any).
    #[must_use]
    pub fn new(catalog: Catalog, settings: &Settings, entry: Option<ModeEntry>) -> Self {
        Self {
            catalog,
            names: settings.rotation.clone(),
            seconds: settings.rotation_seconds,
            with_bot_map: entry.is_some() && settings.bots > 0,
            entry,
            latest_bot_map: Arc::default(),
        }
    }

    /// Where the bot map of each new match is left.
    #[must_use]
    pub fn bot_map_slot(&self) -> SharedBotMap {
        Arc::clone(&self.latest_bot_map)
    }
}

impl MapProvider for CatalogMaps {
    fn count(&self) -> usize {
        self.names.len()
    }

    fn prepare(&self, index: usize) -> Result<PreparedMap, String> {
        let name = self.names.get(index).ok_or("no such map")?;
        let loaded = self.catalog.load(name)?;
        let bot_map = match (&loaded.file, self.with_bot_map) {
            (Some(file), true) => Some(Arc::new(BotMap::build(file, &loaded.pickups))),
            _ => None,
        };
        Ok(PreparedMap {
            name: name.clone(),
            file: self.catalog.file_bytes(name).unwrap_or_default(),
            config: loaded.config.clone(),
            payload: Some(Box::new(Payload { loaded, bot_map })),
        })
    }

    fn new_match(&self, map: &mut PreparedMap) -> Option<Match> {
        let payload = map.payload.take()?.downcast::<Payload>().ok()?;
        if let Ok(mut slot) = self.latest_bot_map.lock() {
            *slot = payload.bot_map.clone();
        }
        let entry = self.entry.as_ref()?;
        let settings = Settings {
            rotation: self.names.clone(),
            rotation_seconds: self.seconds,
            ..Settings::default()
        };
        match build_match(entry, &settings, &payload.loaded) {
            Ok(m) => Some(m),
            Err(e) => {
                tracing::error!(error = %e, "match settings rejected for the next map");
                None
            }
        }
    }
}

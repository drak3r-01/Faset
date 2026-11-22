//! From the play screen to a running integrated server (lot 6.2).
//!
//! [`host_config`] turns a [`PlayRequest`] into the [`HostConfig`] of
//! [`host_game`](crate::host_game): the server configuration (mode, map,
//! player limit), the hosted match with the mode settings, the map
//! rotation, and the bots.
//!
//! Bots are the dedicated server's `FpsBot` brains (same navigation graph,
//! classes cycling through the catalogue). They exist only for modes that
//! declare bots and only on maps that have a file.

use std::any::Any;
use std::sync::{Arc, Mutex};

use dedicated::fps_bot::{BotMap, FpsBot};
use dedicated::maps::{Catalog, LoadedMap};
use faset_bots::Level;
use faset_minigame_api::{
    Match, MatchConfig, ModeEntry, Registry, SettingValue, Settings, TIME_LIMIT_SETTING,
};
use faset_mode_fps::{class, FpsRules};
use faset_server::{
    Bot, MapProvider, PreparedMap, RotationConfig, RotationOrder, Server, ServerConfig,
};
use faset_ui::PlayRequest;

use crate::hosting::{HostConfig, HostRotation, TickHook};

/// Seconds a map is played in a rotation, and the match length unless the
/// mode settings say otherwise (decision: the dedicated server's default).
pub const ROTATION_SECONDS: u32 = 300;

/// The bot level of the play screen's 0 to 4 slider.
#[must_use]
pub fn bot_level(setting: u8) -> Level {
    match setting {
        0 | 1 => Level::Easy,
        2 => Level::Normal,
        3 => Level::Hard,
        _ => Level::Expert,
    }
}

fn bot_seed(seed: u64, index: usize) -> u64 {
    seed ^ (index as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15)
}

/// What a prepared map carries to the match.
struct Payload {
    loaded: LoadedMap,
    bot_map: Option<Arc<BotMap>>,
}

type SharedBotMap = Arc<Mutex<Option<Arc<BotMap>>>>;

/// The maps of the rotation, loaded from the catalogue on the server's
/// worker thread.
struct HostMaps {
    catalog: Catalog,
    names: Vec<String>,
    entry: Option<ModeEntry>,
    settings: Settings,
    with_bots: bool,
    latest_bot_map: SharedBotMap,
}

impl MapProvider for HostMaps {
    fn count(&self) -> usize {
        self.names.len()
    }

    fn prepare(&self, index: usize) -> Result<PreparedMap, String> {
        let name = self.names.get(index).ok_or("no such map")?;
        let loaded = self.catalog.load(name)?;
        let bot_map = match (&loaded.file, self.with_bots) {
            (Some(file), true) => Some(Arc::new(BotMap::build(file, &loaded.pickups))),
            _ => None,
        };
        Ok(PreparedMap {
            name: name.clone(),
            file: self.catalog.file_bytes(name).unwrap_or_default(),
            config: loaded.config.clone(),
            payload: Some(Box::new(Payload { loaded, bot_map }) as Box<dyn Any + Send>),
        })
    }

    fn new_match(&self, map: &mut PreparedMap) -> Option<Match> {
        let payload = map.payload.take()?.downcast::<Payload>().ok()?;
        if let Ok(mut slot) = self.latest_bot_map.lock() {
            slot.clone_from(&payload.bot_map);
        }
        let entry = self.entry.as_ref()?;
        match build_match(entry, &self.settings, &self.names, &payload.loaded) {
            Ok(m) => Some(m),
            Err(e) => {
                tracing::error!(error = %e, "match settings rejected for the next map");
                None
            }
        }
    }
}

fn build_match(
    entry: &ModeEntry,
    settings: &Settings,
    rotation: &[String],
    loaded: &LoadedMap,
) -> Result<Match, faset_minigame_api::SettingsError> {
    Match::new(
        entry,
        MatchConfig {
            spawn_points: loaded.config.spawn_points.clone(),
            objectives: loaded.objectives.clone(),
            pickups: loaded.pickups.clone(),
            rotation: rotation.to_vec(),
            settings: settings.clone(),
            ..MatchConfig::default()
        },
    )
}

/// Builds the host configuration for `request`.
///
/// `names` are the maps of the play screen (`request.map` indexes them);
/// `catalog` loads them. `seed` seeds the world and the bots.
///
/// # Errors
/// An unknown mode or map, a map without spawn point, or invalid mode
/// settings.
pub fn host_config(
    request: &PlayRequest,
    registry: &Registry,
    catalog: &Catalog,
    names: &[String],
    seed: u64,
) -> Result<HostConfig, String> {
    let entry = if request.mode.is_empty() {
        None
    } else {
        Some(
            registry
                .find(&request.mode)
                .ok_or_else(|| format!("unknown game mode `{}`", request.mode))?
                .clone(),
        )
    };
    let chosen = names.get(request.map).ok_or("no map selected")?;
    let rotation: Vec<String> = if request.rotation {
        names.to_vec()
    } else {
        vec![chosen.clone()]
    };
    let start = if request.rotation { request.map } else { 0 };
    let loaded = catalog.load(&rotation[start])?;

    let mut server = ServerConfig {
        mode: entry.as_ref().map_or(0, |e| e.id.0),
        map: loaded.config.clone(),
        seed,
        ..ServerConfig::default()
    };
    server.limits.max_players = usize::from(request.max_players.max(1));

    let mut settings = request.settings.clone();
    if settings.get(TIME_LIMIT_SETTING).is_none() {
        settings.set(
            TIME_LIMIT_SETTING,
            SettingValue::Int(i32::try_from(ROTATION_SECONDS).unwrap_or(i32::MAX)),
        );
    }
    let wants_bots = request.bots > 0 && entry.as_ref().is_some_and(|e| e.descriptor.bots);
    let provider = HostMaps {
        catalog: catalog.clone(),
        names: rotation.clone(),
        entry: entry.clone(),
        settings: settings.clone(),
        with_bots: wants_bots,
        latest_bot_map: Arc::default(),
    };
    let bot_map_slot = Arc::clone(&provider.latest_bot_map);

    let mut config = HostConfig::new(server);
    config.map_name.clone_from(&rotation[start]);
    if let Some(entry) = &entry {
        config.name = format!("Faset {}", entry.descriptor.name);
        // Validate now so a bad setting is reported to the player; the match
        // itself is built on the server thread (it is not `Send`).
        build_match(entry, &settings, &rotation, &loaded).map_err(|e| e.to_string())?;
        let names_in_play = rotation.clone();
        let (entry, settings, rotation, loaded) =
            (entry.clone(), settings.clone(), rotation, loaded.clone());
        config.game = Some(Box::new(move || {
            build_match(&entry, &settings, &rotation, &loaded).ok()
        }));
        config.rotation = Some(HostRotation {
            config: RotationConfig {
                rotation_seconds: ROTATION_SECONDS,
                order: RotationOrder::Sequential,
                seed,
                ..RotationConfig::default()
            },
            provider: Arc::new(provider),
            current: start,
        });
        config.map_names = names_in_play;
    }

    if wants_bots {
        let room = usize::from(request.max_players).saturating_sub(1);
        let count = usize::from(request.bots).min(room);
        match &loaded.file {
            Some(file) => {
                let map = Arc::new(BotMap::build(file, &loaded.pickups));
                let rules = Arc::new(FpsRules::from_catalog(&class::Catalog::builtin()));
                let level = bot_level(request.bot_level);
                config.bots = (0..count)
                    .map(|i| {
                        Box::new(FpsBot::new(level, bot_seed(seed, i), map.clone(), &rules, i))
                            as Box<dyn Bot>
                    })
                    .collect();
                config.tick_hook = Some(rebrain_hook(bot_map_slot, rules, level, seed));
            }
            None => tracing::warn!("this map has no file, so no bot can navigate it"),
        }
    }
    Ok(config)
}

/// After a map change the bots get brains on the new navigation graph.
fn rebrain_hook(slot: SharedBotMap, rules: Arc<FpsRules>, level: Level, seed: u64) -> TickHook {
    let mut generation = 0;
    Box::new(move |server: &mut Server| {
        if server.map_generation() == generation {
            return;
        }
        generation = server.map_generation();
        let Some(map) = slot.lock().ok().and_then(|m| m.clone()) else {
            return;
        };
        let bots: Vec<_> = server.players().filter(|&p| server.is_bot(p)).collect();
        for (i, id) in bots.into_iter().enumerate() {
            let brain = FpsBot::new(level, bot_seed(seed, i), map.clone(), &rules, i);
            server.set_bot_brain(id, Box::new(brain));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_slider_covers_every_level() {
        let levels: Vec<Level> = (0..=4).map(bot_level).collect();
        for l in [Level::Easy, Level::Normal, Level::Hard, Level::Expert] {
            assert!(levels.contains(&l), "{l:?} unreachable");
        }
    }
}

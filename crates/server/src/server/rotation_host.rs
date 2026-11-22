//! The map rotation as the server runs it (lot 5.1): preparation on a
//! worker thread, map switch, map download service and warm-up.

use std::collections::{BTreeSet, HashMap};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;

use super::*;
use crate::rotation::{MapProvider, PreparedMap, Rotation, RotationAction, RotationConfig};
use crate::transfer::{chunk_messages, compress_map, parse_request};

/// A map prepared on the worker thread with its transfer chunks.
type Prepared = (PreparedMap, Vec<Message>);

/// Everything the rotation keeps beside its clock.
pub(super) struct RotationState {
    pub(super) clock: Rotation,
    config: RotationConfig,
    provider: Arc<dyn MapProvider>,
    /// The worker preparing a map, and which.
    worker: Option<(usize, Receiver<Result<Prepared, String>>)>,
    /// The prepared next map.
    prepared: Option<(usize, Prepared)>,
    /// Compressed chunks of the maps clients may ask for, by fingerprint.
    published: HashMap<u64, Arc<Vec<Message>>>,
    /// Match for the new map, hosted when warm-up starts.
    pending_game: Option<Option<Match>>,
    /// Clients that reported ready since the last `MapChange`.
    pub(super) ready: BTreeSet<(usize, PeerId)>,
}

/// The transfer chunks of a prepared map; `None` for an embedded map.
fn publishable(map: &PreparedMap) -> Option<Vec<Message>> {
    if map.file.is_empty() {
        return None;
    }
    match compress_map(&map.file).and_then(|c| chunk_messages(&c)) {
        Ok(chunks) => Some(chunks),
        Err(e) => {
            tracing::warn!(map = %map.name, error = %e, "map cannot be transferred");
            None
        }
    }
}

/// Worker body: load the map and compress it for transfer.
fn prepare_with_chunks(provider: &dyn MapProvider, index: usize) -> Result<Prepared, String> {
    let map = provider.prepare(index)?;
    let chunks = publishable(&map).unwrap_or_default();
    Ok((map, chunks))
}

impl Server {
    /// Configures the map rotation (lot 5.1) over the maps of `provider`,
    /// the server running map `current`. The current map's file is
    /// published so clients lacking it can download it.
    ///
    /// # Panics
    /// If the provider has no map or `current` is out of range.
    pub fn set_rotation(
        &mut self,
        config: RotationConfig,
        provider: Arc<dyn MapProvider>,
        current: usize,
    ) {
        let count = provider.count();
        let mut state = RotationState {
            clock: Rotation::new(config, count, current),
            config,
            provider,
            worker: None,
            prepared: None,
            published: HashMap::new(),
            pending_game: None,
            ready: BTreeSet::new(),
        };
        match state.provider.prepare(current) {
            Ok(p) => {
                if let Some(chunks) = publishable(&p) {
                    state
                        .published
                        .insert(p.config.fingerprint, Arc::new(chunks));
                }
            }
            Err(e) => tracing::warn!(error = %e, "current map cannot be published"),
        }
        self.rotation = Some(state);
    }

    /// How many times the map changed; a host watches it to refresh what
    /// depends on the map (bot brains).
    #[must_use]
    pub fn map_generation(&self) -> u32 {
        self.map_generation
    }

    /// Index of the map in play in the rotation, if one is configured.
    #[must_use]
    pub fn rotation_current(&self) -> Option<usize> {
        self.rotation.as_ref().map(|r| r.clock.current())
    }

    /// Replaces the brain of a bot (a new map needs a new navigation graph).
    /// Returns whether `bot` exists.
    pub fn set_bot_brain(&mut self, bot: NetId, brain: Box<dyn Bot>) -> bool {
        match self.bots.get_mut(&bot) {
            Some(b) => {
                *b = brain;
                true
            }
            None => false,
        }
    }

    /// Answers a map download request (channel 3 chunks).
    pub(super) fn serve_map(&mut self, conn: (usize, PeerId), msg: &Message) {
        let Some(fingerprint) = parse_request(msg) else {
            return;
        };
        let chunks = self
            .rotation
            .as_ref()
            .and_then(|r| r.published.get(&fingerprint).cloned());
        let Some(chunks) = chunks else {
            tracing::info!(
                fingerprint = format_args!("{fingerprint:016x}"),
                "map request for an unknown map"
            );
            return;
        };
        for chunk in chunks.iter() {
            self.send(conn, Channel::MapTransfer, chunk);
        }
    }

    /// One tick of the rotation clock and its actions.
    pub(super) fn run_rotation(&mut self) {
        let Some(r) = self.rotation.as_mut() else {
            return;
        };
        if let Some((index, rx)) = r.worker.as_ref() {
            if let Ok(done) = rx.try_recv() {
                let index = *index;
                r.worker = None;
                match done {
                    Ok(p) => r.prepared = Some((index, p)),
                    Err(e) => tracing::error!(map = index, error = %e, "map preparation failed"),
                }
            }
        }
        let all_ready = r.clock.waiting_ready()
            && self
                .clients
                .iter()
                .filter(|(_, c)| c.phase == Phase::InGame && c.player.is_some())
                .all(|(k, _)| r.ready.contains(k));
        let scoreboard = r.config.scoreboard_seconds * u32::from(TICK_RATE);
        match r.clock.tick(all_ready) {
            None => {}
            Some(RotationAction::Prepare(index)) => {
                let provider = Arc::clone(&r.provider);
                let (tx, rx) = mpsc::channel();
                r.worker = Some((index, rx));
                std::thread::spawn(move || {
                    let _ = tx.send(prepare_with_chunks(provider.as_ref(), index));
                });
            }
            Some(RotationAction::Scoreboard) => {
                self.broadcast_event(&Event::Phase {
                    phase: faset_minigame_api::Phase::RoundEnd.to_u8(),
                    ticks_remaining: scoreboard,
                });
            }
            Some(RotationAction::Switch(index)) => self.switch_map(index),
            Some(RotationAction::Warmup) => self.start_warmup(),
        }
    }

    /// Switches to the prepared map `index` and announces it.
    fn switch_map(&mut self, index: usize) {
        let Some(r) = self.rotation.as_mut() else {
            return;
        };
        let old = r.clock.current();
        let prepared = match r.prepared.take() {
            Some((i, p)) if i == index => Ok(p),
            _ => match r.worker.take() {
                // The worker is still at it: wait for it.
                Some((i, rx)) if i == index => rx.recv().map_err(|e| e.to_string()).and_then(|x| x),
                _ => prepare_with_chunks(r.provider.as_ref(), index),
            },
        };
        let (mut map, chunks) = match prepared {
            Ok(p) => p,
            Err(e) => {
                tracing::error!(map = index, error = %e, "cannot switch map, keeping the current one");
                r.clock.abort_switch(old);
                return;
            }
        };
        let fingerprint = map.config.fingerprint;
        r.pending_game = Some(r.provider.new_match(&mut map));
        r.ready.clear();
        let keep = self.config.map.fingerprint;
        r.published.retain(|k, _| *k == keep);
        if !chunks.is_empty() {
            r.published.insert(fingerprint, Arc::new(chunks));
        }
        tracing::info!(map = %map.name, fingerprint = format_args!("{fingerprint:016x}"), "map change");
        self.install_map(map.config);
        self.map_generation += 1;
        self.broadcast_event(&Event::MapChange {
            map_fingerprint: fingerprint,
        });
    }

    /// Builds a world on `map` at the same tick and puts every player on a
    /// spawn point of it. Decision: the world is rebuilt (the sim has no
    /// way to swap the statics) and stepped, empty, up to the old tick so
    /// ticks stay continuous for commands and snapshots.
    fn install_map(&mut self, map: crate::config::MapConfig) {
        let tick = self.world.tick();
        self.config.map = map;
        let mut world = World::with_controller(
            self.config.seed,
            self.config.map.statics.clone(),
            self.config.move_params,
        );
        world.set_zones(self.config.map.zones.clone());
        while world.tick() < tick {
            world.step(&[]);
        }
        self.world = world;
        self.services = ModeServices::default();
        let spawns = self.config.map.spawn_points.clone();
        for (id, p) in &mut self.players {
            let at = spawns[usize::from(id.0) % spawns.len()];
            self.world.spawn_player(*id, at);
            p.last_position = at;
            p.velocity = Vec3::ZERO;
        }
        for c in self.clients.values_mut() {
            c.snapshots = SnapshotEncoder::new();
        }
    }

    /// Hosts the match of the new map; the warm-up starts.
    fn start_warmup(&mut self) {
        let Some(pending) = self.rotation.as_mut().and_then(|r| r.pending_game.take()) else {
            return;
        };
        self.game = pending;
        if let Some(game) = self.game.as_mut() {
            for &id in self.players.keys() {
                game.player_joined(&self.world, id);
            }
        }
        self.sent_state = None;
    }
}

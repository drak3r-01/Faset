use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::Arc;

use faset_core::glam::Vec3;
use kira::backend::Backend;
use kira::effect::filter::{FilterBuilder, FilterMode};
use kira::sound::static_sound::{StaticSoundData, StaticSoundHandle, StaticSoundSettings};
use kira::sound::PlaybackState;
use kira::track::{MainTrackBuilder, TrackBuilder, TrackHandle};
use kira::{
    AudioManager, AudioManagerSettings, Capacities, Decibels, DefaultBackend, Frame, Tween,
};
use serde::{Deserialize, Serialize};

use crate::recipe::{synthesise, Recipe};
use crate::spatial::{spatialise, SpatialConfig};
use crate::Position3D;

/// Maximum number of simultaneous voices.
pub const MAX_VOICES: usize = 32;

/// Sample rate used to synthesise recipes at load.
const SYNTH_RATE: u32 = 44_100;

/// Mixer buses. `Music` is reserved (music is an open point of the plan).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Bus {
    General,
    Effects,
    Interface,
    Music,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("audio backend error: {0}")]
    Backend(String),
    #[error("invalid recipe: {0}")]
    Recipe(#[from] ron::error::SpannedError),
    #[error("unknown recipe `{0}`")]
    UnknownRecipe(String),
    #[error("playback error: {0}")]
    Play(String),
    #[error("audio resource limit reached")]
    ResourceLimit,
}

/// Engine configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// Output device name, `None` for the system default. Selecting a device
    /// is wired up by the settings screen (lot 6.4); the value is kept here.
    pub output_device: Option<String>,
    pub general_volume: f32,
    pub effects_volume: f32,
    pub interface_volume: f32,
    /// Volume of the typing/key sound, 0..=1.
    pub key_sound_volume: f32,
    pub spatial: SpatialConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            output_device: None,
            general_volume: 1.0,
            effects_volume: 1.0,
            interface_volume: 1.0,
            key_sound_volume: 1.0,
            spatial: SpatialConfig::default(),
        }
    }
}

/// Opaque handle to a playing voice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VoiceId(pub u64);

/// Read-only view of an active voice.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VoiceInfo {
    pub id: VoiceId,
    pub priority: u8,
    pub gain: f32,
    pub pan: f32,
    pub lowpass_hz: Option<f64>,
}

struct Voice {
    info: VoiceInfo,
    handle: StaticSoundHandle,
    _track: TrackHandle,
}

fn to_db(linear: f32) -> Decibels {
    if linear <= 0.001 {
        Decibels::SILENCE
    } else {
        Decibels(20.0 * linear.log10())
    }
}

type Occluder = Box<dyn Fn(Vec3, Vec3) -> bool + Send>;

pub struct Engine<B: Backend = DefaultBackend> {
    _manager: AudioManager<B>,
    general: TrackHandle,
    effects: TrackHandle,
    interface: TrackHandle,
    music: TrackHandle,
    recipes: HashMap<String, (Recipe, StaticSoundData)>,
    voices: Vec<Voice>,
    next_id: u64,
    config: Config,
    listener: (Vec3, Vec3, Vec3),
    underwater: bool,
    occluder: Option<Occluder>,
}

impl Engine<DefaultBackend> {
    /// Creates an engine on kira's default backend (silent mock backend until the
    /// cpal device feature is not enabled yet).
    pub fn new(config: Config) -> Result<Self, Error>
    where
        <DefaultBackend as Backend>::Settings: Default,
        <DefaultBackend as Backend>::Error: Debug,
    {
        Self::with_backend(config, Default::default())
    }
}

impl<B: Backend> Engine<B>
where
    B::Error: Debug,
{
    /// Creates an engine on an explicit backend (tests use the mock backend).
    pub fn with_backend(config: Config, backend_settings: B::Settings) -> Result<Self, Error> {
        let mut manager = AudioManager::<B>::new(AudioManagerSettings {
            capacities: Capacities {
                sub_track_capacity: 256,
                ..Default::default()
            },
            backend_settings,
            main_track_builder: MainTrackBuilder::new(),
            internal_buffer_size: 128,
        })
        .map_err(|e| Error::Backend(format!("{e:?}")))?;
        let mut general = manager
            .add_sub_track(TrackBuilder::new().volume(to_db(config.general_volume)))
            .map_err(|_| Error::ResourceLimit)?;
        let sub = |t: &mut TrackHandle, v: f32| {
            t.add_sub_track(TrackBuilder::new().volume(to_db(v)))
                .map_err(|_| Error::ResourceLimit)
        };
        let effects = sub(&mut general, config.effects_volume)?;
        let interface = sub(&mut general, config.interface_volume)?;
        let music = sub(&mut general, 1.0)?;
        Ok(Self {
            _manager: manager,
            general,
            effects,
            interface,
            music,
            recipes: HashMap::new(),
            voices: Vec::new(),
            next_id: 0,
            config,
            listener: (Vec3::ZERO, Vec3::NEG_Z, Vec3::Y),
            underwater: false,
            occluder: None,
        })
    }

    /// Synthesises a recipe and registers it under its id.
    pub fn add_recipe(&mut self, recipe: Recipe) {
        let samples = synthesise(&recipe, SYNTH_RATE);
        let frames: Arc<[Frame]> = samples.iter().map(|&s| Frame::from_mono(s)).collect();
        let data = StaticSoundData {
            sample_rate: SYNTH_RATE,
            frames,
            settings: StaticSoundSettings::new(),
            slice: None,
        };
        self.recipes.insert(recipe.id.clone(), (recipe, data));
    }

    /// Parses a RON recipe and registers it.
    pub fn add_recipe_ron(&mut self, text: &str) -> Result<(), Error> {
        self.add_recipe(Recipe::from_ron(text)?);
        Ok(())
    }

    pub fn set_listener(&mut self, position: Position3D, forward: Vec3, up: Vec3) {
        self.listener = (position, forward, up);
    }

    pub fn set_underwater(&mut self, underwater: bool) {
        self.underwater = underwater;
    }

    /// Sets the occlusion test: `(listener, source) -> hidden by a wall`.
    pub fn set_occluder(&mut self, f: impl Fn(Vec3, Vec3) -> bool + Send + 'static) {
        self.occluder = Some(Box::new(f));
    }

    /// Sets a bus volume (linear, 0..=1).
    pub fn set_bus_volume(&mut self, bus: Bus, value: f32) {
        let v = value.clamp(0.0, 1.0);
        let t = Tween::default();
        match bus {
            Bus::General => {
                self.config.general_volume = v;
                self.general.set_volume(to_db(v), t);
            }
            Bus::Effects => {
                self.config.effects_volume = v;
                self.effects.set_volume(to_db(v), t);
            }
            Bus::Interface => {
                self.config.interface_volume = v;
                self.interface.set_volume(to_db(v), t);
            }
            Bus::Music => self.music.set_volume(to_db(v), t),
        }
    }

    pub fn set_key_sound_volume(&mut self, value: f32) {
        self.config.key_sound_volume = value.clamp(0.0, 1.0);
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Active voices, oldest first.
    pub fn voices(&self) -> Vec<VoiceInfo> {
        self.voices.iter().map(|v| v.info).collect()
    }

    /// Plays a spatialised effect. Returns `None` when the voice limit is hit
    /// and every active voice has a higher priority than this one.
    pub fn play(
        &mut self,
        recipe_id: &str,
        position: Position3D,
        priority: u8,
    ) -> Result<Option<VoiceId>, Error> {
        let (l, f, u) = self.listener;
        let occluded = self.occluder.as_ref().is_some_and(|o| o(l, position));
        let sp = spatialise(
            l,
            f,
            u,
            position,
            occluded,
            self.underwater,
            self.config.spatial,
        );
        self.start(recipe_id, priority, sp.gain, sp.pan, sp.lowpass_hz)
    }

    /// Plays a non-spatial interface sound (full volume on its recipe bus).
    pub fn play_ui(&mut self, recipe_id: &str, priority: u8) -> Result<Option<VoiceId>, Error> {
        self.start(recipe_id, priority, 1.0, 0.0, None)
    }

    /// Plays the recipe `key_sound` (if registered) scaled by the key-sound volume.
    pub fn play_key_sound(&mut self) -> Result<Option<VoiceId>, Error> {
        if !self.recipes.contains_key("key_sound") {
            return Ok(None);
        }
        let g = self.config.key_sound_volume;
        self.start("key_sound", 0, g, 0.0, None)
    }

    fn start(
        &mut self,
        recipe_id: &str,
        priority: u8,
        gain: f32,
        pan: f32,
        lowpass_hz: Option<f64>,
    ) -> Result<Option<VoiceId>, Error> {
        let (recipe, data) = self
            .recipes
            .get(recipe_id)
            .ok_or_else(|| Error::UnknownRecipe(recipe_id.to_owned()))?;
        let bus = recipe.bus;
        let data = data.clone();
        let amp = recipe.volume;

        self.voices
            .retain(|v| v.handle.state() != PlaybackState::Stopped);
        if self.voices.len() >= MAX_VOICES {
            // Steal the lowest priority; among equals, the oldest (lowest id).
            let (idx, victim) = self
                .voices
                .iter()
                .enumerate()
                .min_by_key(|(_, v)| (v.info.priority, v.info.id.0))
                .expect("voices is non-empty");
            if victim.info.priority > priority {
                tracing::debug!(recipe_id, priority, "voice rejected: all voices outrank it");
                return Ok(None);
            }
            let mut v = self.voices.remove(idx);
            v.handle.stop(Tween::default());
        }

        let parent = match bus {
            Bus::General => &mut self.general,
            Bus::Effects => &mut self.effects,
            Bus::Interface => &mut self.interface,
            Bus::Music => &mut self.music,
        };
        let mut tb = TrackBuilder::new().persist_until_sounds_finish(true);
        if let Some(hz) = lowpass_hz {
            tb = tb.with_effect(FilterBuilder::new().mode(FilterMode::LowPass).cutoff(hz));
        }
        let mut track = parent.add_sub_track(tb).map_err(|_| Error::ResourceLimit)?;
        let handle = track
            .play(data.volume(to_db(gain * amp)).panning(pan))
            .map_err(|e| Error::Play(format!("{e:?}")))?;

        let id = VoiceId(self.next_id);
        self.next_id += 1;
        self.voices.push(Voice {
            info: VoiceInfo {
                id,
                priority,
                gain,
                pan,
                lowpass_hz,
            },
            handle,
            _track: track,
        });
        Ok(Some(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipe::Waveform;
    use crate::spatial::attenuation;
    use kira::backend::mock::{MockBackend, MockBackendSettings};

    fn engine() -> Engine<MockBackend> {
        let mut e = Engine::<MockBackend>::with_backend(
            Config::default(),
            MockBackendSettings {
                sample_rate: 44_100,
            },
        )
        .unwrap();
        e.add_recipe(Recipe {
            id: "beep".into(),
            duration: 5.0,
            ..Default::default()
        });
        e
    }

    #[test]
    fn pan_follows_right_axis() {
        let mut e = engine();
        e.set_listener(Vec3::ZERO, Vec3::NEG_Z, Vec3::Y);
        let r = e
            .play("beep", Vec3::new(5.0, 0.0, 0.0), 1)
            .unwrap()
            .unwrap();
        let l = e
            .play("beep", Vec3::new(-5.0, 0.0, 0.0), 1)
            .unwrap()
            .unwrap();
        let info = e.voices();
        assert!(info.iter().find(|v| v.id == r).unwrap().pan > 0.5);
        assert!(info.iter().find(|v| v.id == l).unwrap().pan < -0.5);
    }

    #[test]
    fn attenuation_is_monotonic() {
        let mut prev = 1.0;
        for i in 0..200 {
            let g = attenuation(i as f32 * 0.5, 2.0, 1.0);
            assert!(g <= prev && (0.0..=1.0).contains(&g));
            prev = g;
        }
    }

    #[test]
    fn forty_sounds_keep_32_highest_priority() {
        let mut e = engine();
        for i in 0..40u8 {
            e.play("beep", Vec3::new(0.0, 0.0, -3.0), i).unwrap();
        }
        let v = e.voices();
        assert_eq!(v.len(), 32);
        assert!(v.iter().all(|v| v.priority >= 8));
    }

    #[test]
    fn equal_priority_steals_oldest() {
        let mut e = engine();
        let first = e.play("beep", Vec3::ZERO, 1).unwrap().unwrap();
        for _ in 0..MAX_VOICES {
            e.play("beep", Vec3::ZERO, 1).unwrap();
        }
        let v = e.voices();
        assert_eq!(v.len(), 32);
        assert!(v.iter().all(|v| v.id != first));
    }

    #[test]
    fn low_priority_is_rejected_when_full() {
        let mut e = engine();
        for _ in 0..MAX_VOICES {
            e.play("beep", Vec3::ZERO, 5).unwrap();
        }
        assert_eq!(e.play("beep", Vec3::ZERO, 1).unwrap(), None);
        assert_eq!(e.voices().len(), 32);
    }

    #[test]
    fn lowpass_when_occluded_or_underwater() {
        let mut e = engine();
        let p = Vec3::new(0.0, 0.0, -4.0);
        let a = e.play("beep", p, 1).unwrap().unwrap();
        e.set_occluder(|_, _| true);
        let b = e.play("beep", p, 1).unwrap().unwrap();
        e.set_occluder(|_, _| false);
        e.set_underwater(true);
        let c = e.play("beep", p, 1).unwrap().unwrap();
        let v = e.voices();
        let get = |id| v.iter().find(|v| v.id == id).unwrap().lowpass_hz;
        assert_eq!(get(a), None);
        assert!(get(b).is_some());
        assert!(get(c).unwrap() < get(b).unwrap());
    }

    #[test]
    fn unknown_recipe_errors() {
        let mut e = engine();
        assert!(matches!(
            e.play("nope", Vec3::ZERO, 0),
            Err(Error::UnknownRecipe(_))
        ));
    }

    #[test]
    fn synthesis_length_and_determinism() {
        let r = Recipe {
            id: "n".into(),
            waveform: Waveform::Saw,
            pitch_end: Some(220.0),
            duration: 0.25,
            noise_mix: 0.3,
            ..Default::default()
        };
        let a = synthesise(&r, 48_000);
        assert_eq!(a.len(), 12_000);
        assert_eq!(a, synthesise(&r, 48_000));
        assert!(a.iter().all(|s| s.abs() <= 1.0));
    }

    #[test]
    fn recipe_parses_from_ron() {
        let r = Recipe::from_ron(
            r#"(id: "step", waveform: Noise, duration: 0.1, envelope: (attack: 0.01), bus: Effects)"#,
        )
        .unwrap();
        assert_eq!(r.id, "step");
        assert_eq!(r.waveform, Waveform::Noise);
        let mut e = engine();
        e.add_recipe(r);
        assert!(e.play("step", Vec3::ZERO, 0).unwrap().is_some());
    }
}

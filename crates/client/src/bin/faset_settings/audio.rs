//! Applies the audio settings to the audio engine without a restart
//! (lot 6.5): bus volumes and the key sound change at once; the output device
//! is a new engine built from [`audio_config`].

use std::fmt::Debug;

use faset_audio::{Bus, Config, Engine};
use faset_core::settings::Settings;
use kira::backend::Backend;

/// The engine configuration the settings ask for.
///
/// Decision: `output_device` 0 is the system default; any other number is
/// handed to the engine as the device name `"<n>"`, because the audio crate
/// does not list devices yet.
pub fn audio_config(settings: &Settings) -> Config {
    let a = &settings.audio;
    Config {
        output_device: (a.output_device != 0).then(|| a.output_device.to_string()),
        general_volume: a.master,
        effects_volume: a.effects,
        interface_volume: a.ui,
        key_sound_volume: if a.key_sound { 1.0 } else { 0.0 },
        ..Config::default()
    }
}

/// Applies the volume or key-sound setting `id`. Returns `None` for ids that
/// are not handled here (the output device and the other categories).
pub fn apply_volume<B: Backend>(
    engine: &mut Engine<B>,
    id: &str,
    settings: &Settings,
) -> Option<bool>
where
    B::Error: Debug,
{
    let a = &settings.audio;
    match id {
        "master_volume" => engine.set_bus_volume(Bus::General, a.master),
        "effects_volume" => engine.set_bus_volume(Bus::Effects, a.effects),
        "ui_volume" => engine.set_bus_volume(Bus::Interface, a.ui),
        "key_sound" => engine.set_key_sound_volume(audio_config(settings).key_sound_volume),
        _ => return None,
    }
    Some(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use faset_core::settings::{apply_settings, SettingMeta, SettingsSink, Value};
    use kira::backend::mock::{MockBackend, MockBackendSettings};

    struct Sink(Engine<MockBackend>);
    impl SettingsSink for Sink {
        fn apply(&mut self, meta: &SettingMeta, settings: &Settings) -> bool {
            apply_volume(&mut self.0, meta.id, settings).unwrap_or(false)
        }
    }

    #[test]
    fn volumes_and_key_sound_apply_without_restart() {
        let engine = Engine::<MockBackend>::with_backend(
            Config::default(),
            MockBackendSettings {
                sample_rate: 44_100,
            },
        )
        .unwrap();
        let mut sink = Sink(engine);
        let old = Settings::default();
        let mut new = old.clone();
        for (id, v) in [
            ("master_volume", Value::Float(0.5)),
            ("effects_volume", Value::Float(0.25)),
            ("ui_volume", Value::Float(0.75)),
            ("key_sound", Value::Bool(false)),
        ] {
            new.set(id, v).unwrap();
        }
        let applied = apply_settings(&old, &new, &mut sink);
        assert_eq!(
            applied,
            vec!["master_volume", "effects_volume", "ui_volume", "key_sound"]
        );
        let c = sink.0.config();
        assert_eq!(c.general_volume, 0.5);
        assert_eq!(c.effects_volume, 0.25);
        assert_eq!(c.interface_volume, 0.75);
        assert_eq!(c.key_sound_volume, 0.0);
    }

    #[test]
    fn output_device_maps_to_the_engine_config() {
        let mut s = Settings::default();
        assert_eq!(audio_config(&s).output_device, None);
        s.set("output_device", Value::Int(2)).unwrap();
        assert_eq!(audio_config(&s).output_device.as_deref(), Some("2"));
    }
}

//! The sounds of the game (lot 6.9): shots, reloads, impacts per surface,
//! footsteps, hit and kill ticks, damage and explosions.
//!
//! Decision: no audio file is shipped. Every sound is a [`Recipe`] of
//! `faset-audio` (a waveform, a glide, an envelope and some noise) built in
//! code and synthesised by the engine when it is registered, so the sounds go
//! through the same buses as everything else and follow the master and
//! effects volumes of the settings. The audio backend is the settings lot's
//! kira engine; no crate was added.

use std::fmt::Debug;

use faset_audio::{Bus, Engine, Envelope, Recipe, Waveform};
use faset_core::glam::Vec3;
use kira::backend::Backend;

use crate::surface::Surface;

/// Something the game makes a sound for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cue {
    /// The local player fired a weapon; the recipe is [`shot_recipe`] of its
    /// id.
    Shot(&'static str),
    /// A magazine was refilled.
    Reload,
    /// A bullet landed on a surface.
    Impact(Surface),
    /// A footstep on a surface.
    Footstep(Surface),
    /// The local player's shot hurt someone.
    Hit,
    /// The local player's shot killed someone.
    Kill,
    /// The local player was hurt.
    Hurt,
    /// A blast.
    Explosion,
}

/// The recipe of the shot of a weapon: a few families, picked by weapon id.
#[must_use]
pub fn shot_recipe(weapon: &str) -> &'static str {
    match weapon {
        "smg" => "shot_smg",
        "pump_shotgun" => "shot_shotgun",
        "precision_rifle" => "shot_precision",
        "sidearm" => "shot_sidearm",
        "boomer_rocket_launcher" => "shot_launcher",
        "baguette" => "shot_swing",
        "toaster"
        | "banana_peel"
        | "bubble_gun"
        | "confetti_cannon"
        | "rubber_chicken_launcher" => "shot_goofy",
        _ => "shot_rifle",
    }
}

impl Cue {
    /// Id of the recipe that makes this sound.
    #[must_use]
    pub fn recipe(self) -> &'static str {
        match self {
            Self::Shot(recipe) => recipe,
            Self::Reload => "reload",
            Self::Impact(s) => match s {
                Surface::Metal => "impact_metal",
                Surface::Concrete => "impact_concrete",
                Surface::Stone => "impact_stone",
                Surface::Sand => "impact_sand",
                Surface::Wood => "impact_wood",
                Surface::Water => "impact_water",
                Surface::Snow => "impact_snow",
            },
            Self::Footstep(s) => match s {
                Surface::Metal => "footstep_metal",
                Surface::Concrete => "footstep_concrete",
                Surface::Stone => "footstep_stone",
                Surface::Sand => "footstep_sand",
                Surface::Wood => "footstep_wood",
                Surface::Water => "footstep_water",
                Surface::Snow => "footstep_snow",
            },
            Self::Hit => "hit",
            Self::Kill => "kill",
            Self::Hurt => "hurt",
            Self::Explosion => "explosion",
        }
    }

    /// Voice priority: when the engine is full, higher wins.
    #[must_use]
    pub fn priority(self) -> u8 {
        match self {
            Self::Kill | Self::Hurt => 9,
            Self::Hit | Self::Explosion => 8,
            Self::Shot(_) | Self::Reload => 7,
            Self::Impact(_) => 4,
            Self::Footstep(_) => 2,
        }
    }
}

/// `(waveform, start Hz, end Hz, seconds, (attack, decay, sustain, release),
/// noise, volume)`.
type Tone = (Waveform, f32, f32, f32, (f32, f32, f32, f32), f32, f32);

fn recipe(id: &str, tone: Tone) -> Recipe {
    let (waveform, frequency, pitch_end, duration, envelope, noise_mix, volume) = tone;
    Recipe {
        id: id.to_owned(),
        waveform,
        frequency,
        pitch_end: Some(pitch_end),
        duration,
        envelope: Envelope {
            attack: envelope.0,
            decay: envelope.1,
            sustain: envelope.2,
            release: envelope.3,
        },
        noise_mix,
        // Each sound has its own noise.
        noise_seed: id.bytes().fold(2_166_136_261u32, |h, b| {
            (h ^ u32::from(b)).wrapping_mul(16_777_619)
        }),
        volume,
        bus: Bus::Effects,
    }
}

/// The surface sounds: `(surface, waveform, start Hz, end Hz, seconds,
/// noise, volume)`.
const SURFACES: [(Surface, Waveform, f32, f32, f32, f32, f32); 7] = [
    (
        Surface::Metal,
        Waveform::Square,
        2400.0,
        900.0,
        0.09,
        0.35,
        0.45,
    ),
    (
        Surface::Concrete,
        Waveform::Triangle,
        320.0,
        120.0,
        0.08,
        0.7,
        0.5,
    ),
    (
        Surface::Stone,
        Waveform::Noise,
        200.0,
        100.0,
        0.11,
        1.0,
        0.5,
    ),
    (Surface::Sand, Waveform::Noise, 150.0, 90.0, 0.14, 1.0, 0.35),
    (
        Surface::Wood,
        Waveform::Triangle,
        230.0,
        110.0,
        0.09,
        0.4,
        0.55,
    ),
    (Surface::Water, Waveform::Sine, 520.0, 190.0, 0.26, 0.6, 0.5),
    (Surface::Snow, Waveform::Noise, 120.0, 80.0, 0.12, 1.0, 0.3),
];

/// Every recipe of the game.
#[must_use]
pub fn recipes() -> Vec<Recipe> {
    use Waveform::{Noise, Saw, Sine, Square, Triangle};
    let mut out = vec![
        recipe(
            "shot_rifle",
            (Saw, 190.0, 60.0, 0.14, (0.002, 0.1, 0.0, 0.03), 0.7, 0.8),
        ),
        recipe(
            "shot_smg",
            (Saw, 230.0, 90.0, 0.09, (0.002, 0.06, 0.0, 0.02), 0.6, 0.6),
        ),
        recipe(
            "shot_shotgun",
            (Noise, 100.0, 40.0, 0.3, (0.002, 0.25, 0.0, 0.04), 0.95, 1.0),
        ),
        recipe(
            "shot_precision",
            (Saw, 150.0, 32.0, 0.36, (0.002, 0.3, 0.0, 0.05), 0.5, 1.0),
        ),
        recipe(
            "shot_sidearm",
            (
                Square,
                320.0,
                100.0,
                0.12,
                (0.002, 0.09, 0.0, 0.02),
                0.5,
                0.7,
            ),
        ),
        recipe(
            "shot_launcher",
            (Noise, 90.0, 220.0, 0.42, (0.05, 0.3, 0.0, 0.07), 0.8, 0.8),
        ),
        recipe(
            "shot_goofy",
            (
                Triangle,
                520.0,
                240.0,
                0.18,
                (0.004, 0.12, 0.0, 0.04),
                0.1,
                0.6,
            ),
        ),
        recipe(
            "shot_swing",
            (Noise, 300.0, 120.0, 0.16, (0.04, 0.08, 0.0, 0.04), 0.9, 0.5),
        ),
        recipe(
            "reload",
            (
                Square,
                700.0,
                320.0,
                0.12,
                (0.002, 0.05, 0.1, 0.04),
                0.3,
                0.5,
            ),
        ),
        recipe(
            "hit",
            (
                Sine,
                1500.0,
                1300.0,
                0.05,
                (0.001, 0.03, 0.0, 0.015),
                0.0,
                0.45,
            ),
        ),
        recipe(
            "kill",
            (
                Sine,
                800.0,
                1700.0,
                0.2,
                (0.002, 0.12, 0.2, 0.06),
                0.0,
                0.55,
            ),
        ),
        recipe(
            "hurt",
            (Saw, 130.0, 55.0, 0.18, (0.002, 0.1, 0.1, 0.06), 0.35, 0.7),
        ),
        recipe(
            "explosion",
            (Noise, 80.0, 30.0, 0.95, (0.004, 0.85, 0.0, 0.1), 1.0, 1.0),
        ),
    ];
    for (surface, waveform, from, to, seconds, noise, volume) in SURFACES {
        out.push(recipe(
            Cue::Impact(surface).recipe(),
            (
                waveform,
                from,
                to,
                seconds,
                (0.002, seconds * 0.7, 0.0, 0.02),
                noise,
                volume,
            ),
        ));
        // A footstep is a shorter, softer version of the same material.
        out.push(recipe(
            Cue::Footstep(surface).recipe(),
            (
                if waveform == Square {
                    Triangle
                } else {
                    waveform
                },
                from * 0.5,
                to * 0.6,
                seconds * 0.7,
                (0.004, seconds * 0.5, 0.0, 0.02),
                (noise + 0.3).min(1.0),
                volume * 0.45,
            ),
        ));
    }
    out
}

/// Registers every recipe in the engine (it synthesises them).
pub fn register<B: Backend>(engine: &mut Engine<B>)
where
    B::Error: Debug,
{
    for r in recipes() {
        engine.add_recipe(r);
    }
}

/// Plays `cue`: at `at` in the world (spatialised against the listener), or
/// without position for sounds of the player's own weapon and body. A failure
/// is logged and dropped: a missing sound never stops the game.
pub fn play<B: Backend>(engine: &mut Engine<B>, cue: Cue, at: Option<Vec3>)
where
    B::Error: Debug,
{
    let result = match at {
        Some(position) => engine.play(cue.recipe(), position, cue.priority()),
        None => engine.play_ui(cue.recipe(), cue.priority()),
    };
    if let Err(error) = result {
        tracing::debug!(%error, ?cue, "sound not played");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use faset_audio::{synthesise, Config};
    use kira::backend::mock::{MockBackend, MockBackendSettings};
    use std::collections::HashSet;

    fn all_cues() -> Vec<Cue> {
        let mut cues = vec![Cue::Reload, Cue::Hit, Cue::Kill, Cue::Hurt, Cue::Explosion];
        for s in Surface::ALL {
            cues.push(Cue::Impact(s));
            cues.push(Cue::Footstep(s));
        }
        for w in [
            "assault_rifle",
            "smg",
            "pump_shotgun",
            "precision_rifle",
            "sidearm",
            "baguette",
            "toaster",
            "banana_peel",
            "bubble_gun",
            "confetti_cannon",
            "rubber_chicken_launcher",
            "boomer_rocket_launcher",
            "a_weapon_nobody_knows",
        ] {
            cues.push(Cue::Shot(shot_recipe(w)));
        }
        cues
    }

    #[test]
    fn every_cue_has_a_recipe_and_ids_are_unique() {
        let ids: Vec<String> = recipes().into_iter().map(|r| r.id).collect();
        let set: HashSet<&str> = ids.iter().map(String::as_str).collect();
        assert_eq!(set.len(), ids.len(), "duplicate recipe ids");
        for cue in all_cues() {
            assert!(set.contains(cue.recipe()), "{cue:?} has no recipe");
        }
    }

    #[test]
    fn every_sound_is_audible_finite_and_short() {
        for r in recipes() {
            let samples = synthesise(&r, 44_100);
            assert!(samples.iter().all(|s| s.is_finite()), "{}", r.id);
            let peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
            assert!(peak > 0.05, "{} is silent (peak {peak})", r.id);
            assert!(peak <= 1.0 + 1e-3, "{} clips (peak {peak})", r.id);
            assert!(r.duration > 0.0 && r.duration < 1.5, "{}", r.id);
            assert_eq!(r.bus, Bus::Effects, "{} follows the effects volume", r.id);
        }
    }

    #[test]
    fn the_sounds_differ_from_one_another() {
        let a = synthesise(
            &recipes()
                .into_iter()
                .find(|r| r.id == "impact_stone")
                .unwrap(),
            8_000,
        );
        let b = synthesise(
            &recipes()
                .into_iter()
                .find(|r| r.id == "impact_sand")
                .unwrap(),
            8_000,
        );
        assert_ne!(a, b);
    }

    #[test]
    fn footsteps_are_quieter_than_impacts_and_shots_louder() {
        let by_id = |id: &str| recipes().into_iter().find(|r| r.id == id).unwrap();
        for s in Surface::ALL {
            assert!(
                by_id(Cue::Footstep(s).recipe()).volume < by_id(Cue::Impact(s).recipe()).volume
            );
        }
        assert!(Cue::Kill.priority() > Cue::Footstep(Surface::Sand).priority());
        assert!(Cue::Shot(shot_recipe("smg")).priority() > Cue::Impact(Surface::Wood).priority());
    }

    #[test]
    fn the_cues_play_through_the_engine_and_follow_the_volume() {
        let mut engine = Engine::<MockBackend>::with_backend(
            Config::default(),
            MockBackendSettings {
                sample_rate: 44_100,
            },
        )
        .unwrap();
        register(&mut engine);
        engine.set_listener(Vec3::ZERO, Vec3::NEG_Z, Vec3::Y);
        play(&mut engine, Cue::Shot(shot_recipe("assault_rifle")), None);
        play(
            &mut engine,
            Cue::Impact(Surface::Metal),
            Some(Vec3::new(2.0, 0.0, -5.0)),
        );
        play(&mut engine, Cue::Reload, None);
        assert_eq!(engine.voices().len(), 3);
        // An unknown recipe is dropped quietly.
        play(&mut engine, Cue::Shot("no_such_recipe"), Some(Vec3::Z));
        // A muted effects bus is the settings' business: the volume is a bus.
        engine.set_bus_volume(Bus::Effects, 0.0);
        assert_eq!(engine.config().effects_volume, 0.0);
    }
}

use serde::{Deserialize, Serialize};

use crate::engine::Bus;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Waveform {
    Sine,
    Square,
    Saw,
    Triangle,
    Noise,
}

/// ADSR envelope; times in seconds, `sustain` is a level in 0..=1.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Envelope {
    pub attack: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,
}

impl Default for Envelope {
    fn default() -> Self {
        Self {
            attack: 0.005,
            decay: 0.0,
            sustain: 1.0,
            release: 0.02,
        }
    }
}

/// A sound effect described as data and synthesised at load (no audio file).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Recipe {
    pub id: String,
    pub waveform: Waveform,
    /// Start frequency in Hz.
    pub frequency: f32,
    /// Frequency reached at the end (linear glide); `None` keeps `frequency`.
    pub pitch_end: Option<f32>,
    /// Seconds.
    pub duration: f32,
    pub envelope: Envelope,
    /// Noise mixed over the oscillator, 0..=1.
    pub noise_mix: f32,
    pub noise_seed: u32,
    /// Linear amplitude of the recipe, 0..=1.
    pub volume: f32,
    pub bus: Bus,
}

impl Default for Recipe {
    fn default() -> Self {
        Self {
            id: String::new(),
            waveform: Waveform::Sine,
            frequency: 440.0,
            pitch_end: None,
            duration: 0.2,
            envelope: Envelope::default(),
            noise_mix: 0.0,
            noise_seed: 1,
            volume: 1.0,
            bus: Bus::Effects,
        }
    }
}

impl Recipe {
    /// Parses a recipe from RON.
    pub fn from_ron(text: &str) -> Result<Self, ron::error::SpannedError> {
        ron::from_str(text)
    }
}

fn envelope_at(e: &Envelope, t: f32, duration: f32) -> f32 {
    let mut level = if t < e.attack {
        t / e.attack.max(1e-6)
    } else if t < e.attack + e.decay {
        let a = (t - e.attack) / e.decay.max(1e-6);
        1.0 + (e.sustain - 1.0) * a
    } else {
        e.sustain
    };
    if e.release > 0.0 && t > duration - e.release {
        level *= ((duration - t) / e.release).clamp(0.0, 1.0);
    }
    level
}

/// Renders the recipe to mono samples: `round(duration * sample_rate)` long,
/// deterministic for the same input.
pub fn synthesise(recipe: &Recipe, sample_rate: u32) -> Vec<f32> {
    let sr = sample_rate as f32;
    let n = (recipe.duration.max(0.0) * sr).round() as usize;
    let end_freq = recipe.pitch_end.unwrap_or(recipe.frequency);
    let mut rng = recipe.noise_seed.max(1);
    let mut phase = 0.0f32;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let t = i as f32 / sr;
        let progress = if n > 1 {
            i as f32 / (n - 1) as f32
        } else {
            0.0
        };
        let freq = recipe.frequency + (end_freq - recipe.frequency) * progress;
        phase = (phase + freq / sr).fract();
        // xorshift32 noise in -1..1
        rng ^= rng << 13;
        rng ^= rng >> 17;
        rng ^= rng << 5;
        let noise = (rng as f32 / u32::MAX as f32) * 2.0 - 1.0;
        let osc = match recipe.waveform {
            Waveform::Sine => (phase * std::f32::consts::TAU).sin(),
            Waveform::Square => {
                if phase < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            Waveform::Saw => phase * 2.0 - 1.0,
            Waveform::Triangle => 1.0 - 4.0 * (phase - 0.5).abs(),
            Waveform::Noise => noise,
        };
        let mix = recipe.noise_mix.clamp(0.0, 1.0);
        let s = osc * (1.0 - mix) + noise * mix;
        out.push(s * envelope_at(&recipe.envelope, t, recipe.duration) * recipe.volume);
    }
    out
}

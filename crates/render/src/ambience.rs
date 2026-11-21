//! Ambience profiles: sun, ambient, ramp, rim and fog parameters of the
//! stylised lighting model, plus the values later lots consume (sky,
//! weather, water, exposure and grading). Loaded from RON; two profiles
//! blend linearly for hour and weather transitions.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Where the sun direction comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SunSource {
    /// `Sun::direction` as written in the profile.
    #[default]
    Profile,
    /// Computed from the map time and latitude (lot 2.6). Until then the
    /// renderer uses `Sun::direction`.
    TimeOfDay,
}

/// The directional light.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sun {
    /// Linear RGB, multiplied by `intensity`.
    pub color: [f32; 3],
    /// >= 0; 0 switches the sun off.
    pub intensity: f32,
    /// World-space vector from a surface towards the sun (normalised by the
    /// renderer; must not be zero).
    pub direction: [f32; 3],
    pub source: SunSource,
}

/// Hemispheric ambient: `mix(ground, sky, 0.5 N.y + 0.5)`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ambient {
    pub ground: [f32; 3],
    pub sky: [f32; 3],
}

/// Banded diffuse ramp, shared by the sun and the point lights.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ramp {
    /// Threshold on N.L, in [0, 1].
    pub threshold: f32,
    /// Half width of the transition, in [0, 1].
    pub softness: f32,
    /// Shadow colour of a light as a multiple of its colour (linear RGB):
    /// C_shadow = shadow * C_light.
    pub shadow: [f32; 3],
}

/// Rim light: `color * (1 - max(N.V, 0))^power`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rim {
    /// Linear RGB strength; black disables it.
    pub color: [f32; 3],
    /// > 0.
    pub power: f32,
}

/// Exponential height fog.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fog {
    /// Linear RGB (replaced by the sky colour in the view direction in 2.6).
    pub color: [f32; 3],
    /// Density at `height`, per metre, >= 0; 0 disables fog.
    pub density: f32,
    /// Height falloff per metre, >= 0.
    pub falloff: f32,
    /// Reference height h0 in metres.
    pub height: f32,
}

/// Sun shadow bias, in texels of the cascade the fragment falls in, so the
/// same values work for the three cascades.
///
/// The shadow lookup moves the receiver towards the sun by
/// `(depth_bias + slope_bias * tan(theta)) * texel` metres and out along its
/// normal by `normal_offset * sin(theta) * texel` metres, where `theta` is the
/// angle between the normal and the sun direction.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShadowBias {
    /// Constant depth bias, >= 0.
    pub depth_bias: f32,
    /// Slope-scaled depth bias, per unit of `tan(theta)`, >= 0.
    pub slope_bias: f32,
    /// Offset along the surface normal, scaled by `sin(theta)`, >= 0.
    pub normal_offset: f32,
}

impl Default for ShadowBias {
    fn default() -> Self {
        Self {
            depth_bias: 1.0,
            slope_bias: 1.5,
            normal_offset: 1.0,
        }
    }
}

/// Sky colours (consumed by lot 2.6).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sky {
    pub horizon: [f32; 3],
    pub zenith: [f32; 3],
    /// Horizon-to-zenith exponent, > 0.
    pub exponent: f32,
    /// Star brightness, >= 0.
    pub stars: f32,
}

/// Weather kind (consumed by lot 2.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum WeatherKind {
    #[default]
    Clear,
    Rain,
    Snow,
    Storm,
    Sand,
}

/// Weather parameters (consumed by lot 2.7).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Weather {
    pub kind: WeatherKind,
    /// Particle intensity in [0, 1].
    pub intensity: f32,
    /// Wind speed in metres per second, >= 0.
    pub wind: f32,
    /// Heat haze strength in [0, 1].
    pub heat_haze: f32,
}

/// Colour grading and bloom (consumed by lot 2.9).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grading {
    pub lift: [f32; 3],
    pub gamma: [f32; 3],
    pub gain: [f32; 3],
    pub saturation: f32,
    /// Hue rotation in radians.
    pub hue_shift: f32,
    pub bloom_threshold: f32,
    pub bloom_intensity: f32,
}

/// Water look (consumed by lot 2.8).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Water {
    pub shallow: [f32; 3],
    pub deep: [f32; 3],
    /// Underwater fog density per metre, >= 0.
    pub fog_density: f32,
    /// Caustics strength, >= 0.
    pub caustics: f32,
}

/// Every lighting and atmosphere parameter of a scene mood.
///
/// Colours are linear RGB. [`Default`] is the neutral profile: white
/// ambient, no sun, no rim and no fog, which leaves surfaces at their
/// albedo plus emission.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AmbienceProfile {
    pub sun: Sun,
    pub ambient: Ambient,
    pub ramp: Ramp,
    pub rim: Rim,
    pub fog: Fog,
    pub sky: Sky,
    /// Cascaded sun shadow bias; optional in RON files.
    #[serde(default)]
    pub shadow: ShadowBias,
    pub weather: Weather,
    /// Exposure multiplier, > 0 (applied by tonemapping, lot 2.9).
    pub exposure: f32,
    pub grading: Grading,
    pub water: Water,
}

/// Ambience profile errors.
#[derive(Debug, thiserror::Error)]
pub enum AmbienceError {
    #[error("cannot read ambience profile: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid ambience profile: {0}")]
    Parse(#[from] ron::error::SpannedError),
    #[error("ambience profile value out of range: {0}")]
    InvalidValue(&'static str),
}

/// Names of the built-in profiles (`render/data/ambience/<name>.ron`).
pub const BUILTIN_PROFILES: [&str; 9] = [
    "day",
    "sunset",
    "night",
    "desert",
    "space",
    "underwater",
    "snow",
    "storm",
    "neon",
];

const BUILTIN_SOURCES: [&str; 9] = [
    include_str!("../data/ambience/day.ron"),
    include_str!("../data/ambience/sunset.ron"),
    include_str!("../data/ambience/night.ron"),
    include_str!("../data/ambience/desert.ron"),
    include_str!("../data/ambience/space.ron"),
    include_str!("../data/ambience/underwater.ron"),
    include_str!("../data/ambience/snow.ron"),
    include_str!("../data/ambience/storm.ron"),
    include_str!("../data/ambience/neon.ron"),
];

impl AmbienceProfile {
    /// Parses and validates a RON profile.
    pub fn from_ron(text: &str) -> Result<Self, AmbienceError> {
        let profile: Self = ron::from_str(text)?;
        profile.validate()?;
        Ok(profile)
    }

    /// Reads, parses and validates a RON file.
    pub fn load(path: &Path) -> Result<Self, AmbienceError> {
        Self::from_ron(&std::fs::read_to_string(path)?)
    }

    /// A built-in profile by name (see [`BUILTIN_PROFILES`]).
    pub fn builtin(name: &str) -> Option<Self> {
        let index = BUILTIN_PROFILES.iter().position(|n| *n == name)?;
        Some(Self::from_ron(BUILTIN_SOURCES[index]).expect("built-in profiles are valid"))
    }

    /// Checks value ranges: everything finite, densities, intensities and
    /// strengths >= 0, ramp threshold and softness in [0, 1], powers and
    /// exposure > 0, a non-zero sun direction.
    pub fn validate(&self) -> Result<(), AmbienceError> {
        let check = |ok: bool, what: &'static str| {
            if ok {
                Ok(())
            } else {
                Err(AmbienceError::InvalidValue(what))
            }
        };
        let unit = |x: f32| (0.0..=1.0).contains(&x);
        check(
            self.scalars().iter().all(|x| x.is_finite()),
            "non-finite value",
        )?;
        check(self.sun.intensity >= 0.0, "sun.intensity < 0")?;
        let d = self.sun.direction;
        check(d.iter().any(|c| *c != 0.0), "sun.direction is zero")?;
        check(unit(self.ramp.threshold), "ramp.threshold outside [0, 1]")?;
        check(unit(self.ramp.softness), "ramp.softness outside [0, 1]")?;
        check(self.rim.power > 0.0, "rim.power <= 0")?;
        check(self.fog.density >= 0.0, "fog.density < 0")?;
        check(self.fog.falloff >= 0.0, "fog.falloff < 0")?;
        check(self.sky.exponent > 0.0, "sky.exponent <= 0")?;
        check(self.sky.stars >= 0.0, "sky.stars < 0")?;
        check(self.shadow.depth_bias >= 0.0, "shadow.depth_bias < 0")?;
        check(self.shadow.slope_bias >= 0.0, "shadow.slope_bias < 0")?;
        check(self.shadow.normal_offset >= 0.0, "shadow.normal_offset < 0")?;
        check(
            unit(self.weather.intensity),
            "weather.intensity outside [0, 1]",
        )?;
        check(self.weather.wind >= 0.0, "weather.wind < 0")?;
        check(
            unit(self.weather.heat_haze),
            "weather.heat_haze outside [0, 1]",
        )?;
        check(self.exposure > 0.0, "exposure <= 0")?;
        check(self.water.fog_density >= 0.0, "water.fog_density < 0")?;
        check(self.water.caustics >= 0.0, "water.caustics < 0")?;
        Ok(())
    }

    /// Linear interpolation of every numeric parameter, `t` clamped to
    /// [0, 1]: `a (1 - t) + b t`, so `t = 0` gives `a` and `t = 1` gives `b`
    /// exactly. Enumerations (sun source, weather kind) switch from `a` to
    /// `b` at `t = 0.5`.
    pub fn blend(a: &Self, b: &Self, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        let f = |x: f32, y: f32| x * (1.0 - t) + y * t;
        let v = |x: [f32; 3], y: [f32; 3]| [f(x[0], y[0]), f(x[1], y[1]), f(x[2], y[2])];
        let pick = |x, y| if t < 0.5 { x } else { y };
        Self {
            sun: Sun {
                color: v(a.sun.color, b.sun.color),
                intensity: f(a.sun.intensity, b.sun.intensity),
                direction: v(a.sun.direction, b.sun.direction),
                source: if t < 0.5 { a.sun.source } else { b.sun.source },
            },
            ambient: Ambient {
                ground: v(a.ambient.ground, b.ambient.ground),
                sky: v(a.ambient.sky, b.ambient.sky),
            },
            ramp: Ramp {
                threshold: f(a.ramp.threshold, b.ramp.threshold),
                softness: f(a.ramp.softness, b.ramp.softness),
                shadow: v(a.ramp.shadow, b.ramp.shadow),
            },
            rim: Rim {
                color: v(a.rim.color, b.rim.color),
                power: f(a.rim.power, b.rim.power),
            },
            fog: Fog {
                color: v(a.fog.color, b.fog.color),
                density: f(a.fog.density, b.fog.density),
                falloff: f(a.fog.falloff, b.fog.falloff),
                height: f(a.fog.height, b.fog.height),
            },
            sky: Sky {
                horizon: v(a.sky.horizon, b.sky.horizon),
                zenith: v(a.sky.zenith, b.sky.zenith),
                exponent: f(a.sky.exponent, b.sky.exponent),
                stars: f(a.sky.stars, b.sky.stars),
            },
            shadow: ShadowBias {
                depth_bias: f(a.shadow.depth_bias, b.shadow.depth_bias),
                slope_bias: f(a.shadow.slope_bias, b.shadow.slope_bias),
                normal_offset: f(a.shadow.normal_offset, b.shadow.normal_offset),
            },
            weather: Weather {
                kind: pick(a.weather.kind, b.weather.kind),
                intensity: f(a.weather.intensity, b.weather.intensity),
                wind: f(a.weather.wind, b.weather.wind),
                heat_haze: f(a.weather.heat_haze, b.weather.heat_haze),
            },
            exposure: f(a.exposure, b.exposure),
            grading: Grading {
                lift: v(a.grading.lift, b.grading.lift),
                gamma: v(a.grading.gamma, b.grading.gamma),
                gain: v(a.grading.gain, b.grading.gain),
                saturation: f(a.grading.saturation, b.grading.saturation),
                hue_shift: f(a.grading.hue_shift, b.grading.hue_shift),
                bloom_threshold: f(a.grading.bloom_threshold, b.grading.bloom_threshold),
                bloom_intensity: f(a.grading.bloom_intensity, b.grading.bloom_intensity),
            },
            water: Water {
                shallow: v(a.water.shallow, b.water.shallow),
                deep: v(a.water.deep, b.water.deep),
                fog_density: f(a.water.fog_density, b.water.fog_density),
                caustics: f(a.water.caustics, b.water.caustics),
            },
        }
    }

    /// Every numeric parameter, in declaration order.
    fn scalars(&self) -> Vec<f32> {
        let mut copy = *self;
        copy.scalars_mut().into_iter().map(|x| *x).collect()
    }

    /// Mutable references to every numeric parameter, in declaration order.
    fn scalars_mut(&mut self) -> Vec<&mut f32> {
        let mut out: Vec<&mut f32> = Vec::with_capacity(64);
        let s = &mut self.sun;
        out.extend(&mut s.color);
        out.push(&mut s.intensity);
        out.extend(&mut s.direction);
        out.extend(&mut self.ambient.ground);
        out.extend(&mut self.ambient.sky);
        let r = &mut self.ramp;
        out.extend([&mut r.threshold, &mut r.softness]);
        out.extend(&mut r.shadow);
        out.extend(&mut self.rim.color);
        out.push(&mut self.rim.power);
        let f = &mut self.fog;
        out.extend(&mut f.color);
        out.extend([&mut f.density, &mut f.falloff, &mut f.height]);
        let k = &mut self.sky;
        out.extend(&mut k.horizon);
        out.extend(&mut k.zenith);
        out.extend([&mut k.exponent, &mut k.stars]);
        let sh = &mut self.shadow;
        out.extend([
            &mut sh.depth_bias,
            &mut sh.slope_bias,
            &mut sh.normal_offset,
        ]);
        let w = &mut self.weather;
        out.extend([&mut w.intensity, &mut w.wind, &mut w.heat_haze]);
        out.push(&mut self.exposure);
        let g = &mut self.grading;
        out.extend(&mut g.lift);
        out.extend(&mut g.gamma);
        out.extend(&mut g.gain);
        out.extend([
            &mut g.saturation,
            &mut g.hue_shift,
            &mut g.bloom_threshold,
            &mut g.bloom_intensity,
        ]);
        let w = &mut self.water;
        out.extend(&mut w.shallow);
        out.extend(&mut w.deep);
        out.extend([&mut w.fog_density, &mut w.caustics]);
        out
    }
}

impl Default for AmbienceProfile {
    fn default() -> Self {
        Self {
            sun: Sun {
                color: [1.0; 3],
                intensity: 0.0,
                direction: [0.0, 1.0, 0.0],
                source: SunSource::Profile,
            },
            ambient: Ambient {
                ground: [1.0; 3],
                sky: [1.0; 3],
            },
            ramp: Ramp {
                threshold: 0.5,
                softness: 0.1,
                shadow: [0.0; 3],
            },
            rim: Rim {
                color: [0.0; 3],
                power: 1.0,
            },
            fog: Fog {
                color: [0.0; 3],
                density: 0.0,
                falloff: 0.0,
                height: 0.0,
            },
            sky: Sky {
                horizon: [0.0; 3],
                zenith: [0.0; 3],
                exponent: 1.0,
                stars: 0.0,
            },
            shadow: ShadowBias::default(),
            weather: Weather {
                kind: WeatherKind::Clear,
                intensity: 0.0,
                wind: 0.0,
                heat_haze: 0.0,
            },
            exposure: 1.0,
            grading: Grading {
                lift: [0.0; 3],
                gamma: [1.0; 3],
                gain: [1.0; 3],
                saturation: 1.0,
                hue_shift: 0.0,
                bloom_threshold: 1.0,
                bloom_intensity: 0.0,
            },
            water: Water {
                shallow: [0.0; 3],
                deep: [0.0; 3],
                fog_density: 0.0,
                caustics: 0.0,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all() -> Vec<AmbienceProfile> {
        BUILTIN_PROFILES
            .iter()
            .map(|n| AmbienceProfile::builtin(n).unwrap())
            .collect()
    }

    #[test]
    fn all_builtin_profiles_parse_and_validate() {
        for (name, src) in BUILTIN_PROFILES.iter().zip(BUILTIN_SOURCES) {
            let p =
                AmbienceProfile::from_ron(src).unwrap_or_else(|e| panic!("profile {name}: {e}"));
            p.validate().unwrap();
            assert!(p.fog.density >= 0.0);
            assert!((0.0..=1.0).contains(&p.ramp.threshold));
            assert!((0.0..=1.0).contains(&p.ramp.softness));
        }
        assert!(AmbienceProfile::builtin("mars").is_none());
        AmbienceProfile::default().validate().unwrap();
        let space = AmbienceProfile::builtin("space").unwrap();
        assert_eq!(space.fog.density, 0.0, "space has no fog");
    }

    #[test]
    fn out_of_range_values_are_rejected() {
        let base = AmbienceProfile::builtin("day").unwrap();
        let bad: [fn(&mut AmbienceProfile); 7] = [
            |p| p.fog.density = -0.01,
            |p| p.ramp.threshold = 1.5,
            |p| p.ramp.softness = -0.1,
            |p| p.sun.direction = [0.0; 3],
            |p| p.rim.power = 0.0,
            |p| p.sun.color[1] = f32::NAN,
            |p| p.shadow.slope_bias = -1.0,
        ];
        for edit in bad {
            let mut p = base;
            edit(&mut p);
            assert!(matches!(p.validate(), Err(AmbienceError::InvalidValue(_))));
        }
        let text = ron::to_string(&base).unwrap();
        assert_eq!(AmbienceProfile::from_ron(&text).unwrap(), base);
        let broken = text.replace("density:", "densty:");
        assert!(matches!(
            AmbienceProfile::from_ron(&broken),
            Err(AmbienceError::Parse(_))
        ));
    }

    #[test]
    fn blend_endpoints_are_exact_and_midpoint_is_the_average() {
        let profiles = all();
        for a in &profiles {
            for b in &profiles {
                assert_eq!(AmbienceProfile::blend(a, b, 0.0), *a);
                assert_eq!(AmbienceProfile::blend(a, b, 1.0), *b);
                let mid = AmbienceProfile::blend(a, b, 0.5);
                let (sa, sb, sm) = (a.scalars(), b.scalars(), mid.scalars());
                assert_eq!(sa.len(), sm.len());
                for ((x, y), m) in sa.iter().zip(&sb).zip(&sm) {
                    assert_eq!(*m, 0.5 * (x + y));
                }
                assert_eq!(mid.weather.kind, b.weather.kind);
                assert_eq!(
                    AmbienceProfile::blend(a, b, 0.49).weather.kind,
                    a.weather.kind
                );
            }
        }
    }

    #[test]
    fn blend_covers_every_field() {
        // `b` differs from `a` in every numeric field and both enumerations,
        // so a field `blend` forgot would fail one of the checks.
        let a = AmbienceProfile::default();
        let mut b = a;
        for (k, x) in b.scalars_mut().into_iter().enumerate() {
            *x += 1.0 + k as f32;
        }
        b.sun.source = SunSource::TimeOfDay;
        b.weather.kind = WeatherKind::Storm;
        assert_eq!(AmbienceProfile::blend(&a, &b, 0.0), a);
        assert_eq!(AmbienceProfile::blend(&a, &b, 1.0), b);
        let mid = AmbienceProfile::blend(&a, &b, 0.5).scalars();
        for (k, ((x, y), m)) in a.scalars().iter().zip(b.scalars()).zip(mid).enumerate() {
            assert_ne!(*x, y, "field {k}");
            assert_eq!(m, 0.5 * (x + y), "field {k}");
        }
        // Out-of-range factors clamp.
        assert_eq!(AmbienceProfile::blend(&a, &b, -1.0), a);
        assert_eq!(AmbienceProfile::blend(&a, &b, 2.0), b);
    }
}

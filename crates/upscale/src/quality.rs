//! Quality levels, the render scale and the internal resolution they give.

pub use ash::vk::Extent2D;

use crate::UpscalerKind;

/// Lowest free render scale (50 %).
pub const FREE_SCALE_MIN: f32 = 0.5;
/// Highest free render scale (100 %).
pub const FREE_SCALE_MAX: f32 = 1.0;

/// How far below the display resolution the scene is rendered.
///
/// The four presets divide each axis by a fixed factor; [`Quality::Free`]
/// is a render scale between 50 % and 100 %, only honoured by the Native and
/// FSR 1 upscalers ([`effective_quality`]).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Quality {
    /// Factor 1.5.
    Quality,
    /// Factor 1.7.
    Balanced,
    /// Factor 2.0.
    Performance,
    /// Factor 3.0.
    UltraPerformance,
    /// 100 %: render at the display resolution.
    #[default]
    Native,
    /// A render scale as a fraction of the display resolution, clamped to
    /// [`FREE_SCALE_MIN`]..=[`FREE_SCALE_MAX`].
    Free(f32),
}

impl Quality {
    /// The four preset levels, from the sharpest to the cheapest.
    pub const PRESETS: [Self; 4] = [
        Self::Quality,
        Self::Balanced,
        Self::Performance,
        Self::UltraPerformance,
    ];

    /// The divisor of each axis: render size = display size / factor.
    pub fn factor(self) -> f32 {
        match self {
            Self::Quality => 1.5,
            Self::Balanced => 1.7,
            Self::Performance => 2.0,
            Self::UltraPerformance => 3.0,
            Self::Native => 1.0,
            Self::Free(scale) => 1.0 / clamp_free_scale(scale),
        }
    }

    /// The render scale, `1 / factor`.
    pub fn scale(self) -> f32 {
        1.0 / self.factor()
    }
}

/// Clamps a free render scale to [50 %, 100 %]; NaN gives 100 %.
pub fn clamp_free_scale(scale: f32) -> f32 {
    if scale.is_nan() {
        FREE_SCALE_MAX
    } else {
        scale.clamp(FREE_SCALE_MIN, FREE_SCALE_MAX)
    }
}

/// The internal resolution for `display` at `quality`: each axis divided by
/// the factor and rounded, at least 1 and at most the display size.
pub fn scaled_extent(display: Extent2D, quality: Quality) -> Extent2D {
    let axis = |d: u32| -> u32 {
        let v = (f64::from(d) / f64::from(quality.factor())).round() as u32;
        v.clamp(1, d.max(1))
    };
    Extent2D {
        width: axis(display.width),
        height: axis(display.height),
    }
}

/// The quality an upscaler really uses for a requested one.
///
/// FSR 1 honours every level. FSR 3 and DLSS honour the presets and 100 %
/// (native anti-aliasing, DLAA for DLSS) and treat a free scale as 100 %.
/// Native honours the free scale and 100 %, and treats a preset as 100 %.
/// TAA does not upscale: it always runs at 100 %.
pub fn effective_quality(kind: UpscalerKind, requested: Quality) -> Quality {
    match (kind, requested) {
        (UpscalerKind::Fsr1, q) => q,
        (UpscalerKind::Fsr3 | UpscalerKind::Dlss, Quality::Free(_)) => Quality::Native,
        (UpscalerKind::Fsr3 | UpscalerKind::Dlss, q) => q,
        (UpscalerKind::Native, Quality::Free(s)) => Quality::Free(clamp_free_scale(s)),
        _ => Quality::Native,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ext(width: u32, height: u32) -> Extent2D {
        Extent2D { width, height }
    }

    #[test]
    fn presets_divide_the_display_by_their_factor_and_round() {
        let display = ext(1920, 1080);
        for (quality, factor) in [
            (Quality::Quality, 1.5f64),
            (Quality::Balanced, 1.7),
            (Quality::Performance, 2.0),
            (Quality::UltraPerformance, 3.0),
        ] {
            let e = scaled_extent(display, quality);
            assert_eq!(e.width, (1920.0 / factor).round() as u32, "{quality:?}");
            assert_eq!(e.height, (1080.0 / factor).round() as u32, "{quality:?}");
        }
        assert_eq!(scaled_extent(display, Quality::Quality), ext(1280, 720));
        assert_eq!(scaled_extent(display, Quality::Balanced), ext(1129, 635));
        assert_eq!(scaled_extent(display, Quality::Performance), ext(960, 540));
        assert_eq!(
            scaled_extent(display, Quality::UltraPerformance),
            ext(640, 360)
        );
        assert_eq!(scaled_extent(display, Quality::Native), display);
    }

    #[test]
    fn free_scale_is_clamped_to_fifty_and_one_hundred_percent() {
        let display = ext(1920, 1080);
        assert_eq!(scaled_extent(display, Quality::Free(0.75)), ext(1440, 810));
        assert_eq!(scaled_extent(display, Quality::Free(0.5)), ext(960, 540));
        // Below 50 % and above 100 % are clamped.
        assert_eq!(scaled_extent(display, Quality::Free(0.1)), ext(960, 540));
        assert_eq!(scaled_extent(display, Quality::Free(-3.0)), ext(960, 540));
        assert_eq!(scaled_extent(display, Quality::Free(4.0)), display);
        assert_eq!(scaled_extent(display, Quality::Free(f32::NAN)), display);
        assert_eq!(clamp_free_scale(0.2), FREE_SCALE_MIN);
        assert_eq!(clamp_free_scale(7.0), FREE_SCALE_MAX);
    }

    #[test]
    fn tiny_displays_keep_at_least_one_pixel() {
        assert_eq!(
            scaled_extent(ext(1, 1), Quality::UltraPerformance),
            ext(1, 1)
        );
        assert_eq!(
            scaled_extent(ext(2, 100), Quality::UltraPerformance),
            ext(1, 33)
        );
    }

    #[test]
    fn only_native_and_fsr1_honour_the_free_scale() {
        let free = Quality::Free(0.6);
        assert_eq!(effective_quality(UpscalerKind::Fsr1, free), free);
        assert_eq!(effective_quality(UpscalerKind::Native, free), free);
        assert_eq!(effective_quality(UpscalerKind::Taa, free), Quality::Native);
        // Presets: FSR 1 only.
        let preset = Quality::Performance;
        assert_eq!(effective_quality(UpscalerKind::Fsr1, preset), preset);
        assert_eq!(
            effective_quality(UpscalerKind::Native, preset),
            Quality::Native
        );
        assert_eq!(
            effective_quality(UpscalerKind::Taa, preset),
            Quality::Native
        );
        // FSR 3: presets as given, no free scale.
        for q in Quality::PRESETS {
            assert_eq!(effective_quality(UpscalerKind::Fsr3, q), q);
        }
        assert_eq!(effective_quality(UpscalerKind::Fsr3, free), Quality::Native);
        // DLSS: the same levels as FSR 3.
        for q in Quality::PRESETS {
            assert_eq!(effective_quality(UpscalerKind::Dlss, q), q);
        }
        assert_eq!(effective_quality(UpscalerKind::Dlss, free), Quality::Native);
        assert_eq!(
            effective_quality(UpscalerKind::Dlss, Quality::Native),
            Quality::Native
        );
        // Out of range free scales are clamped.
        assert_eq!(
            effective_quality(UpscalerKind::Native, Quality::Free(0.1)),
            Quality::Free(FREE_SCALE_MIN)
        );
    }
}

//! The jitter sequence of the temporal upscalers (FSR 3 and DLSS): Halton
//! (2, 3), as long as `8 * (display / render)^2` frames.

use faset_render::glam::Vec2;

/// Length of the jitter sequence for a scaling ratio, as
/// `ffxQueryDescUpscaleGetJitterPhaseCount` and the DLSS programming guide:
/// `8 * (display / render)^2`.
pub(crate) fn jitter_phase_count(render_width: u32, display_width: u32) -> u32 {
    let ratio = display_width.max(1) as f32 / render_width.max(1) as f32;
    ((8.0 * ratio * ratio) as u32).max(1)
}

/// Jitter of a frame in render pixels, as `ffxQueryDescUpscaleGetJitterOffset`:
/// Halton (2, 3) at `index % phases + 1`, centred on zero.
pub(crate) fn jitter_offset(index: u32, phases: u32) -> Vec2 {
    let k = index % phases.max(1) + 1;
    Vec2::new(halton(k, 2) - 0.5, halton(k, 3) - 0.5)
}

fn halton(mut index: u32, base: u32) -> f32 {
    let mut f = 1.0f32;
    let mut r = 0.0f32;
    while index > 0 {
        f /= base as f32;
        r += f * (index % base) as f32;
        index /= base;
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_jitter_sequence_is_halton_2_3() {
        // 8 * ratio^2, truncated.
        assert_eq!(jitter_phase_count(1920, 1920), 8);
        assert_eq!(jitter_phase_count(1280, 1920), 18);
        assert_eq!(jitter_phase_count(960, 1920), 32);
        assert_eq!(jitter_phase_count(640, 1920), 72);
        assert!(jitter_offset(0, 8).abs_diff_eq(Vec2::new(0.0, -1.0 / 6.0), 1e-6));
        assert_eq!(jitter_offset(8, 8), jitter_offset(0, 8));
        for i in 0..72 {
            let j = jitter_offset(i, 72);
            assert!(j.x.abs() <= 0.5 && j.y.abs() <= 0.5, "{i}: {j}");
        }
    }
}

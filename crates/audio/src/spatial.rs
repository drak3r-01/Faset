use faset_core::glam::Vec3;

/// Reference distance in metres: closer than this a source is at full volume.
pub const D_REF: f32 = 2.0;
/// Roll-off factor of the attenuation law.
pub const K: f32 = 1.0;
/// Low-pass cutoff (Hz) for a source hidden behind a wall.
pub const OCCLUDED_CUTOFF_HZ: f64 = 1200.0;
/// Low-pass cutoff (Hz) when the listener is underwater.
pub const UNDERWATER_CUTOFF_HZ: f64 = 500.0;

/// `G(d) = clamp(d_ref / (d_ref + k (d - d_ref)), 0, 1)` (plan section 19).
pub fn attenuation(d: f32, d_ref: f32, k: f32) -> f32 {
    (d_ref / (d_ref + k * (d - d_ref))).clamp(0.0, 1.0)
}

/// Tunable spatialisation constants.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpatialConfig {
    pub d_ref: f32,
    pub k: f32,
}

impl Default for SpatialConfig {
    fn default() -> Self {
        Self { d_ref: D_REF, k: K }
    }
}

/// Result of spatialising one source against the listener.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spatial {
    /// Linear gain in 0..=1.
    pub gain: f32,
    /// Pan in -1 (left) ..= 1 (right).
    pub pan: f32,
    /// Low-pass cutoff in Hz when the sound must be filtered.
    pub lowpass_hz: Option<f64>,
}

/// Computes gain, pan and filtering. `right = forward x up` (+X in the identity pose).
pub fn spatialise(
    listener: Vec3,
    forward: Vec3,
    up: Vec3,
    source: Vec3,
    occluded: bool,
    underwater: bool,
    cfg: SpatialConfig,
) -> Spatial {
    let offset = source - listener;
    let d = offset.length();
    let right = forward.cross(up).normalize_or_zero();
    let pan = if d > 1e-4 {
        (offset / d).dot(right).clamp(-1.0, 1.0)
    } else {
        0.0
    };
    let lowpass_hz = if underwater {
        Some(UNDERWATER_CUTOFF_HZ)
    } else if occluded {
        Some(OCCLUDED_CUTOFF_HZ)
    } else {
        None
    };
    Spatial {
        gain: attenuation(d, cfg.d_ref, cfg.k),
        pan,
        lowpass_hz,
    }
}

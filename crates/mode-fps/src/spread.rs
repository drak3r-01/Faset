//! Deterministic spread: cone size, bloom and the direction of one pellet.
//!
//! Trigonometry and `exp` go through `libm` (pure software, the same bits on
//! every platform), `sqrt` and the vector maths are exact IEEE operations:
//! a client on Windows and a dedicated server on Linux draw the same
//! direction bit for bit.

use faset_core::glam::Vec3;
use faset_sim::controller::sin_cos_q;
use faset_sim::rng::Pcg32;

use crate::weapon::{SightDef, SpreadDef};

/// Camera basis of a shot: forward, right and up unit vectors.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AimBasis {
    pub forward: Vec3,
    pub right: Vec3,
    pub up: Vec3,
}

impl AimBasis {
    /// Basis of quantised view angles, as carried by an `InputCommand`
    /// (step 2π/65536; yaw 0 faces -Z, positive pitch looks up).
    #[must_use]
    pub fn from_quantized(yaw: u16, pitch: i16) -> Self {
        let (sy, cy) = sin_cos_q(yaw);
        let (sp, cp) = sin_cos_q(pitch as u16);
        let forward = Vec3::new(-sy * cp, sp, -cy * cp);
        let right = Vec3::new(cy, 0.0, -sy);
        Self {
            forward,
            right,
            up: right.cross(forward),
        }
    }

    /// Basis looking along `forward` with no roll; `None` for a zero or
    /// vertical vector.
    #[must_use]
    pub fn looking_along(forward: Vec3) -> Option<Self> {
        let forward = forward.try_normalize()?;
        let right = forward.cross(Vec3::Y).try_normalize()?;
        Some(Self {
            forward,
            right,
            up: right.cross(forward),
        })
    }
}

/// What the shooter is doing, as far as the cone is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Stance {
    pub moving: bool,
    pub airborne: bool,
    pub sighted: bool,
}

/// m_state: product of the movement, airborne and sight multipliers.
#[must_use]
pub fn state_multiplier(spread: &SpreadDef, sight: &SightDef, stance: Stance) -> f32 {
    let mut m = 1.0;
    if stance.moving {
        m *= spread.move_multiplier;
    }
    if stance.airborne {
        m *= spread.air_multiplier;
    }
    if stance.sighted {
        m *= sight.spread_multiplier;
    }
    m
}

/// Current cone half angle: `min(sigma_base * m_state + beta, sigma_max)`.
#[must_use]
pub fn cone_sigma(spread: &SpreadDef, m_state: f32, beta: f32) -> f32 {
    (spread.sigma_base * m_state + beta).min(spread.sigma_max)
}

/// Bloom after one tick of `dt` seconds: `beta * exp(-dt / tau)`.
#[must_use]
pub fn decay_beta(beta: f32, dt: f32, tau: f32) -> f32 {
    beta * libm::expf(-dt / tau)
}

/// Direction of one pellet: `u1` and `u2` are uniform in `[0, 1)`.
///
/// `r = tan(sigma) sqrt(u1)`, `phi = 2 pi u2`,
/// `d = normalize(F + r cos(phi) R + r sin(phi) U)`.
#[must_use]
pub fn spread_direction(basis: &AimBasis, sigma: f32, u1: f32, u2: f32) -> Vec3 {
    let r = libm::tanf(sigma) * libm::sqrtf(u1);
    let phi = std::f32::consts::TAU * u2;
    let (s, c) = (libm::sinf(phi), libm::cosf(phi));
    (basis.forward + basis.right * (r * c) + basis.up * (r * s)).normalize()
}

/// Draws the two uniforms of one pellet from the shot generator.
pub fn draw_pair(rng: &mut Pcg32) -> (f32, f32) {
    let u1 = rng.next_f32();
    let u2 = rng.next_f32();
    (u1, u2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use faset_core::NetId;

    fn spread() -> SpreadDef {
        SpreadDef {
            sigma_base: 0.01,
            sigma_max: 0.05,
            delta_beta: 0.01,
            tau: 0.25,
            move_multiplier: 2.0,
            air_multiplier: 3.0,
        }
    }

    fn sight() -> SightDef {
        SightDef {
            fov: 1.0,
            spread_multiplier: 0.5,
            speed_multiplier: 0.8,
        }
    }

    #[test]
    fn basis_is_orthonormal_and_faces_minus_z() {
        let b = AimBasis::from_quantized(0, 0);
        assert!((b.forward - Vec3::NEG_Z).length() < 1e-6);
        assert!((b.right - Vec3::X).length() < 1e-6);
        assert!((b.up - Vec3::Y).length() < 1e-6);
        let b = AimBasis::from_quantized(12_345, -3_000);
        assert!(b.forward.dot(b.right).abs() < 1e-5);
        assert!(b.forward.dot(b.up).abs() < 1e-5);
        assert!((b.up.length() - 1.0).abs() < 1e-5);
        // Positive pitch looks up.
        assert!(AimBasis::from_quantized(0, 4_000).forward.y > 0.0);
    }

    #[test]
    fn cone_combines_multipliers_and_caps() {
        let m = state_multiplier(
            &spread(),
            &sight(),
            Stance {
                moving: true,
                airborne: true,
                sighted: true,
            },
        );
        assert!((m - 3.0).abs() < 1e-6);
        assert!((cone_sigma(&spread(), 1.0, 0.0) - 0.01).abs() < 1e-7);
        assert!((cone_sigma(&spread(), 2.0, 0.01) - 0.03).abs() < 1e-7);
        assert!((cone_sigma(&spread(), 3.0, 0.1) - 0.05).abs() < 1e-7);
    }

    #[test]
    fn bloom_decays_exponentially() {
        let b = decay_beta(0.04, 0.25, 0.25);
        assert!((b - 0.04 * (-1.0f32).exp()).abs() < 1e-6);
    }

    #[test]
    fn centre_draw_goes_straight_and_edge_draw_stays_in_the_cone() {
        let basis = AimBasis::from_quantized(0, 0);
        let d = spread_direction(&basis, 0.05, 0.0, 0.3);
        assert!((d - basis.forward).length() < 1e-6);
        let d = spread_direction(&basis, 0.05, 0.999_999, 0.7);
        let angle = d.dot(basis.forward).clamp(-1.0, 1.0).acos();
        assert!(angle <= 0.05 + 1e-4);
    }

    #[test]
    fn spread_identical_client_server() {
        // Two independent evaluations (client prediction, server check) of
        // the same (seed, player, counter) agree bit for bit.
        let basis = AimBasis::from_quantized(777, 321);
        for counter in 0..64 {
            let dir = |_path: u8| {
                let mut rng = Pcg32::for_shot(0xFA5E7, NetId(5), counter);
                let (u1, u2) = draw_pair(&mut rng);
                spread_direction(&basis, 0.03, u1, u2)
            };
            let (c, s) = (dir(0), dir(1));
            assert_eq!(c.x.to_bits(), s.x.to_bits());
            assert_eq!(c.y.to_bits(), s.y.to_bits());
            assert_eq!(c.z.to_bits(), s.z.to_bits());
        }
        // Different counters give different directions.
        let a = Pcg32::for_shot(1, NetId(5), 0).next_u32();
        let b = Pcg32::for_shot(1, NetId(5), 1).next_u32();
        assert_ne!(a, b);
    }
}

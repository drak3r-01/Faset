//! PCG32 (XSH-RR). Shot spread is seeded from (server seed, player, shot
//! counter) so a client reproduces its own spread exactly.

use faset_core::NetId;

const MULT: u64 = 6_364_136_223_846_793_005;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pcg32 {
    state: u64,
    inc: u64,
}

impl Pcg32 {
    #[must_use]
    pub fn new(seed: u64, stream: u64) -> Self {
        let mut r = Self {
            state: 0,
            inc: (stream << 1) | 1,
        };
        r.next_u32();
        r.state = r.state.wrapping_add(seed);
        r.next_u32();
        r
    }

    /// Generator for one shot.
    #[must_use]
    pub fn for_shot(server_seed: u64, player: NetId, shot_counter: u32) -> Self {
        Self::new(
            server_seed ^ (u64::from(shot_counter) << 16),
            u64::from(player.0),
        )
    }

    pub fn next_u32(&mut self) -> u32 {
        let old = self.state;
        self.state = old.wrapping_mul(MULT).wrapping_add(self.inc);
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        let rot = (old >> 59) as u32;
        xorshifted.rotate_right(rot)
    }

    /// Uniform float in [0, 1).
    pub fn next_f32(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / (1u32 << 24) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_values() {
        // Reference sequence of the PCG32 demo (seed 42, stream 54).
        let mut r = Pcg32::new(42, 54);
        let got: Vec<u32> = (0..3).map(|_| r.next_u32()).collect();
        assert_eq!(got, vec![0xa15c_02b7, 0x7b47_f409, 0xba1d_3330]);
    }

    #[test]
    fn shot_rng_is_reproducible() {
        let mut a = Pcg32::for_shot(9, NetId(3), 7);
        let mut b = Pcg32::for_shot(9, NetId(3), 7);
        assert_eq!(a.next_u32(), b.next_u32());
        assert_ne!(
            Pcg32::for_shot(9, NetId(3), 8).next_u32(),
            Pcg32::for_shot(9, NetId(4), 7).next_u32()
        );
    }
}

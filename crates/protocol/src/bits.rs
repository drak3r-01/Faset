//! Bit-level writer and reader (LSB-first within each byte).

use crate::message::DecodeError;

/// Appends values of 1..=64 bits to a byte buffer.
#[derive(Debug, Default, Clone)]
pub struct BitWriter {
    bytes: Vec<u8>,
    bit_len: usize,
}

impl BitWriter {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of bits written so far.
    #[must_use]
    pub fn bit_len(&self) -> usize {
        self.bit_len
    }

    /// Writes the low `n` bits of `value` (`n <= 64`; higher bits are ignored).
    pub fn write_bits(&mut self, value: u64, n: u32) {
        debug_assert!(n <= 64);
        for i in 0..n {
            let bit = (value >> i) & 1;
            let pos = self.bit_len & 7;
            if pos == 0 {
                self.bytes.push(0);
            }
            if bit == 1 {
                if let Some(last) = self.bytes.last_mut() {
                    *last |= 1 << pos;
                }
            }
            self.bit_len += 1;
        }
    }

    pub fn write_bool(&mut self, v: bool) {
        self.write_bits(u64::from(v), 1);
    }

    /// Finishes the buffer; trailing padding bits are zero.
    #[must_use]
    pub fn finish(self) -> Vec<u8> {
        self.bytes
    }
}

/// Reads bit fields back, with bounds checking.
#[derive(Debug, Clone)]
pub struct BitReader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> BitReader<'a> {
    #[must_use]
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    /// Bits left to read.
    #[must_use]
    pub fn remaining_bits(&self) -> usize {
        self.bytes.len() * 8 - self.pos
    }

    /// Reads `n` bits (`n <= 64`).
    ///
    /// # Errors
    /// [`DecodeError::Truncated`] if fewer than `n` bits remain.
    pub fn read_bits(&mut self, n: u32) -> Result<u64, DecodeError> {
        if n > 64 || self.remaining_bits() < n as usize {
            return Err(DecodeError::Truncated);
        }
        let mut v = 0u64;
        for i in 0..n {
            let byte = self.bytes[self.pos >> 3];
            let bit = u64::from((byte >> (self.pos & 7)) & 1);
            v |= bit << i;
            self.pos += 1;
        }
        Ok(v)
    }

    /// # Errors
    /// [`DecodeError::Truncated`] at end of input.
    pub fn read_bool(&mut self) -> Result<bool, DecodeError> {
        Ok(self.read_bits(1)? == 1)
    }

    /// Checks that only zero padding (less than one byte) remains.
    ///
    /// # Errors
    /// [`DecodeError::TrailingData`] otherwise.
    pub fn finish(&mut self) -> Result<(), DecodeError> {
        let rem = self.remaining_bits();
        if rem >= 8 || self.read_bits(rem as u32)? != 0 {
            return Err(DecodeError::TrailingData);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn packs_lsb_first() {
        let mut w = BitWriter::new();
        w.write_bits(0b101, 3);
        w.write_bits(0xFF, 8);
        assert_eq!(w.bit_len(), 11);
        assert_eq!(w.finish(), vec![0b1111_1101, 0b0000_0111]);
    }

    #[test]
    fn truncated_read_errors() {
        let mut r = BitReader::new(&[0xFF]);
        assert!(r.read_bits(9).is_err());
        assert_eq!(r.read_bits(8).unwrap(), 0xFF);
        assert!(r.read_bool().is_err());
    }

    proptest! {
        #[test]
        fn round_trip(fields in proptest::collection::vec((any::<u64>(), 1u32..=64), 0..40)) {
            let mut w = BitWriter::new();
            for (v, n) in &fields {
                w.write_bits(*v, *n);
            }
            let bytes = w.finish();
            let mut r = BitReader::new(&bytes);
            for (v, n) in &fields {
                let mask = if *n == 64 { u64::MAX } else { (1u64 << n) - 1 };
                prop_assert_eq!(r.read_bits(*n).unwrap(), v & mask);
            }
            prop_assert!(r.finish().is_ok());
        }
    }
}

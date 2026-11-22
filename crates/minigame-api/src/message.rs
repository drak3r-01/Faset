//! Byte layouts of mode messages and of the replicated match state.
//!
//! `protocol` knows no mode: both travel as opaque bytes.

use faset_core::Tick;

use crate::phase::Phase;

/// Numeric mode id, assigned by the [`Registry`](crate::Registry) in
/// registration order; the value of the handshake's `mode` byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModeId(pub u8);

/// Malformed mode bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DecodeError {
    /// Fewer bytes than the layout needs.
    #[error("truncated")]
    Truncated,
    /// An unknown phase code.
    #[error("unknown phase {0}")]
    Phase(u8),
    /// Bytes after the end of the layout.
    #[error("trailing data")]
    TrailingData,
    /// An unknown tag or code, bad UTF-8 or a non-finite float.
    #[error("invalid value")]
    Invalid,
}

/// A mode-defined message: `ModeMessage(mode_id, bytes)`.
///
/// Layout: mode id (1 byte), then the bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeMessage {
    /// Mode the bytes belong to.
    pub mode: ModeId,
    /// Mode-defined payload.
    pub bytes: Vec<u8>,
}

impl ModeMessage {
    /// Wire bytes.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(1 + self.bytes.len());
        out.push(self.mode.0);
        out.extend_from_slice(&self.bytes);
        out
    }

    /// Inverse of [`encode`](Self::encode).
    ///
    /// # Errors
    /// Empty input.
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let (&mode, rest) = bytes.split_first().ok_or(DecodeError::Truncated)?;
        Ok(Self {
            mode: ModeId(mode),
            bytes: rest.to_vec(),
        })
    }
}

/// Replicated match state: phase, its end tick and the mode's state.
///
/// Layout: phase code (1), end flag (1), end tick (4, little endian, only
/// with the flag), mode state length (4, little endian), mode state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchState {
    pub phase: Phase,
    /// End tick of a timed phase.
    pub phase_end: Option<Tick>,
    /// [`GameMode::replicated_state`](crate::GameMode::replicated_state).
    pub mode_state: Vec<u8>,
}

impl MatchState {
    /// Wire bytes.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = vec![self.phase.to_u8(), u8::from(self.phase_end.is_some())];
        if let Some(end) = self.phase_end {
            out.extend_from_slice(&end.0.to_le_bytes());
        }
        let len = u32::try_from(self.mode_state.len()).unwrap_or(u32::MAX);
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&self.mode_state[..len as usize]);
        out
    }

    /// Inverse of [`encode`](Self::encode).
    ///
    /// # Errors
    /// Truncated or trailing bytes, or an unknown phase.
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut r = bytes;
        let code = take(&mut r, 1)?[0];
        let phase = Phase::from_u8(code).ok_or(DecodeError::Phase(code))?;
        let phase_end = match take(&mut r, 1)?[0] {
            0 => None,
            _ => Some(Tick(read_u32(&mut r)?)),
        };
        let len = read_u32(&mut r)? as usize;
        let mode_state = take(&mut r, len)?.to_vec();
        if !r.is_empty() {
            return Err(DecodeError::TrailingData);
        }
        Ok(Self {
            phase,
            phase_end,
            mode_state,
        })
    }
}

fn take<'a>(r: &mut &'a [u8], n: usize) -> Result<&'a [u8], DecodeError> {
    if r.len() < n {
        return Err(DecodeError::Truncated);
    }
    let (head, tail) = r.split_at(n);
    *r = tail;
    Ok(head)
}

fn read_u32(r: &mut &[u8]) -> Result<u32, DecodeError> {
    let b = take(r, 4)?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_state_round_trips() {
        for end in [None, Some(Tick(1234))] {
            let s = MatchState {
                phase: Phase::RoundEnd,
                phase_end: end,
                mode_state: vec![1, 2, 3],
            };
            assert_eq!(MatchState::decode(&s.encode()), Ok(s.clone()));
            let mut long = s.encode();
            long.push(0);
            assert_eq!(MatchState::decode(&long), Err(DecodeError::TrailingData));
            let short = &s.encode()[..3];
            assert_eq!(MatchState::decode(short), Err(DecodeError::Truncated));
        }
    }

    #[test]
    fn mode_message_round_trips() {
        let m = ModeMessage {
            mode: ModeId(3),
            bytes: vec![9, 8],
        };
        assert_eq!(ModeMessage::decode(&m.encode()), Ok(m));
        assert_eq!(ModeMessage::decode(&[]), Err(DecodeError::Truncated));
    }
}

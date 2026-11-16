//! Message enum with bit-exact `encode` / `decode`.
//!
//! Layout: 8-bit message tag, then the fields below, LSB-first, zero padded
//! to a whole byte. Decoding is strict: unknown tags, out-of-range values,
//! trailing bytes and non-zero padding are errors.

use faset_core::{NetId, Tick};
use thiserror::Error;

use crate::bits::{BitReader, BitWriter};

/// Wire protocol version; a mismatch is rejected at handshake.
pub const PROTOCOL_VERSION: u16 = 1;
/// Maximum size of one packet payload in bytes.
pub const MAX_PACKET_BYTES: usize = 1200;
/// Maximum chat text length in bytes (decision).
pub const MAX_CHAT_BYTES: usize = 200;
/// Maximum map chunk data length in bytes (decision; fits one packet with header).
pub const MAX_CHUNK_BYTES: usize = 1024;
/// Maximum opaque snapshot payload length in bytes (reserved for lots 1.4/1.5).
pub const MAX_SNAPSHOT_BYTES: usize = 1150;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DecodeError {
    #[error("input ended before the message was complete")]
    Truncated,
    #[error("unknown message tag {0}")]
    UnknownTag(u8),
    #[error("unknown event tag {0}")]
    UnknownEvent(u8),
    #[error("field value out of range")]
    OutOfRange,
    #[error("length {0} exceeds the allowed maximum")]
    TooLong(usize),
    #[error("text is not valid UTF-8")]
    InvalidUtf8,
    #[error("unexpected data after the message")]
    TrailingData,
    #[error("packet larger than {MAX_PACKET_BYTES} bytes")]
    PacketTooLarge,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum EncodeError {
    #[error("variable-length field too long ({0} bytes)")]
    FieldTooLong(usize),
    #[error("encoded message exceeds {MAX_PACKET_BYTES} bytes")]
    PacketTooLarge,
}

/// Client-to-server input command, sent at 60 Hz on channel 0.
///
/// Channel 0 carries only commands, so the 13-byte body has no message tag
/// (deviation: the plan says "about 10 bytes"; the field list sums to 13).
///
/// `yaw` and `pitch` are raw quantized values (see [`crate::quant`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputCommand {
    pub tick: Tick,
    pub buttons: u16,
    pub move_x: i8,
    pub move_y: i8,
    pub yaw: u16,
    pub pitch: i16,
    pub weapon_slot: u8,
}

impl InputCommand {
    /// Exact encoded size in bytes.
    pub const SIZE: usize = 13;

    #[must_use]
    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut w = BitWriter::new();
        w.write_bits(u64::from(self.tick.0), 32);
        w.write_bits(u64::from(self.buttons), 16);
        w.write_bits(u64::from(self.move_x as u8), 8);
        w.write_bits(u64::from(self.move_y as u8), 8);
        w.write_bits(u64::from(self.yaw), 16);
        w.write_bits(u64::from(self.pitch as u16), 16);
        w.write_bits(u64::from(self.weapon_slot), 8);
        let mut out = [0u8; Self::SIZE];
        out.copy_from_slice(&w.finish());
        out
    }

    /// # Errors
    /// [`DecodeError`] unless `bytes` is exactly 13 bytes.
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        if bytes.len() > Self::SIZE {
            return Err(DecodeError::TrailingData);
        }
        let mut r = BitReader::new(bytes);
        let c = Self {
            tick: Tick(r.read_bits(32)? as u32),
            buttons: r.read_bits(16)? as u16,
            move_x: r.read_bits(8)? as u8 as i8,
            move_y: r.read_bits(8)? as u8 as i8,
            yaw: r.read_bits(16)? as u16,
            pitch: r.read_bits(16)? as u16 as i16,
            weapon_slot: r.read_bits(8)? as u8,
        };
        r.finish()?;
        Ok(c)
    }
}

/// Reliable event messages (channel 2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Death {
        victim: NetId,
        killer: NetId,
        weapon: u8,
    },
    Score {
        player: NetId,
        score: i16,
    },
    Phase {
        phase: u8,
        ticks_remaining: u32,
    },
    MapChange {
        map_fingerprint: u64,
    },
    Chat {
        sender: NetId,
        text: String,
    },
}

/// Every message that crosses the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    ClientHello {
        protocol_version: u16,
        build_fingerprint: u64,
    },
    ServerHello {
        mode: u8,
        player_id: NetId,
        map_fingerprint: u64,
        tick_rate: u8,
    },
    Ready,
    /// Reserved: layout implemented in lot 1.4. `data` is opaque here.
    SnapshotFull {
        tick: Tick,
        data: Vec<u8>,
    },
    /// Reserved: layout implemented in lot 1.5. `data` is opaque here.
    SnapshotDelta {
        tick: Tick,
        baseline: Tick,
        data: Vec<u8>,
    },
    /// Channel 2.
    Event(Event),
    /// Channel 3.
    MapChunk {
        index: u16,
        total: u16,
        data: Vec<u8>,
    },
}

const T_CLIENT_HELLO: u8 = 0;
const T_SERVER_HELLO: u8 = 1;
const T_READY: u8 = 2;
const T_SNAP_FULL: u8 = 4;
const T_SNAP_DELTA: u8 = 5;
const T_EVENT: u8 = 6;
const T_MAP_CHUNK: u8 = 7;

const E_DEATH: u8 = 0;
const E_SCORE: u8 = 1;
const E_PHASE: u8 = 2;
const E_MAP_CHANGE: u8 = 3;
const E_CHAT: u8 = 4;

fn write_bytes(w: &mut BitWriter, data: &[u8], max: usize) -> Result<(), EncodeError> {
    if data.len() > max {
        return Err(EncodeError::FieldTooLong(data.len()));
    }
    w.write_bits(data.len() as u64, 16);
    for b in data {
        w.write_bits(u64::from(*b), 8);
    }
    Ok(())
}

fn read_bytes(r: &mut BitReader, max: usize) -> Result<Vec<u8>, DecodeError> {
    let len = r.read_bits(16)? as usize;
    if len > max {
        return Err(DecodeError::TooLong(len));
    }
    if r.remaining_bits() < len * 8 {
        return Err(DecodeError::Truncated);
    }
    let mut out = Vec::with_capacity(len);
    for _ in 0..len {
        out.push(r.read_bits(8)? as u8);
    }
    Ok(out)
}

impl Event {
    fn write(&self, w: &mut BitWriter) -> Result<(), EncodeError> {
        match self {
            Self::Death {
                victim,
                killer,
                weapon,
            } => {
                w.write_bits(u64::from(E_DEATH), 8);
                w.write_bits(u64::from(victim.0), 16);
                w.write_bits(u64::from(killer.0), 16);
                w.write_bits(u64::from(*weapon), 8);
            }
            Self::Score { player, score } => {
                w.write_bits(u64::from(E_SCORE), 8);
                w.write_bits(u64::from(player.0), 16);
                w.write_bits(u64::from(*score as u16), 16);
            }
            Self::Phase {
                phase,
                ticks_remaining,
            } => {
                w.write_bits(u64::from(E_PHASE), 8);
                w.write_bits(u64::from(*phase), 8);
                w.write_bits(u64::from(*ticks_remaining), 32);
            }
            Self::MapChange { map_fingerprint } => {
                w.write_bits(u64::from(E_MAP_CHANGE), 8);
                w.write_bits(*map_fingerprint, 64);
            }
            Self::Chat { sender, text } => {
                w.write_bits(u64::from(E_CHAT), 8);
                w.write_bits(u64::from(sender.0), 16);
                write_bytes(w, text.as_bytes(), MAX_CHAT_BYTES)?;
            }
        }
        Ok(())
    }

    fn read(r: &mut BitReader) -> Result<Self, DecodeError> {
        let tag = r.read_bits(8)? as u8;
        Ok(match tag {
            E_DEATH => Self::Death {
                victim: NetId(r.read_bits(16)? as u16),
                killer: NetId(r.read_bits(16)? as u16),
                weapon: r.read_bits(8)? as u8,
            },
            E_SCORE => Self::Score {
                player: NetId(r.read_bits(16)? as u16),
                score: r.read_bits(16)? as u16 as i16,
            },
            E_PHASE => Self::Phase {
                phase: r.read_bits(8)? as u8,
                ticks_remaining: r.read_bits(32)? as u32,
            },
            E_MAP_CHANGE => Self::MapChange {
                map_fingerprint: r.read_bits(64)?,
            },
            E_CHAT => {
                let sender = NetId(r.read_bits(16)? as u16);
                let bytes = read_bytes(r, MAX_CHAT_BYTES)?;
                let text = String::from_utf8(bytes).map_err(|_| DecodeError::InvalidUtf8)?;
                Self::Chat { sender, text }
            }
            other => return Err(DecodeError::UnknownEvent(other)),
        })
    }
}

impl Message {
    /// Encodes the message into at most [`MAX_PACKET_BYTES`] bytes.
    ///
    /// # Errors
    /// [`EncodeError`] if a variable-length field or the whole packet is too long.
    pub fn encode(&self) -> Result<Vec<u8>, EncodeError> {
        let mut w = BitWriter::new();
        match self {
            Self::ClientHello {
                protocol_version,
                build_fingerprint,
            } => {
                w.write_bits(u64::from(T_CLIENT_HELLO), 8);
                w.write_bits(u64::from(*protocol_version), 16);
                w.write_bits(*build_fingerprint, 64);
            }
            Self::ServerHello {
                mode,
                player_id,
                map_fingerprint,
                tick_rate,
            } => {
                w.write_bits(u64::from(T_SERVER_HELLO), 8);
                w.write_bits(u64::from(*mode), 8);
                w.write_bits(u64::from(player_id.0), 16);
                w.write_bits(*map_fingerprint, 64);
                w.write_bits(u64::from(*tick_rate), 8);
            }
            Self::Ready => w.write_bits(u64::from(T_READY), 8),
            Self::SnapshotFull { tick, data } => {
                w.write_bits(u64::from(T_SNAP_FULL), 8);
                w.write_bits(u64::from(tick.0), 32);
                write_bytes(&mut w, data, MAX_SNAPSHOT_BYTES)?;
            }
            Self::SnapshotDelta {
                tick,
                baseline,
                data,
            } => {
                w.write_bits(u64::from(T_SNAP_DELTA), 8);
                w.write_bits(u64::from(tick.0), 32);
                w.write_bits(u64::from(baseline.0), 32);
                write_bytes(&mut w, data, MAX_SNAPSHOT_BYTES)?;
            }
            Self::Event(e) => {
                w.write_bits(u64::from(T_EVENT), 8);
                e.write(&mut w)?;
            }
            Self::MapChunk { index, total, data } => {
                w.write_bits(u64::from(T_MAP_CHUNK), 8);
                w.write_bits(u64::from(*index), 16);
                w.write_bits(u64::from(*total), 16);
                write_bytes(&mut w, data, MAX_CHUNK_BYTES)?;
            }
        }
        let bytes = w.finish();
        if bytes.len() > MAX_PACKET_BYTES {
            return Err(EncodeError::PacketTooLarge);
        }
        Ok(bytes)
    }

    /// Decodes one message. Never panics on arbitrary input.
    ///
    /// # Errors
    /// [`DecodeError`] for malformed, truncated or oversized input.
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        if bytes.len() > MAX_PACKET_BYTES {
            return Err(DecodeError::PacketTooLarge);
        }
        let mut r = BitReader::new(bytes);
        let tag = r.read_bits(8)? as u8;
        let msg = match tag {
            T_CLIENT_HELLO => Self::ClientHello {
                protocol_version: r.read_bits(16)? as u16,
                build_fingerprint: r.read_bits(64)?,
            },
            T_SERVER_HELLO => Self::ServerHello {
                mode: r.read_bits(8)? as u8,
                player_id: NetId(r.read_bits(16)? as u16),
                map_fingerprint: r.read_bits(64)?,
                tick_rate: r.read_bits(8)? as u8,
            },
            T_READY => Self::Ready,
            T_SNAP_FULL => Self::SnapshotFull {
                tick: Tick(r.read_bits(32)? as u32),
                data: read_bytes(&mut r, MAX_SNAPSHOT_BYTES)?,
            },
            T_SNAP_DELTA => Self::SnapshotDelta {
                tick: Tick(r.read_bits(32)? as u32),
                baseline: Tick(r.read_bits(32)? as u32),
                data: read_bytes(&mut r, MAX_SNAPSHOT_BYTES)?,
            },
            T_EVENT => Self::Event(Event::read(&mut r)?),
            T_MAP_CHUNK => Self::MapChunk {
                index: r.read_bits(16)? as u16,
                total: r.read_bits(16)? as u16,
                data: read_bytes(&mut r, MAX_CHUNK_BYTES)?,
            },
            other => return Err(DecodeError::UnknownTag(other)),
        };
        r.finish()?;
        Ok(msg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn arb_input() -> impl Strategy<Value = InputCommand> {
        (
            any::<u32>(),
            any::<u16>(),
            any::<i8>(),
            any::<i8>(),
            any::<u16>(),
            any::<i16>(),
            any::<u8>(),
        )
            .prop_map(|(t, buttons, move_x, move_y, yaw, pitch, weapon_slot)| {
                InputCommand {
                    tick: Tick(t),
                    buttons,
                    move_x,
                    move_y,
                    yaw,
                    pitch,
                    weapon_slot,
                }
            })
    }

    fn arb_event() -> impl Strategy<Value = Event> {
        prop_oneof![
            (any::<u16>(), any::<u16>(), any::<u8>()).prop_map(|(v, k, weapon)| Event::Death {
                victim: NetId(v),
                killer: NetId(k),
                weapon
            }),
            (any::<u16>(), any::<i16>()).prop_map(|(p, score)| Event::Score {
                player: NetId(p),
                score
            }),
            (any::<u8>(), any::<u32>()).prop_map(|(phase, ticks_remaining)| Event::Phase {
                phase,
                ticks_remaining
            }),
            any::<u64>().prop_map(|map_fingerprint| Event::MapChange { map_fingerprint }),
            (any::<u16>(), "\\PC{0,40}").prop_map(|(s, text)| Event::Chat {
                sender: NetId(s),
                text
            }),
        ]
    }

    fn arb_message() -> impl Strategy<Value = Message> {
        prop_oneof![
            (any::<u16>(), any::<u64>()).prop_map(|(protocol_version, build_fingerprint)| {
                Message::ClientHello {
                    protocol_version,
                    build_fingerprint,
                }
            }),
            (any::<u8>(), any::<u16>(), any::<u64>(), any::<u8>()).prop_map(
                |(mode, p, map_fingerprint, tick_rate)| Message::ServerHello {
                    mode,
                    player_id: NetId(p),
                    map_fingerprint,
                    tick_rate
                }
            ),
            Just(Message::Ready),
            (
                any::<u32>(),
                proptest::collection::vec(any::<u8>(), 0..=MAX_SNAPSHOT_BYTES)
            )
                .prop_map(|(t, data)| Message::SnapshotFull {
                    tick: Tick(t),
                    data
                }),
            (
                any::<u32>(),
                any::<u32>(),
                proptest::collection::vec(any::<u8>(), 0..=MAX_SNAPSHOT_BYTES)
            )
                .prop_map(|(t, b, data)| Message::SnapshotDelta {
                    tick: Tick(t),
                    baseline: Tick(b),
                    data
                }),
            arb_event().prop_map(Message::Event),
            (
                any::<u16>(),
                any::<u16>(),
                proptest::collection::vec(any::<u8>(), 0..=MAX_CHUNK_BYTES)
            )
                .prop_map(|(index, total, data)| Message::MapChunk {
                    index,
                    total,
                    data
                }),
        ]
    }

    proptest! {
        #[test]
        fn every_variant_round_trips(m in arb_message()) {
            let bytes = m.encode().unwrap();
            prop_assert!(bytes.len() <= MAX_PACKET_BYTES);
            prop_assert_eq!(Message::decode(&bytes).unwrap(), m);
        }

        #[test]
        fn arbitrary_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..1300)) {
            let _ = Message::decode(&bytes);
        }
    }

    proptest! {
        #[test]
        fn input_command_round_trips_in_13_bytes(c in arb_input()) {
            let bytes = c.encode();
            prop_assert_eq!(bytes.len(), 13);
            prop_assert_eq!(InputCommand::decode(&bytes).unwrap(), c);
        }

        #[test]
        fn input_decode_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..40)) {
            let _ = InputCommand::decode(&bytes);
        }
    }

    #[test]
    fn rejects_bad_input() {
        assert_eq!(Message::decode(&[]), Err(DecodeError::Truncated));
        assert_eq!(Message::decode(&[0xEE]), Err(DecodeError::UnknownTag(0xEE)));
        assert_eq!(
            Message::decode(&[T_READY, 0]),
            Err(DecodeError::TrailingData)
        );
        let long = Message::MapChunk {
            index: 0,
            total: 1,
            data: vec![0; MAX_CHUNK_BYTES + 1],
        };
        assert!(long.encode().is_err());
    }
}

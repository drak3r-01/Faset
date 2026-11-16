//! Snapshots: full layout, quantized state, delta encoding and acknowledgements.
//!
//! # Full snapshot (`Message::SnapshotFull::data`)
//!
//! Layout, LSB-first through [`BitWriter`], zero padded to a byte:
//!
//! | field                      | bits |
//! |----------------------------|------|
//! | has last command           | 1    |
//! | last command tick          | 32 (only if present) |
//! | entity count               | 8    |
//! | per entity: net id         | 16   |
//! | position x, y, z (1/256 m) | 3 x 24 |
//! | velocity x, y, z (1/16 m/s)| 3 x 16 |
//! | yaw                        | 16   |
//! | pitch                      | 12   |
//! | health (0 to 255)          | 8    |
//! | weapon slot (0 to 15)      | 4    |
//! | on ground, sliding, alive  | 3 x 1 |
//!
//! 179 bits per entity; [`MAX_SNAPSHOT_ENTITIES`] fit the protocol's
//! snapshot payload limit.
//!
//! # Delta snapshot
//!
//! See [`delta`] for the `Message::SnapshotDelta::data` layout, the
//! 64-snapshot reference ring and the acknowledgement carried by input
//! packets.
//!
//! Both codecs work on [`QuantizedSnapshot`], the exact integer values on the
//! wire, so a delta reconstructs the sender's quantized state bit for bit.

use faset_core::glam::Vec3;
use faset_core::{NetId, Tick};
use faset_protocol::message::MAX_SNAPSHOT_BYTES;
use faset_protocol::quant::{
    dequantize_pitch_snapshot, dequantize_position, dequantize_velocity, dequantize_yaw,
    quantize_pitch_snapshot, quantize_position, quantize_velocity, quantize_yaw,
    PITCH_SNAPSHOT_BITS, POS_BITS, VEL_BITS,
};
use faset_protocol::{BitReader, BitWriter, DecodeError, EncodeError, Message};

pub mod delta;

pub use delta::{
    InputPacket, SnapshotDecoder, SnapshotEncoder, ENTITY_FIELDS, REFERENCE_SNAPSHOTS,
};

const ENTITY_BITS: usize = 16 + 3 * POS_BITS as usize + 3 * VEL_BITS as usize + 16 + 12 + 8 + 4 + 3;
const HEADER_BITS: usize = 1 + 32 + 8;
const WEAPON_BITS: u32 = 4;

/// Most entities one snapshot carries (decision: 48, below the 51 that fit
/// [`MAX_SNAPSHOT_BYTES`]).
pub const MAX_SNAPSHOT_ENTITIES: usize = 48;

const _: () = assert!(HEADER_BITS + MAX_SNAPSHOT_ENTITIES * ENTITY_BITS <= MAX_SNAPSHOT_BYTES * 8);

/// Snapshot encode or decode failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SnapshotError {
    /// More entities than [`MAX_SNAPSHOT_ENTITIES`].
    #[error("{0} entities exceed the snapshot maximum")]
    TooManyEntities(usize),
    /// The message is not the expected snapshot message.
    #[error("not a snapshot message of the expected kind")]
    WrongMessage,
    /// A delta refers to a baseline snapshot the receiver does not hold.
    #[error("delta baseline {0:?} is unknown")]
    MissingBaseline(Tick),
    /// Malformed payload.
    #[error("malformed snapshot: {0}")]
    Decode(#[from] DecodeError),
    /// The message could not be encoded.
    #[error("snapshot encoding failed: {0}")]
    Encode(#[from] EncodeError),
}

/// State of one entity as sent on the wire. Values are what the receiver
/// gets back after quantization.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EntityState {
    pub id: NetId,
    /// Capsule centre, metres.
    pub position: Vec3,
    /// Metres per second.
    pub velocity: Vec3,
    /// Radians, [0, 2pi).
    pub yaw: f32,
    /// Radians, [-pi/2, pi/2].
    pub pitch: f32,
    pub health: u8,
    /// Weapon slot, 0 to 15.
    pub weapon: u8,
    pub on_ground: bool,
    pub sliding: bool,
    pub alive: bool,
}

/// A full snapshot for one client.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    /// World tick after the step this snapshot describes.
    pub tick: Tick,
    /// Tick of the last command of the receiving client the server applied.
    pub last_command: Option<Tick>,
    /// Entities in ascending id order.
    pub entities: Vec<EntityState>,
}

/// One entity as raw quantized wire values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct QuantizedEntity {
    pub id: NetId,
    /// Units of 1/256 m, 24-bit signed range.
    pub position: [i32; 3],
    /// Units of 1/16 m/s.
    pub velocity: [i16; 3],
    pub yaw: u16,
    /// 12-bit snapshot pitch.
    pub pitch: u16,
    pub health: u8,
    /// 0 to 15.
    pub weapon: u8,
    pub on_ground: bool,
    pub sliding: bool,
    pub alive: bool,
}

/// A snapshot as raw quantized wire values: the unit both codecs compare
/// and reconstruct.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct QuantizedSnapshot {
    pub tick: Tick,
    pub last_command: Option<Tick>,
    /// Entities in ascending id order.
    pub entities: Vec<QuantizedEntity>,
}

impl QuantizedEntity {
    /// Quantizes `e` exactly as the wire does.
    #[must_use]
    pub fn from_state(e: &EntityState) -> Self {
        Self {
            id: e.id,
            position: e.position.to_array().map(quantize_position),
            velocity: e.velocity.to_array().map(quantize_velocity),
            yaw: quantize_yaw(e.yaw),
            pitch: quantize_pitch_snapshot(e.pitch),
            health: e.health,
            weapon: e.weapon.min(15),
            on_ground: e.on_ground,
            sliding: e.sliding,
            alive: e.alive,
        }
    }

    /// The values the receiver works with.
    #[must_use]
    pub fn to_state(&self) -> EntityState {
        EntityState {
            id: self.id,
            position: Vec3::from_array(self.position.map(dequantize_position)),
            velocity: Vec3::from_array(self.velocity.map(dequantize_velocity)),
            yaw: dequantize_yaw(self.yaw),
            pitch: dequantize_pitch_snapshot(self.pitch),
            health: self.health,
            weapon: self.weapon,
            on_ground: self.on_ground,
            sliding: self.sliding,
            alive: self.alive,
        }
    }
}

fn write_signed(w: &mut BitWriter, v: i64, bits: u32) {
    w.write_bits(v as u64 & ((1u64 << bits) - 1), bits);
}

fn read_signed(r: &mut BitReader, bits: u32) -> Result<i64, DecodeError> {
    let raw = r.read_bits(bits)?;
    let shift = 64 - bits;
    Ok(((raw << shift) as i64) >> shift)
}

/// Writes field `index` (see [`ENTITY_FIELDS`]) of `e`.
pub(crate) fn write_field(w: &mut BitWriter, e: &QuantizedEntity, index: usize) {
    match index {
        0..=2 => write_signed(w, i64::from(e.position[index]), POS_BITS),
        3..=5 => write_signed(w, i64::from(e.velocity[index - 3]), VEL_BITS),
        6 => w.write_bits(u64::from(e.yaw), 16),
        7 => w.write_bits(u64::from(e.pitch), PITCH_SNAPSHOT_BITS),
        8 => w.write_bits(u64::from(e.health), 8),
        9 => w.write_bits(u64::from(e.weapon), WEAPON_BITS),
        _ => {
            w.write_bool(e.on_ground);
            w.write_bool(e.sliding);
            w.write_bool(e.alive);
        }
    }
}

/// Reads field `index` (see [`ENTITY_FIELDS`]) into `e`.
pub(crate) fn read_field(
    r: &mut BitReader,
    e: &mut QuantizedEntity,
    index: usize,
) -> Result<(), DecodeError> {
    match index {
        0..=2 => e.position[index] = read_signed(r, POS_BITS)? as i32,
        3..=5 => e.velocity[index - 3] = read_signed(r, VEL_BITS)? as i16,
        6 => e.yaw = r.read_bits(16)? as u16,
        7 => e.pitch = r.read_bits(PITCH_SNAPSHOT_BITS)? as u16,
        8 => e.health = r.read_bits(8)? as u8,
        9 => e.weapon = r.read_bits(WEAPON_BITS)? as u8,
        _ => {
            e.on_ground = r.read_bool()?;
            e.sliding = r.read_bool()?;
            e.alive = r.read_bool()?;
        }
    }
    Ok(())
}

/// Whether field `index` (see [`ENTITY_FIELDS`]) differs between `a` and `b`.
pub(crate) fn field_differs(a: &QuantizedEntity, b: &QuantizedEntity, index: usize) -> bool {
    match index {
        0..=2 => a.position[index] != b.position[index],
        3..=5 => a.velocity[index - 3] != b.velocity[index - 3],
        6 => a.yaw != b.yaw,
        7 => a.pitch != b.pitch,
        8 => a.health != b.health,
        9 => a.weapon != b.weapon,
        _ => (a.on_ground, a.sliding, a.alive) != (b.on_ground, b.sliding, b.alive),
    }
}

pub(crate) fn empty_entity(id: NetId) -> QuantizedEntity {
    QuantizedEntity {
        id,
        position: [0; 3],
        velocity: [0; 3],
        yaw: 0,
        pitch: 0,
        health: 0,
        weapon: 0,
        on_ground: false,
        sliding: false,
        alive: false,
    }
}

pub(crate) fn write_last_command(w: &mut BitWriter, last: Option<Tick>) {
    match last {
        Some(t) => {
            w.write_bool(true);
            w.write_bits(u64::from(t.0), 32);
        }
        None => w.write_bool(false),
    }
}

pub(crate) fn read_last_command(r: &mut BitReader) -> Result<Option<Tick>, DecodeError> {
    Ok(if r.read_bool()? {
        Some(Tick(r.read_bits(32)? as u32))
    } else {
        None
    })
}

impl QuantizedSnapshot {
    /// Quantizes `s` exactly as the wire does.
    #[must_use]
    pub fn from_snapshot(s: &Snapshot) -> Self {
        Self {
            tick: s.tick,
            last_command: s.last_command,
            entities: s.entities.iter().map(QuantizedEntity::from_state).collect(),
        }
    }

    /// The values the receiver works with.
    #[must_use]
    pub fn to_snapshot(&self) -> Snapshot {
        Snapshot {
            tick: self.tick,
            last_command: self.last_command,
            entities: self
                .entities
                .iter()
                .map(QuantizedEntity::to_state)
                .collect(),
        }
    }

    /// Encodes the full snapshot payload (`Message::SnapshotFull::data`).
    ///
    /// # Errors
    /// [`SnapshotError::TooManyEntities`].
    pub fn encode_full(&self) -> Result<Vec<u8>, SnapshotError> {
        if self.entities.len() > MAX_SNAPSHOT_ENTITIES {
            return Err(SnapshotError::TooManyEntities(self.entities.len()));
        }
        let mut w = BitWriter::new();
        write_last_command(&mut w, self.last_command);
        w.write_bits(self.entities.len() as u64, 8);
        for e in &self.entities {
            w.write_bits(u64::from(e.id.0), 16);
            for f in 0..ENTITY_FIELDS {
                write_field(&mut w, e, f);
            }
        }
        Ok(w.finish())
    }

    /// Decodes a full snapshot payload.
    ///
    /// # Errors
    /// [`SnapshotError::Decode`] or [`SnapshotError::TooManyEntities`].
    pub fn decode_full(tick: Tick, data: &[u8]) -> Result<Self, SnapshotError> {
        let mut r = BitReader::new(data);
        let last_command = read_last_command(&mut r)?;
        let count = r.read_bits(8)? as usize;
        if count > MAX_SNAPSHOT_ENTITIES {
            return Err(SnapshotError::TooManyEntities(count));
        }
        let mut entities = Vec::with_capacity(count);
        for _ in 0..count {
            let mut e = empty_entity(NetId(r.read_bits(16)? as u16));
            for f in 0..ENTITY_FIELDS {
                read_field(&mut r, &mut e, f)?;
            }
            entities.push(e);
        }
        r.finish()?;
        Ok(Self {
            tick,
            last_command,
            entities,
        })
    }
}

impl Snapshot {
    /// Encodes the payload carried in `Message::SnapshotFull::data`.
    ///
    /// # Errors
    /// [`SnapshotError::TooManyEntities`].
    pub fn encode_data(&self) -> Result<Vec<u8>, SnapshotError> {
        QuantizedSnapshot::from_snapshot(self).encode_full()
    }

    /// Decodes a payload written by [`encode_data`](Self::encode_data).
    ///
    /// # Errors
    /// [`SnapshotError::Decode`] for malformed input.
    pub fn decode_data(tick: Tick, data: &[u8]) -> Result<Self, SnapshotError> {
        Ok(QuantizedSnapshot::decode_full(tick, data)?.to_snapshot())
    }

    /// Wraps the snapshot in a `Message::SnapshotFull`.
    ///
    /// # Errors
    /// See [`encode_data`](Self::encode_data).
    pub fn to_message(&self) -> Result<Message, SnapshotError> {
        Ok(Message::SnapshotFull {
            tick: self.tick,
            data: self.encode_data()?,
        })
    }

    /// Reads a `Message::SnapshotFull`. Deltas need a
    /// [`SnapshotDecoder`], which holds their baselines.
    ///
    /// # Errors
    /// [`SnapshotError::WrongMessage`] for any other message, or a decode error.
    pub fn from_message(message: &Message) -> Result<Self, SnapshotError> {
        match message {
            Message::SnapshotFull { tick, data } => Self::decode_data(*tick, data),
            _ => Err(SnapshotError::WrongMessage),
        }
    }

    /// Encodes the full wire message.
    ///
    /// # Errors
    /// See [`SnapshotError`].
    pub fn encode(&self) -> Result<Vec<u8>, SnapshotError> {
        Ok(self.to_message()?.encode()?)
    }

    /// Decodes the full wire message.
    ///
    /// # Errors
    /// See [`SnapshotError`].
    pub fn decode(bytes: &[u8]) -> Result<Self, SnapshotError> {
        Self::from_message(&Message::decode(bytes)?)
    }

    /// The entity with `id`, if present.
    #[must_use]
    pub fn entity(&self, id: NetId) -> Option<&EntityState> {
        self.entities
            .binary_search_by_key(&id, |e| e.id)
            .ok()
            .map(|i| &self.entities[i])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use faset_protocol::quant::{PITCH_SNAPSHOT_STEP, POS_STEP, VEL_STEP};
    use proptest::prelude::*;

    fn entity(id: u16) -> EntityState {
        EntityState {
            id: NetId(id),
            position: Vec3::new(1.0, 2.0, -3.5),
            velocity: Vec3::new(-5.0, 0.0, 2.25),
            yaw: 1.0,
            pitch: -0.3,
            health: 100,
            weapon: 3,
            on_ground: true,
            sliding: false,
            alive: true,
        }
    }

    #[test]
    fn max_entities_fit_one_packet() {
        let s = Snapshot {
            tick: Tick(u32::MAX),
            last_command: Some(Tick(u32::MAX)),
            entities: (0..MAX_SNAPSHOT_ENTITIES as u16).map(entity).collect(),
        };
        let bytes = s.encode().unwrap();
        assert!(bytes.len() <= faset_protocol::MAX_PACKET_BYTES);
        assert_eq!(
            Snapshot::decode(&bytes).unwrap().entities.len(),
            MAX_SNAPSHOT_ENTITIES
        );
        let mut too_many = s;
        too_many.entities.push(entity(999));
        assert_eq!(
            too_many.encode(),
            Err(SnapshotError::TooManyEntities(MAX_SNAPSHOT_ENTITIES + 1))
        );
    }

    #[test]
    fn full_layout_is_179_bits_per_entity() {
        let s = Snapshot {
            tick: Tick(1),
            last_command: None,
            entities: (0..8).map(entity).collect(),
        };
        let bits = 1 + 8 + 8 * ENTITY_BITS;
        assert_eq!(ENTITY_BITS, 179);
        assert_eq!(s.encode_data().unwrap().len(), bits.div_ceil(8));
    }

    #[test]
    fn rejects_other_messages() {
        assert_eq!(
            Snapshot::from_message(&Message::Ready),
            Err(SnapshotError::WrongMessage)
        );
    }

    proptest! {
        #[test]
        fn roundtrip_within_half_step(
            x in -1000.0f32..1000.0, y in -1000.0f32..1000.0, z in -1000.0f32..1000.0,
            vx in -2000.0f32..2000.0, vz in -2000.0f32..2000.0,
            pitch in -1.5f32..1.5, health: u8, weapon in 0u8..16,
            ack: Option<u32>, flags: (bool, bool, bool),
        ) {
            let e = EntityState {
                id: NetId(7),
                position: Vec3::new(x, y, z),
                velocity: Vec3::new(vx, 0.0, vz),
                yaw: 0.5,
                pitch,
                health,
                weapon,
                on_ground: flags.0,
                sliding: flags.1,
                alive: flags.2,
            };
            let s = Snapshot { tick: Tick(9), last_command: ack.map(Tick), entities: vec![e] };
            let d = Snapshot::decode(&s.encode().unwrap()).unwrap();
            prop_assert_eq!(d.tick, s.tick);
            prop_assert_eq!(d.last_command, s.last_command);
            let g = d.entities[0];
            prop_assert!((g.position - e.position).abs().max_element() <= POS_STEP / 2.0 + 1e-4);
            prop_assert!((g.velocity - e.velocity).abs().max_element() <= VEL_STEP / 2.0 + 1e-4);
            prop_assert!((g.pitch - e.pitch).abs() <= PITCH_SNAPSHOT_STEP / 2.0 + 1e-5);
            prop_assert_eq!((g.health, g.weapon), (health, weapon));
            prop_assert_eq!((g.on_ground, g.sliding, g.alive), flags);
        }

        #[test]
        fn decode_never_panics(data in proptest::collection::vec(any::<u8>(), 0..300)) {
            let _ = Snapshot::decode_data(Tick(0), &data);
        }
    }
}

//! Full snapshot layout (the opaque `data` of `Message::SnapshotFull`).
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

use faset_core::glam::Vec3;
use faset_core::{NetId, Tick};
use faset_protocol::message::MAX_SNAPSHOT_BYTES;
use faset_protocol::quant::{
    dequantize_pitch_snapshot, dequantize_position, dequantize_velocity, dequantize_yaw,
    quantize_pitch_snapshot, quantize_position, quantize_velocity, quantize_yaw,
    PITCH_SNAPSHOT_BITS, POS_BITS, VEL_BITS,
};
use faset_protocol::{BitReader, BitWriter, DecodeError, EncodeError, Message};

const ENTITY_BITS: usize = 16 + 3 * POS_BITS as usize + 3 * VEL_BITS as usize + 16 + 12 + 8 + 4 + 3;
const HEADER_BITS: usize = 1 + 32 + 8;
const WEAPON_BITS: u32 = 4;

/// Most entities one full snapshot carries (decision: 48, below the 51 that
/// fit [`MAX_SNAPSHOT_BYTES`]).
pub const MAX_SNAPSHOT_ENTITIES: usize = 48;

const _: () = assert!(HEADER_BITS + MAX_SNAPSHOT_ENTITIES * ENTITY_BITS <= MAX_SNAPSHOT_BYTES * 8);

/// Snapshot encode or decode failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SnapshotError {
    /// More entities than [`MAX_SNAPSHOT_ENTITIES`].
    #[error("{0} entities exceed the snapshot maximum")]
    TooManyEntities(usize),
    /// The message is not a full snapshot.
    #[error("not a full snapshot message")]
    WrongMessage,
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

fn write_vec(w: &mut BitWriter, v: Vec3, q: fn(f32) -> i64, bits: u32) {
    for c in [v.x, v.y, v.z] {
        let mask = (1u64 << bits) - 1;
        w.write_bits(q(c) as u64 & mask, bits);
    }
}

fn read_vec(r: &mut BitReader, bits: u32, dq: fn(i64) -> f32) -> Result<Vec3, DecodeError> {
    let mut c = [0.0f32; 3];
    for v in &mut c {
        let raw = r.read_bits(bits)?;
        // Sign-extend from `bits` to 64.
        let shift = 64 - bits;
        *v = dq(((raw << shift) as i64) >> shift);
    }
    Ok(Vec3::from_array(c))
}

impl Snapshot {
    /// Encodes the payload carried in `Message::SnapshotFull::data`.
    ///
    /// # Errors
    /// [`SnapshotError::TooManyEntities`].
    pub fn encode_data(&self) -> Result<Vec<u8>, SnapshotError> {
        if self.entities.len() > MAX_SNAPSHOT_ENTITIES {
            return Err(SnapshotError::TooManyEntities(self.entities.len()));
        }
        let mut w = BitWriter::new();
        match self.last_command {
            Some(t) => {
                w.write_bool(true);
                w.write_bits(u64::from(t.0), 32);
            }
            None => w.write_bool(false),
        }
        w.write_bits(self.entities.len() as u64, 8);
        for e in &self.entities {
            w.write_bits(u64::from(e.id.0), 16);
            write_vec(
                &mut w,
                e.position,
                |v| i64::from(quantize_position(v)),
                POS_BITS,
            );
            write_vec(
                &mut w,
                e.velocity,
                |v| i64::from(quantize_velocity(v)),
                VEL_BITS,
            );
            w.write_bits(u64::from(quantize_yaw(e.yaw)), 16);
            w.write_bits(
                u64::from(quantize_pitch_snapshot(e.pitch)),
                PITCH_SNAPSHOT_BITS,
            );
            w.write_bits(u64::from(e.health), 8);
            w.write_bits(u64::from(e.weapon.min(15)), WEAPON_BITS);
            w.write_bool(e.on_ground);
            w.write_bool(e.sliding);
            w.write_bool(e.alive);
        }
        Ok(w.finish())
    }

    /// Decodes a payload written by [`encode_data`](Self::encode_data).
    ///
    /// # Errors
    /// [`SnapshotError::Decode`] for malformed input.
    pub fn decode_data(tick: Tick, data: &[u8]) -> Result<Self, SnapshotError> {
        let mut r = BitReader::new(data);
        let last_command = if r.read_bool()? {
            Some(Tick(r.read_bits(32)? as u32))
        } else {
            None
        };
        let count = r.read_bits(8)? as usize;
        if count > MAX_SNAPSHOT_ENTITIES {
            return Err(SnapshotError::TooManyEntities(count));
        }
        let mut entities = Vec::with_capacity(count);
        for _ in 0..count {
            let id = NetId(r.read_bits(16)? as u16);
            let position = read_vec(&mut r, POS_BITS, |q| dequantize_position(q as i32))?;
            let velocity = read_vec(&mut r, VEL_BITS, |q| dequantize_velocity(q as i16))?;
            let yaw = dequantize_yaw(r.read_bits(16)? as u16);
            let pitch_q = r.read_bits(PITCH_SNAPSHOT_BITS)? as u16;
            if pitch_q > 4095 {
                return Err(DecodeError::OutOfRange.into());
            }
            let pitch = dequantize_pitch_snapshot(pitch_q);
            entities.push(EntityState {
                id,
                position,
                velocity,
                yaw,
                pitch,
                health: r.read_bits(8)? as u8,
                weapon: r.read_bits(WEAPON_BITS)? as u8,
                on_ground: r.read_bool()?,
                sliding: r.read_bool()?,
                alive: r.read_bool()?,
            });
        }
        r.finish()?;
        Ok(Self {
            tick,
            last_command,
            entities,
        })
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

    /// Reads a `Message::SnapshotFull`.
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

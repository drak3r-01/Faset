//! Delta snapshots against the last snapshot each client acknowledged.
//!
//! # Acknowledgement carrier (decision)
//!
//! The plan does not say where the client's acknowledgement travels. Each
//! channel 0 input packet carries it: an [`InputPacket`] is the 13-byte
//! [`InputCommand`] followed by the tick of the newest snapshot the client
//! has received, as a little-endian `u32` (17 bytes). A bare 13-byte command
//! is still accepted and carries no acknowledgement.
//!
//! # Reference ring
//!
//! The server keeps the last [`REFERENCE_SNAPSHOTS`] (64) snapshots it sent
//! to each client ([`SnapshotEncoder`]). An acknowledgement only counts when
//! it names one of them and is newer than the previous one. The next
//! snapshot is a delta against the acknowledged one while it is still in the
//! ring; otherwise (no acknowledgement yet, or the acknowledged snapshot was
//! pushed out of the ring) the server falls back to a full snapshot. It also
//! sends a full snapshot whenever that is not larger than the delta.
//!
//! The client keeps the last 64 snapshots it received ([`SnapshotDecoder`]),
//! so it always holds every baseline the server may pick.
//!
//! # Delta layout (`Message::SnapshotDelta::data`)
//!
//! The message header already carries the snapshot tick and the baseline
//! tick. The payload, LSB-first through [`BitWriter`], zero padded to a byte:
//!
//! | field                          | bits |
//! |--------------------------------|------|
//! | has last command               | 1    |
//! | last command tick              | 32 (only if present) |
//! | removed count                  | 8    |
//! | per removed entity: net id     | 16, strictly ascending |
//! | record count                   | 8    |
//! | per record: net id             | 16, strictly ascending |
//! | created                        | 1    |
//! | if created: every field        | as in the full layout (163 bits) |
//! | else: changed field mask       | 11 ([`ENTITY_FIELDS`]), non-zero |
//! | else: each changed field       | as in the full layout, in mask order, except position and velocity (below) |
//!
//! A changed position or velocity axis (lot 4.11) is a flag bit, then
//! either (flag set) its difference to the baseline, signed, in 12 bits for
//! a position axis (±4 m) and 8 bits for a velocity axis (±8 m/s), or (flag
//! clear) the full field. The character controller changes the velocity of
//! a turning player over many ticks, so this keeps 16 moving players below
//! 100 kbit/s.
//!
//! Field mask bits, from bit 0: position x, y, z; velocity x, y, z; yaw;
//! pitch; health; weapon; the three flags (on ground, sliding, alive) as one
//! field.
//!
//! Entity creation and removal are explicit: a removed entity is listed by
//! id; a created entity is a record with the created bit and every field.
//! An entity of the baseline that is neither removed nor in a record is
//! unchanged. A created id must not be in the baseline after removals; a
//! changed id must be. Decoding rejects anything else.

use std::collections::VecDeque;

use faset_core::{NetId, Tick};
use faset_protocol::{BitReader, BitWriter, DecodeError, InputCommand, Message};

use super::{
    empty_entity, field_differs, read_field, read_last_command, write_field, write_last_command,
    QuantizedEntity, QuantizedSnapshot, Snapshot, SnapshotError, MAX_SNAPSHOT_ENTITIES,
};

/// Number of reference snapshots kept per client, on both sides.
pub const REFERENCE_SNAPSHOTS: usize = 64;

/// Number of fields in the delta change mask.
pub const ENTITY_FIELDS: usize = 11;

/// Bits of a small position difference (±4 m at 1/512 m).
const POS_DIFF_BITS: u32 = 12;
/// Bits of a small velocity difference (±8 m/s at 1/16 m/s).
const VEL_DIFF_BITS: u32 = 8;

fn fits(diff: i64, bits: u32) -> bool {
    let half = 1i64 << (bits - 1);
    (-half..half).contains(&diff)
}

fn write_signed_bits(w: &mut BitWriter, v: i64, bits: u32) {
    w.write_bits(v as u64 & ((1u64 << bits) - 1), bits);
}

fn read_signed_bits(r: &mut BitReader, bits: u32) -> Result<i64, DecodeError> {
    let raw = r.read_bits(bits)?;
    let shift = 64 - bits;
    Ok(((raw << shift) as i64) >> shift)
}

/// The difference of a position or velocity axis, if `index` is one.
fn axis(e: &QuantizedEntity, index: usize) -> Option<(i64, u32)> {
    match index {
        0..=2 => Some((i64::from(e.position[index]), POS_DIFF_BITS)),
        3..=5 => Some((i64::from(e.velocity[index - 3]), VEL_DIFF_BITS)),
        _ => None,
    }
}

/// Writes a changed field of `new` against `old` (see the module docs).
fn write_changed_field(w: &mut BitWriter, old: &QuantizedEntity, new: &QuantizedEntity, f: usize) {
    if let (Some((a, bits)), Some((b, _))) = (axis(old, f), axis(new, f)) {
        let diff = b - a;
        if fits(diff, bits) {
            w.write_bool(true);
            write_signed_bits(w, diff, bits);
            return;
        }
        w.write_bool(false);
    }
    write_field(w, new, f);
}

/// Reads a changed field into `e`, which holds the baseline values.
fn read_changed_field(
    r: &mut BitReader,
    e: &mut QuantizedEntity,
    f: usize,
) -> Result<(), DecodeError> {
    let Some((old, bits)) = axis(e, f) else {
        return read_field(r, e, f);
    };
    if !r.read_bool()? {
        return read_field(r, e, f);
    }
    let value = old + read_signed_bits(r, bits)?;
    match f {
        0..=2 => {
            if !fits(value, faset_protocol::quant::POS_BITS) {
                return Err(DecodeError::OutOfRange);
            }
            e.position[f] = value as i32;
        }
        _ => {
            e.velocity[f - 3] = i16::try_from(value).map_err(|_| DecodeError::OutOfRange)?;
        }
    }
    Ok(())
}

/// Encodes `current` as a delta against `baseline`.
///
/// # Errors
/// [`SnapshotError::TooManyEntities`] when `current` or the record lists
/// exceed [`MAX_SNAPSHOT_ENTITIES`].
pub fn encode_delta(
    baseline: &QuantizedSnapshot,
    current: &QuantizedSnapshot,
) -> Result<Vec<u8>, SnapshotError> {
    if current.entities.len() > MAX_SNAPSHOT_ENTITIES {
        return Err(SnapshotError::TooManyEntities(current.entities.len()));
    }
    let mut removed = Vec::new();
    let mut records: Vec<(&QuantizedEntity, Option<(u16, &QuantizedEntity)>)> = Vec::new();
    let (mut b, mut c) = (
        baseline.entities.iter().peekable(),
        current.entities.iter().peekable(),
    );
    loop {
        match (b.peek(), c.peek()) {
            (Some(old), Some(new)) if old.id == new.id => {
                let mask = (0..ENTITY_FIELDS)
                    .filter(|&f| field_differs(old, new, f))
                    .fold(0u16, |m, f| m | (1 << f));
                if mask != 0 {
                    records.push((new, Some((mask, old))));
                }
                b.next();
                c.next();
            }
            (Some(old), Some(new)) if old.id < new.id => {
                removed.push(old.id);
                b.next();
            }
            (Some(old), None) => {
                removed.push(old.id);
                b.next();
            }
            (_, Some(new)) => {
                records.push((new, None));
                c.next();
            }
            (None, None) => break,
        }
    }
    // Removals are bounded by the baseline, which may come from a peer that
    // is not this encoder; keep the 8-bit counts honest.
    if removed.len() > MAX_SNAPSHOT_ENTITIES {
        return Err(SnapshotError::TooManyEntities(removed.len()));
    }
    let mut w = BitWriter::new();
    write_last_command(&mut w, current.last_command);
    w.write_bits(removed.len() as u64, 8);
    for id in &removed {
        w.write_bits(u64::from(id.0), 16);
    }
    w.write_bits(records.len() as u64, 8);
    for (e, mask) in records {
        w.write_bits(u64::from(e.id.0), 16);
        match mask {
            None => {
                w.write_bool(true);
                for f in 0..ENTITY_FIELDS {
                    write_field(&mut w, e, f);
                }
            }
            Some((mask, old)) => {
                w.write_bool(false);
                w.write_bits(u64::from(mask), ENTITY_FIELDS as u32);
                for f in (0..ENTITY_FIELDS).filter(|f| mask & (1 << f) != 0) {
                    write_changed_field(&mut w, old, e, f);
                }
            }
        }
    }
    Ok(w.finish())
}

fn read_ascending_ids(
    r: &mut BitReader,
    count: usize,
    out: &mut Vec<NetId>,
) -> Result<(), DecodeError> {
    for _ in 0..count {
        let id = NetId(r.read_bits(16)? as u16);
        if out.last().is_some_and(|&l| id <= l) {
            return Err(DecodeError::OutOfRange);
        }
        out.push(id);
    }
    Ok(())
}

/// Rebuilds the snapshot at `tick` from `baseline` and a delta payload.
///
/// # Errors
/// [`SnapshotError::Decode`] for malformed or inconsistent input,
/// [`SnapshotError::TooManyEntities`] when the result is too large.
pub fn decode_delta(
    baseline: &QuantizedSnapshot,
    tick: Tick,
    data: &[u8],
) -> Result<QuantizedSnapshot, SnapshotError> {
    let mut r = BitReader::new(data);
    let last_command = read_last_command(&mut r)?;
    let removed_count = r.read_bits(8)? as usize;
    let mut removed = Vec::with_capacity(removed_count);
    read_ascending_ids(&mut r, removed_count, &mut removed)?;

    let mut entities: Vec<QuantizedEntity> = Vec::with_capacity(baseline.entities.len());
    let mut gone = removed.iter().peekable();
    for e in &baseline.entities {
        if gone.peek() == Some(&&e.id) {
            gone.next();
        } else {
            entities.push(*e);
        }
    }
    if gone.next().is_some() {
        // A removed id the baseline does not hold.
        return Err(DecodeError::OutOfRange.into());
    }

    let record_count = r.read_bits(8)? as usize;
    let mut last: Option<NetId> = None;
    for _ in 0..record_count {
        let id = NetId(r.read_bits(16)? as u16);
        if last.is_some_and(|l| id <= l) {
            return Err(DecodeError::OutOfRange.into());
        }
        last = Some(id);
        let slot = entities.binary_search_by_key(&id, |e| e.id);
        if r.read_bool()? {
            let Err(at) = slot else {
                return Err(DecodeError::OutOfRange.into());
            };
            let mut e = empty_entity(id);
            for f in 0..ENTITY_FIELDS {
                read_field(&mut r, &mut e, f)?;
            }
            entities.insert(at, e);
        } else {
            let Ok(at) = slot else {
                return Err(DecodeError::OutOfRange.into());
            };
            let mask = r.read_bits(ENTITY_FIELDS as u32)? as u16;
            if mask == 0 {
                return Err(DecodeError::OutOfRange.into());
            }
            for f in (0..ENTITY_FIELDS).filter(|f| mask & (1 << f) != 0) {
                read_changed_field(&mut r, &mut entities[at], f)?;
            }
        }
        if entities.len() > MAX_SNAPSHOT_ENTITIES {
            return Err(SnapshotError::TooManyEntities(entities.len()));
        }
    }
    r.finish()?;
    Ok(QuantizedSnapshot {
        tick,
        last_command,
        entities,
    })
}

/// Snapshots kept by tick, newest last, at most [`REFERENCE_SNAPSHOTS`].
#[derive(Debug, Clone, Default)]
struct Ring {
    items: VecDeque<QuantizedSnapshot>,
}

impl Ring {
    fn push(&mut self, s: QuantizedSnapshot) {
        if self.items.len() == REFERENCE_SNAPSHOTS {
            self.items.pop_front();
        }
        self.items.push_back(s);
    }

    fn get(&self, tick: Tick) -> Option<&QuantizedSnapshot> {
        self.items.iter().rev().find(|s| s.tick == tick)
    }
}

/// Server side, one per client: the reference ring of sent snapshots and
/// the client's acknowledgement.
#[derive(Debug, Clone, Default)]
pub struct SnapshotEncoder {
    sent: Ring,
    acked: Option<Tick>,
}

impl SnapshotEncoder {
    /// An encoder that has sent nothing: the first snapshot is full.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records that the client received the snapshot at `tick`. Ignored
    /// (returns `false`) unless that snapshot is still in the ring and newer
    /// than the current acknowledgement.
    pub fn acknowledge(&mut self, tick: Tick) -> bool {
        if self.acked.is_some_and(|a| tick <= a) || self.sent.get(tick).is_none() {
            return false;
        }
        self.acked = Some(tick);
        true
    }

    /// The acknowledged tick, if any.
    #[must_use]
    pub fn acknowledged(&self) -> Option<Tick> {
        self.acked
    }

    /// The baseline the next snapshot would be encoded against: the
    /// acknowledged snapshot while it is still in the ring.
    #[must_use]
    pub fn baseline(&self) -> Option<&QuantizedSnapshot> {
        self.sent.get(self.acked?)
    }

    /// Encodes `snapshot` as a delta against the baseline, or as a full
    /// snapshot when there is none or the full one is not larger, and keeps
    /// it as a future reference.
    ///
    /// # Errors
    /// [`SnapshotError::TooManyEntities`].
    pub fn encode(&mut self, snapshot: &Snapshot) -> Result<Message, SnapshotError> {
        let current = QuantizedSnapshot::from_snapshot(snapshot);
        let full = current.encode_full()?;
        let message = match self.baseline() {
            Some(base) => {
                let delta = encode_delta(base, &current)?;
                if delta.len() < full.len() {
                    Message::SnapshotDelta {
                        tick: current.tick,
                        baseline: base.tick,
                        data: delta,
                    }
                } else {
                    Message::SnapshotFull {
                        tick: current.tick,
                        data: full,
                    }
                }
            }
            None => Message::SnapshotFull {
                tick: current.tick,
                data: full,
            },
        };
        self.sent.push(current);
        Ok(message)
    }
}

/// Client side: the last snapshots received, to resolve delta baselines,
/// and the acknowledgement to send back.
#[derive(Debug, Clone, Default)]
pub struct SnapshotDecoder {
    received: Ring,
    newest: Option<Tick>,
}

impl SnapshotDecoder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Decodes a full or delta snapshot message and keeps the result as a
    /// future baseline.
    ///
    /// # Errors
    /// [`SnapshotError::WrongMessage`] for a non-snapshot message,
    /// [`SnapshotError::MissingBaseline`] when the delta's baseline is not
    /// held, or a decode error.
    pub fn decode(&mut self, message: &Message) -> Result<Snapshot, SnapshotError> {
        let q = match message {
            Message::SnapshotFull { tick, data } => QuantizedSnapshot::decode_full(*tick, data)?,
            Message::SnapshotDelta {
                tick,
                baseline,
                data,
            } => {
                let base = self
                    .received
                    .get(*baseline)
                    .ok_or(SnapshotError::MissingBaseline(*baseline))?;
                decode_delta(base, *tick, data)?
            }
            _ => return Err(SnapshotError::WrongMessage),
        };
        let out = q.to_snapshot();
        if self.newest.is_none_or(|n| q.tick > n) {
            self.newest = Some(q.tick);
        }
        self.received.push(q);
        Ok(out)
    }

    /// Decodes a wire message (see [`decode`](Self::decode)).
    ///
    /// # Errors
    /// See [`decode`](Self::decode).
    pub fn decode_bytes(&mut self, bytes: &[u8]) -> Result<Snapshot, SnapshotError> {
        self.decode(&Message::decode(bytes)?)
    }

    /// Tick of the newest snapshot received: the acknowledgement to send in
    /// the next [`InputPacket`].
    #[must_use]
    pub fn ack(&self) -> Option<Tick> {
        self.newest
    }
}

/// A channel 0 packet: one input command plus the snapshot acknowledgement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputPacket {
    pub command: InputCommand,
    /// Tick of the newest snapshot the client has received.
    pub ack: Option<Tick>,
}

impl InputPacket {
    /// Size with an acknowledgement.
    pub const SIZE: usize = InputCommand::SIZE + 4;

    /// 13 bytes without an acknowledgement, 17 with one.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = self.command.encode().to_vec();
        if let Some(t) = self.ack {
            out.extend_from_slice(&t.0.to_le_bytes());
        }
        out
    }

    /// # Errors
    /// [`DecodeError`] unless `bytes` is a valid 13- or 17-byte packet.
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let (cmd, ack) = match bytes.len() {
            InputCommand::SIZE => (bytes, None),
            Self::SIZE => {
                let (cmd, ack) = bytes.split_at(InputCommand::SIZE);
                let ack: [u8; 4] = ack.try_into().map_err(|_| DecodeError::Truncated)?;
                (cmd, Some(Tick(u32::from_le_bytes(ack))))
            }
            n if n < Self::SIZE => return Err(DecodeError::Truncated),
            _ => return Err(DecodeError::TrailingData),
        };
        Ok(Self {
            command: InputCommand::decode(cmd)?,
            ack,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::EntityState;
    use faset_core::glam::Vec3;
    use proptest::prelude::*;
    use std::collections::BTreeMap;

    fn entity(id: u16, x: f32) -> EntityState {
        EntityState {
            id: NetId(id),
            position: Vec3::new(x, 1.0, -2.0),
            velocity: Vec3::new(5.0, 0.0, 0.0),
            yaw: 1.0,
            pitch: 0.2,
            health: 100,
            weapon: 2,
            on_ground: true,
            sliding: false,
            alive: true,
        }
    }

    fn snap(tick: u32, entities: Vec<EntityState>) -> Snapshot {
        Snapshot {
            tick: Tick(tick),
            last_command: Some(Tick(tick.saturating_sub(2))),
            entities,
        }
    }

    fn sixteen(tick: u32) -> Snapshot {
        snap(
            tick,
            (1..=16)
                .map(|i| entity(i, tick as f32 * 0.1 + f32::from(i)))
                .collect(),
        )
    }

    #[test]
    fn first_snapshot_is_full_then_deltas_after_ack() {
        let mut enc = SnapshotEncoder::new();
        let mut dec = SnapshotDecoder::new();
        let m = enc.encode(&sixteen(2)).unwrap();
        assert!(matches!(m, Message::SnapshotFull { .. }));
        dec.decode(&m).unwrap();
        assert!(enc.acknowledge(dec.ack().unwrap()));
        let s = sixteen(4);
        let m = enc.encode(&s).unwrap();
        let Message::SnapshotDelta { baseline, data, .. } = &m else {
            panic!("expected a delta, got {m:?}");
        };
        assert_eq!(*baseline, Tick(2));
        // 16 entities, each with x changed only by 0.2 m: 16 + 1 + 11 bits,
        // then a small difference (1 + 12 bits).
        assert_eq!(data.len(), (1 + 32 + 8 + 8 + 16 * 41usize).div_ceil(8));
        // A long move sends the full axis (1 + 24 bits).
        let far = snap(
            4,
            (1..=16).map(|i| entity(i, 100.0 + f32::from(i))).collect(),
        );
        let Message::SnapshotDelta { data, .. } = enc.encode(&far).unwrap() else {
            panic!("expected a delta");
        };
        assert_eq!(data.len(), (1 + 32 + 8 + 8 + 16 * 53usize).div_ceil(8));
        assert_eq!(
            dec.decode(&m).unwrap(),
            QuantizedSnapshot::from_snapshot(&s).to_snapshot()
        );
    }

    #[test]
    fn ack_older_than_the_ring_falls_back_to_full() {
        let mut enc = SnapshotEncoder::new();
        enc.encode(&sixteen(0)).unwrap();
        assert!(enc.acknowledge(Tick(0)));
        // 63 more fit with tick 0 still kept: deltas.
        for t in 1..REFERENCE_SNAPSHOTS as u32 {
            let m = enc.encode(&sixteen(t)).unwrap();
            assert!(
                matches!(
                    m,
                    Message::SnapshotDelta {
                        baseline: Tick(0),
                        ..
                    }
                ),
                "{t}"
            );
        }
        // The 65th pushes tick 0 out of the ring.
        let m = enc.encode(&sixteen(REFERENCE_SNAPSHOTS as u32)).unwrap();
        assert!(matches!(m, Message::SnapshotDelta { .. }));
        assert!(enc.baseline().is_none());
        let m = enc
            .encode(&sixteen(REFERENCE_SNAPSHOTS as u32 + 1))
            .unwrap();
        assert!(matches!(m, Message::SnapshotFull { .. }), "{m:?}");
        // An acknowledgement that arrives after its snapshot left the ring
        // is ignored, and the server keeps sending full snapshots.
        assert!(!enc.acknowledge(Tick(1)));
        let m = enc
            .encode(&sixteen(REFERENCE_SNAPSHOTS as u32 + 2))
            .unwrap();
        assert!(matches!(m, Message::SnapshotFull { .. }));
        // Unknown and stale acknowledgements are ignored too.
        assert!(!enc.acknowledge(Tick(9999)));
        assert!(enc.acknowledge(Tick(REFERENCE_SNAPSHOTS as u32 + 2)));
        assert!(!enc.acknowledge(Tick(REFERENCE_SNAPSHOTS as u32 + 1)));
    }

    #[test]
    fn creation_and_removal_are_explicit() {
        let base = QuantizedSnapshot::from_snapshot(&snap(1, vec![entity(1, 0.0), entity(3, 0.0)]));
        let cur = QuantizedSnapshot::from_snapshot(&snap(2, vec![entity(2, 5.0), entity(3, 0.0)]));
        let data = encode_delta(&base, &cur).unwrap();
        assert_eq!(decode_delta(&base, Tick(2), &data).unwrap(), cur);
        // 1 removed, 2 created, 3 unchanged (no record).
        let bits = 1 + 32 + 8 + 16 + 8 + 16 + 1 + 163;
        assert_eq!(data.len(), usize::div_ceil(bits, 8));
    }

    #[test]
    fn missing_baseline_is_reported() {
        let mut dec = SnapshotDecoder::new();
        let m = Message::SnapshotDelta {
            tick: Tick(5),
            baseline: Tick(3),
            data: vec![0, 0, 0],
        };
        assert_eq!(dec.decode(&m), Err(SnapshotError::MissingBaseline(Tick(3))));
    }

    #[test]
    fn input_packet_roundtrip() {
        let command = InputCommand {
            tick: Tick(77),
            buttons: 3,
            move_x: -5,
            move_y: 127,
            yaw: 1234,
            pitch: -40,
            weapon_slot: 2,
        };
        for ack in [None, Some(Tick(70)), Some(Tick(u32::MAX))] {
            let p = InputPacket { command, ack };
            let bytes = p.encode();
            assert_eq!(bytes.len(), if ack.is_some() { 17 } else { 13 });
            assert_eq!(InputPacket::decode(&bytes), Ok(p));
        }
        assert!(InputPacket::decode(&[0; 15]).is_err());
        assert!(InputPacket::decode(&[0; 18]).is_err());
    }

    /// One world mutation: (entity id, operation, value).
    /// Operation 0 removes, 1 (re)creates from `value`, 2..=12 sets one field.
    type Op = (u16, u8, u32);

    fn apply(world: &mut BTreeMap<u16, EntityState>, (id, op, v): Op) {
        let f = |range: f32| (v % 20_001) as f32 / 10_000.0 * range - range;
        match op {
            0 => {
                world.remove(&id);
            }
            1 => {
                let mut e = entity(id, f(500.0));
                e.velocity.z = f(40.0);
                e.health = v as u8;
                world.insert(id, e);
            }
            _ => {
                let Some(e) = world.get_mut(&id) else { return };
                match op {
                    2 => e.position.x = f(1000.0),
                    3 => e.position.y = f(100.0),
                    4 => e.position.z = f(1000.0),
                    5 => e.velocity.x = f(50.0),
                    6 => e.velocity.y = f(50.0),
                    7 => e.velocity.z = f(50.0),
                    8 => e.yaw = f(7.0),
                    9 => e.pitch = f(1.5),
                    10 => e.health = v as u8,
                    11 => e.weapon = (v % 16) as u8,
                    _ => {
                        e.on_ground = v & 1 != 0;
                        e.sliding = v & 2 != 0;
                        e.alive = v & 4 != 0;
                    }
                }
            }
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(128))]

        /// Random world sequences over a lossy link with late, lost and
        /// reordered acknowledgements: every snapshot the client decodes
        /// equals the server's quantized state exactly.
        #[test]
        fn delta_reconstructs_quantized_state_exactly(
            frames in proptest::collection::vec(
                (proptest::collection::vec((1u16..20, 0u8..13, any::<u32>()), 0..8),
                 any::<bool>(), 0u8..4, any::<Option<u32>>()),
                1..160),
        ) {
            let mut world = BTreeMap::new();
            let mut enc = SnapshotEncoder::new();
            let mut dec = SnapshotDecoder::new();
            let mut acks: VecDeque<Tick> = VecDeque::new();
            for (tick, (ops, delivered, ack_delay, last_command)) in frames.into_iter().enumerate() {
                for op in ops {
                    apply(&mut world, op);
                }
                let s = Snapshot {
                    tick: Tick(tick as u32),
                    last_command: last_command.map(Tick),
                    entities: world.values().copied().collect(),
                };
                let msg = enc.encode(&s).unwrap();
                // The wire message stays within the protocol limits.
                prop_assert!(msg.encode().is_ok());
                if delivered {
                    let got = dec.decode(&msg).unwrap();
                    prop_assert_eq!(got, QuantizedSnapshot::from_snapshot(&s).to_snapshot());
                    if let Some(a) = dec.ack() {
                        acks.push_back(a);
                    }
                }
                // Acks arrive 0 to 3 frames late, sometimes not at all.
                if ack_delay == 3 {
                    acks.pop_front();
                }
                while acks.len() > usize::from(ack_delay) {
                    if let Some(a) = acks.pop_front() {
                        enc.acknowledge(a);
                    }
                }
            }
        }

        #[test]
        fn decode_delta_never_panics(data in proptest::collection::vec(any::<u8>(), 0..300)) {
            let base = QuantizedSnapshot::from_snapshot(&sixteen(1));
            let _ = decode_delta(&base, Tick(2), &data);
        }
    }
}

//! Framing between `renet` packets and transport datagrams.
//!
//! `renet` slices large messages at 1200 bytes of payload, so its packets can
//! reach about 1215 bytes. Every datagram must stay within
//! [`MAX_PACKET_SIZE`], so each `renet` packet travels in a one-byte-tagged
//! frame, split into fragments when it does not fit. The same tag space
//! carries a payload-free keep-alive.
//!
//! Frame layouts (all integers little-endian):
//!
//! * `[0] packet`: one whole `renet` packet follows.
//! * `[1, id: u16, index: u8, count: u8] fragment`: part of a `renet` packet.
//! * `[2] keep-alive`: nothing follows; makes a silent peer known and alive.

use std::collections::HashMap;
use std::time::Duration;

use crate::MAX_PACKET_SIZE;

const KIND_PACKET: u8 = 0;
const KIND_FRAGMENT: u8 = 1;
const KIND_KEEPALIVE: u8 = 2;

const FRAGMENT_HEADER: usize = 5;
const FRAGMENT_PAYLOAD: usize = MAX_PACKET_SIZE - FRAGMENT_HEADER;
/// Partial packets kept per peer; the oldest is evicted beyond this.
const MAX_PARTIAL: usize = 32;
/// A partial packet missing a fragment for this long is abandoned.
const FRAGMENT_TTL: Duration = Duration::from_secs(2);

/// Result of decoding one received frame.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Incoming {
    /// A complete `renet` packet.
    Packet(Vec<u8>),
    /// A keep-alive.
    Keepalive,
    /// A valid fragment, packet not complete yet.
    Partial,
    /// Not a frame of this protocol.
    Invalid,
}

#[derive(Debug)]
struct Partial {
    parts: Vec<Option<Vec<u8>>>,
    missing: usize,
    last_seen: Duration,
}

/// Per-remote framing state.
#[derive(Debug, Default)]
pub(crate) struct Link {
    partial: HashMap<u16, Partial>,
    next_fragment_id: u16,
    /// Time of the last valid frame received.
    pub last_recv: Duration,
    /// Time of the last frame sent.
    pub last_send: Duration,
}

impl Link {
    pub fn new(now: Duration) -> Self {
        Self {
            last_recv: now,
            last_send: now,
            ..Self::default()
        }
    }

    /// Wraps one `renet` packet into frames of at most [`MAX_PACKET_SIZE`].
    pub fn encode(&mut self, packet: &[u8], now: Duration) -> Vec<Vec<u8>> {
        self.last_send = now;
        if packet.len() < MAX_PACKET_SIZE {
            let mut frame = Vec::with_capacity(packet.len() + 1);
            frame.push(KIND_PACKET);
            frame.extend_from_slice(packet);
            return vec![frame];
        }
        let count = packet.len().div_ceil(FRAGMENT_PAYLOAD);
        let Ok(count) = u8::try_from(count) else {
            tracing::error!(len = packet.len(), "packet too large to fragment; dropped");
            return Vec::new();
        };
        let id = self.next_fragment_id;
        self.next_fragment_id = id.wrapping_add(1);
        packet
            .chunks(FRAGMENT_PAYLOAD)
            .zip(0u8..)
            .map(|(chunk, index)| {
                let mut frame = Vec::with_capacity(FRAGMENT_HEADER + chunk.len());
                frame.push(KIND_FRAGMENT);
                frame.extend_from_slice(&id.to_le_bytes());
                frame.push(index);
                frame.push(count);
                frame.extend_from_slice(chunk);
                frame
            })
            .collect()
    }

    /// The keep-alive frame.
    pub fn keepalive(&mut self, now: Duration) -> Vec<u8> {
        self.last_send = now;
        vec![KIND_KEEPALIVE]
    }

    /// Decodes a received frame, reassembling fragments.
    pub fn decode(&mut self, frame: &[u8], now: Duration) -> Incoming {
        let result = match frame.split_first() {
            Some((&KIND_PACKET, rest)) if !rest.is_empty() => Incoming::Packet(rest.to_vec()),
            Some((&KIND_KEEPALIVE, [])) => Incoming::Keepalive,
            Some((&KIND_FRAGMENT, rest)) => self.decode_fragment(rest, now),
            _ => Incoming::Invalid,
        };
        if result != Incoming::Invalid {
            self.last_recv = now;
        }
        result
    }

    fn decode_fragment(&mut self, rest: &[u8], now: Duration) -> Incoming {
        let [id0, id1, index, count, payload @ ..] = rest else {
            return Incoming::Invalid;
        };
        let id = u16::from_le_bytes([*id0, *id1]);
        let (index, count) = (usize::from(*index), usize::from(*count));
        if count < 2 || index >= count || payload.is_empty() {
            return Incoming::Invalid;
        }
        if self
            .partial
            .get(&id)
            .is_some_and(|p| p.parts.len() != count)
        {
            self.partial.remove(&id);
        }
        if !self.partial.contains_key(&id) && self.partial.len() >= MAX_PARTIAL {
            let oldest = self
                .partial
                .iter()
                .min_by_key(|(_, p)| p.last_seen)
                .map(|(k, _)| *k);
            if let Some(oldest) = oldest {
                self.partial.remove(&oldest);
            }
        }
        let entry = self.partial.entry(id).or_insert_with(|| Partial {
            parts: vec![None; count],
            missing: count,
            last_seen: now,
        });
        entry.last_seen = now;
        if entry.parts[index].is_none() {
            entry.parts[index] = Some(payload.to_vec());
            entry.missing -= 1;
        }
        if entry.missing > 0 {
            return Incoming::Partial;
        }
        match self.partial.remove(&id) {
            Some(done) => Incoming::Packet(done.parts.into_iter().flatten().flatten().collect()),
            None => Incoming::Partial,
        }
    }

    /// Drops partial packets that stopped receiving fragments.
    pub fn expire(&mut self, now: Duration) {
        self.partial
            .retain(|_, p| now.saturating_sub(p.last_seen) < FRAGMENT_TTL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: Duration = Duration::ZERO;

    fn pattern(len: usize) -> Vec<u8> {
        (0..len).map(|i| u8::try_from(i % 251).unwrap()).collect()
    }

    #[test]
    fn small_packet_is_one_frame() {
        let mut link = Link::default();
        let packet = pattern(MAX_PACKET_SIZE - 1);
        let frames = link.encode(&packet, T0);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].len(), MAX_PACKET_SIZE);
        assert_eq!(link.decode(&frames[0], T0), Incoming::Packet(packet));
    }

    #[test]
    fn oversized_packets_fragment_within_the_limit_and_reassemble_in_any_order() {
        for len in [MAX_PACKET_SIZE, 1215, 2400, 5000] {
            let mut tx = Link::default();
            let mut rx = Link::default();
            let packet = pattern(len);
            let mut frames = tx.encode(&packet, T0);
            assert!(frames.len() >= 2);
            assert!(frames.iter().all(|f| f.len() <= MAX_PACKET_SIZE));
            frames.reverse();
            let mut done = None;
            for f in &frames {
                match rx.decode(f, T0) {
                    Incoming::Packet(p) => done = Some(p),
                    Incoming::Partial => {}
                    other => panic!("unexpected {other:?}"),
                }
            }
            assert_eq!(done, Some(packet), "len {len}");
        }
    }

    #[test]
    fn duplicate_fragments_do_not_complete_a_packet_early() {
        let mut tx = Link::default();
        let mut rx = Link::default();
        let frames = tx.encode(&pattern(2400), T0);
        assert_eq!(rx.decode(&frames[0], T0), Incoming::Partial);
        assert_eq!(rx.decode(&frames[0], T0), Incoming::Partial);
        assert!(matches!(
            rx.decode(&frames[1], T0),
            Incoming::Partial | Incoming::Packet(_)
        ));
    }

    #[test]
    fn garbage_is_invalid_and_never_panics() {
        let mut link = Link::default();
        for frame in [
            &[][..],
            &[0],
            &[1],
            &[1, 0, 0],
            &[1, 0, 0, 5, 3, 9],
            &[1, 0, 0, 0, 1, 9],
            &[2, 0],
            &[3, 1, 2],
            &[255; 40],
        ] {
            assert_eq!(link.decode(frame, T0), Incoming::Invalid, "{frame:?}");
        }
        assert_eq!(link.decode(&[2], T0), Incoming::Keepalive);
    }

    #[test]
    fn stale_partials_expire_and_are_bounded() {
        let mut tx = Link::default();
        let mut rx = Link::default();
        for _ in 0..(MAX_PARTIAL + 10) {
            let frames = tx.encode(&pattern(2400), T0);
            rx.decode(&frames[0], T0);
        }
        assert!(rx.partial.len() <= MAX_PARTIAL);
        rx.expire(FRAGMENT_TTL + Duration::from_millis(1));
        assert!(rx.partial.is_empty());
    }

    #[test]
    fn valid_frames_refresh_the_receive_clock() {
        let mut link = Link::new(T0);
        let now = Duration::from_secs(3);
        assert_eq!(link.decode(&[7], now), Incoming::Invalid);
        assert_eq!(link.last_recv, T0);
        assert_eq!(link.decode(&[2], now), Incoming::Keepalive);
        assert_eq!(link.last_recv, now);
    }
}

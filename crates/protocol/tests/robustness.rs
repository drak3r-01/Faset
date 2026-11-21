//! Lot 1.10: the decoder survives random and truncated input. Decoding
//! returns `Err` (or a valid message), never panics.

use faset_core::{NetId, Tick};
use faset_protocol::message::Event;
use faset_protocol::{InputCommand, Message};
use proptest::prelude::*;

/// Cases per property; the three properties below run 3 x this many inputs.
const CASES: u32 = 10_000;

fn valid_messages() -> Vec<Message> {
    vec![
        Message::ClientHello {
            protocol_version: 1,
            build_fingerprint: 0xDEAD_BEEF_CAFE_F00D,
        },
        Message::ServerHello {
            mode: 2,
            player_id: NetId(3),
            map_fingerprint: 77,
            tick_rate: 60,
        },
        Message::Ready,
        Message::SnapshotFull {
            tick: Tick(9),
            data: vec![1, 2, 3, 4, 5, 6, 7, 8],
        },
        Message::SnapshotDelta {
            tick: Tick(10),
            baseline: Tick(9),
            data: vec![9; 40],
        },
        Message::Event(Event::Chat {
            sender: NetId(1),
            text: "hello there".into(),
        }),
        Message::Event(Event::Death {
            victim: NetId(1),
            killer: NetId(2),
            weapon: 3,
        }),
        Message::MapChunk {
            index: 1,
            total: 4,
            data: vec![0xAB; 200],
        },
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(CASES))]

    /// Arbitrary bytes never panic the message decoder.
    #[test]
    fn random_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..1500)) {
        let _ = Message::decode(&bytes);
    }

    /// Every strict prefix of a valid message decodes to `Err` or to some
    /// valid message (a prefix can be a shorter message), and never panics;
    /// a flipped byte never panics either.
    #[test]
    fn truncated_and_corrupted_messages_never_panic(
        which in 0usize..8,
        cut in 0usize..400,
        flip in any::<(usize, u8)>(),
    ) {
        let bytes = valid_messages()[which].encode().unwrap();
        let cut = cut.min(bytes.len().saturating_sub(1));
        let _ = Message::decode(&bytes[..cut]);
        let mut corrupted = bytes.clone();
        let at = flip.0 % corrupted.len();
        corrupted[at] ^= flip.1;
        let _ = Message::decode(&corrupted);
    }

    /// Fixed-size input commands: every wrong length is an error.
    #[test]
    fn input_commands_of_the_wrong_length_are_errors(
        bytes in proptest::collection::vec(any::<u8>(), 0..64),
    ) {
        let r = InputCommand::decode(&bytes);
        if bytes.len() != InputCommand::SIZE {
            prop_assert!(r.is_err());
        }
    }
}

#[test]
fn every_prefix_of_a_fixed_size_message_is_an_error() {
    for m in [
        Message::ClientHello {
            protocol_version: 1,
            build_fingerprint: 5,
        },
        Message::ServerHello {
            mode: 0,
            player_id: NetId(0),
            map_fingerprint: 1,
            tick_rate: 60,
        },
    ] {
        let bytes = m.encode().unwrap();
        for n in 0..bytes.len() {
            assert!(Message::decode(&bytes[..n]).is_err(), "{m:?} cut at {n}");
        }
    }
}

#[test]
fn oversized_packets_are_rejected() {
    assert!(Message::decode(&vec![0u8; 100_000]).is_err());
}

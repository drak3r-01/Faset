//! The four network channels and their `renet` configuration.

use std::time::Duration;

use renet::{ChannelConfig, ConnectionConfig, SendType};

/// Largest payload accepted on [`Channel::MapTransfer`] (decision: 4 MiB; a
/// map is a few hundred KiB).
pub const MAX_MAP_PAYLOAD: usize = 4 * 1024 * 1024;

/// How long a reliable channel waits for an ack before resending.
const RESEND_TIME: Duration = Duration::from_millis(300);

/// Bytes `renet` may put on the wire per `update` call and connection
/// (decision: 60 000, `renet`'s default; 28.8 Mbit/s at 60 Hz).
const BYTES_PER_UPDATE: u64 = 60_000;

/// A network channel. Ids match the specification table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Channel {
    /// 0: client to server, unreliable. Input commands.
    Input = 0,
    /// 1: server to client, unreliable. Snapshots.
    Snapshot = 1,
    /// 2: both directions, reliable and ordered. Events: deaths, score,
    /// phases, map change, chat.
    Event = 2,
    /// 3: server to client, reliable, chunked. Map transfer.
    MapTransfer = 3,
}

impl Channel {
    /// All channels in id order.
    pub const ALL: [Channel; 4] = [
        Channel::Input,
        Channel::Snapshot,
        Channel::Event,
        Channel::MapTransfer,
    ];

    /// Numeric channel id (0 to 3).
    #[must_use]
    pub fn id(self) -> u8 {
        self as u8
    }

    /// Channel for a numeric id.
    #[must_use]
    pub fn from_id(id: u8) -> Option<Self> {
        Self::ALL.get(usize::from(id)).copied()
    }

    /// Whether messages are guaranteed to arrive.
    #[must_use]
    pub fn is_reliable(self) -> bool {
        matches!(self, Channel::Event | Channel::MapTransfer)
    }

    /// Whether the client may send on this channel.
    #[must_use]
    pub fn client_may_send(self) -> bool {
        matches!(self, Channel::Input | Channel::Event)
    }

    /// Whether the server may send on this channel.
    #[must_use]
    pub fn server_may_send(self) -> bool {
        matches!(
            self,
            Channel::Snapshot | Channel::Event | Channel::MapTransfer
        )
    }
}

fn unreliable(id: u8) -> ChannelConfig {
    ChannelConfig {
        channel_id: id,
        max_memory_usage_bytes: 1024 * 1024,
        send_type: SendType::Unreliable,
    }
}

fn reliable_ordered(id: u8, memory: usize) -> ChannelConfig {
    ChannelConfig {
        channel_id: id,
        max_memory_usage_bytes: memory,
        send_type: SendType::ReliableOrdered {
            resend_time: RESEND_TIME,
        },
    }
}

/// `renet` configuration shared by client and server.
///
/// Channel order is priority order: on the server, snapshots go first, then
/// events, and the map transfer uses what is left.
pub(crate) fn connection_config() -> ConnectionConfig {
    ConnectionConfig {
        available_bytes_per_tick: BYTES_PER_UPDATE,
        server_channels_config: vec![
            unreliable(Channel::Snapshot.id()),
            reliable_ordered(Channel::Event.id(), 4 * 1024 * 1024),
            reliable_ordered(Channel::MapTransfer.id(), 2 * MAX_MAP_PAYLOAD),
        ],
        client_channels_config: vec![
            unreliable(Channel::Input.id()),
            reliable_ordered(Channel::Event.id(), 4 * 1024 * 1024),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_match_the_table() {
        for (i, ch) in Channel::ALL.into_iter().enumerate() {
            assert_eq!(usize::from(ch.id()), i);
            assert_eq!(Channel::from_id(ch.id()), Some(ch));
        }
        assert_eq!(Channel::from_id(4), None);
    }

    #[test]
    fn directions_and_reliability_match_the_table() {
        assert!(Channel::Input.client_may_send() && !Channel::Input.server_may_send());
        assert!(Channel::Snapshot.server_may_send() && !Channel::Snapshot.client_may_send());
        assert!(Channel::Event.client_may_send() && Channel::Event.server_may_send());
        assert!(Channel::MapTransfer.server_may_send() && !Channel::MapTransfer.client_may_send());
        let reliable: Vec<_> = Channel::ALL.map(Channel::is_reliable).to_vec();
        assert_eq!(reliable, [false, false, true, true]);
    }
}

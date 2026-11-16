//! The datagram transport abstraction.

use std::fmt::Debug;
use std::hash::Hash;
use std::time::Duration;

/// Maximum size in bytes of one datagram handed to a [`Transport`].
///
/// Keeps every packet below the usual path MTU so IP never fragments it.
pub const MAX_PACKET_SIZE: usize = 1200;

/// One received datagram and the address it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Datagram<A> {
    /// Sender address.
    pub from: A,
    /// Packet bytes, at most [`MAX_PACKET_SIZE`] long.
    pub data: Vec<u8>,
}

/// Errors reported by transports.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    /// The packet is larger than [`MAX_PACKET_SIZE`].
    #[error("packet of {len} bytes exceeds the {MAX_PACKET_SIZE}-byte limit")]
    PacketTooLarge {
        /// Offending packet length.
        len: usize,
    },
    /// An operating-system socket error.
    #[error("socket error: {0}")]
    Io(#[from] std::io::Error),
}

/// A non-blocking, unreliable, unordered datagram transport.
///
/// Reliability, ordering and channels are built on top (see
/// [`ClientEndpoint`](crate::ClientEndpoint) and
/// [`ServerEndpoint`](crate::ServerEndpoint)). A transport may drop, duplicate
/// or reorder packets, exactly like UDP.
pub trait Transport {
    /// Address of a remote endpoint.
    type Addr: Copy + Eq + Hash + Debug;

    /// Sends one datagram. Delivery is not guaranteed; a packet the network
    /// cannot take right now is dropped silently.
    ///
    /// # Errors
    /// [`TransportError::PacketTooLarge`] above [`MAX_PACKET_SIZE`], or a
    /// socket error.
    fn send_to(&mut self, to: Self::Addr, packet: &[u8]) -> Result<(), TransportError>;

    /// Returns the next received datagram, or `None` when none is waiting.
    /// Never blocks. Call until `None` to drain the queue.
    ///
    /// # Errors
    /// A socket error.
    fn poll(&mut self) -> Result<Option<Datagram<Self::Addr>>, TransportError>;

    /// Advances transport-internal time by `dt`. Needed only by transports
    /// that delay packets (see
    /// [`ConditionedTransport`](crate::ConditionedTransport)); the default
    /// does nothing.
    fn update(&mut self, dt: Duration) {
        let _ = dt;
    }
}

pub(crate) fn check_size(packet: &[u8]) -> Result<(), TransportError> {
    if packet.len() > MAX_PACKET_SIZE {
        return Err(TransportError::PacketTooLarge { len: packet.len() });
    }
    Ok(())
}

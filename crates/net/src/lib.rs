//! Network layer: datagram transports and the four game channels.
//!
//! Two layers:
//!
//! * [`Transport`] moves raw datagrams of at most [`MAX_PACKET_SIZE`] bytes
//!   between addresses. It never blocks and has no async runtime. Three
//!   implementations: [`UdpTransport`] (real sockets), [`MemoryTransport`]
//!   (in-process, used by the integrated server) and [`ConditionedTransport`]
//!   (wraps any transport and simulates latency, jitter, loss, duplication and
//!   reordering).
//! * [`ClientEndpoint`] and [`ServerEndpoint`] run `renet` connections over a
//!   transport and expose the four [`Channel`]s. Every datagram they emit is
//!   at most [`MAX_PACKET_SIZE`] bytes: a thin framing layer fragments the rare
//!   `renet` packet that would not fit.
//!
//! Time is passed in explicitly with `update(dt)`; no wall clock is read, so
//! tests are deterministic and the server can drive everything from its tick.
#![forbid(unsafe_code)]

mod channel;
mod conditioned;
mod endpoint;
mod memory;
mod transport;
mod udp;
mod wire;

pub use channel::{Channel, MAX_MAP_PAYLOAD};
pub use conditioned::{ConditionedTransport, NetworkConditions};
pub use endpoint::{
    ClientEndpoint, EndpointConfig, PeerId, SendError, ServerEndpoint, ServerEvent,
};
pub use memory::{MemoryAddr, MemoryNetwork, MemoryTransport};
pub use transport::{Datagram, Transport, TransportError, MAX_PACKET_SIZE};
pub use udp::UdpTransport;

//! Type-erased server endpoints, so one server can listen on transports
//! with different address types at once.

use std::fmt;
use std::time::Duration;

use faset_net::{
    Channel, EndpointConfig, PeerId, SendError, ServerEndpoint, ServerEvent, Transport,
    TransportError,
};

/// The parts of [`ServerEndpoint`] the server uses, without its address type.
pub(crate) trait Listener {
    fn update(&mut self, dt: Duration) -> Result<(), TransportError>;
    fn poll_event(&mut self) -> Option<ServerEvent>;
    fn recv(&mut self, peer: PeerId, channel: Channel) -> Option<Vec<u8>>;
    fn send(&mut self, peer: PeerId, channel: Channel, payload: &[u8]) -> Result<(), SendError>;
    fn disconnect(&mut self, peer: PeerId);
    fn rtt(&self, peer: PeerId) -> f64;
}

impl<T: Transport> Listener for ServerEndpoint<T> {
    fn update(&mut self, dt: Duration) -> Result<(), TransportError> {
        ServerEndpoint::update(self, dt)
    }
    fn poll_event(&mut self) -> Option<ServerEvent> {
        ServerEndpoint::poll_event(self)
    }
    fn recv(&mut self, peer: PeerId, channel: Channel) -> Option<Vec<u8>> {
        ServerEndpoint::recv(self, peer, channel)
    }
    fn send(&mut self, peer: PeerId, channel: Channel, payload: &[u8]) -> Result<(), SendError> {
        ServerEndpoint::send(self, peer, channel, payload)
    }
    fn disconnect(&mut self, peer: PeerId) {
        ServerEndpoint::disconnect(self, peer);
    }
    fn rtt(&self, peer: PeerId) -> f64 {
        ServerEndpoint::rtt(self, peer)
    }
}

/// One transport the server listens on, of any address type.
///
/// Build it with `transport.into()` (default endpoint settings) or
/// [`with_config`](Self::with_config).
pub struct ServerTransport {
    pub(crate) listener: Box<dyn Listener + Send>,
}

impl ServerTransport {
    /// Wraps `transport` with default endpoint settings.
    pub fn new<T>(transport: T) -> Self
    where
        T: Transport + Send + 'static,
        T::Addr: Send,
    {
        Self::with_config(transport, EndpointConfig::default())
    }

    /// Wraps `transport` with explicit endpoint settings.
    pub fn with_config<T>(transport: T, config: EndpointConfig) -> Self
    where
        T: Transport + Send + 'static,
        T::Addr: Send,
    {
        Self {
            listener: Box::new(ServerEndpoint::with_config(transport, config)),
        }
    }
}

impl<T> From<T> for ServerTransport
where
    T: Transport + Send + 'static,
    T::Addr: Send,
{
    fn from(transport: T) -> Self {
        Self::new(transport)
    }
}

impl fmt::Debug for ServerTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ServerTransport")
    }
}

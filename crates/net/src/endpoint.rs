//! Client and server endpoints: `renet` connections over a [`Transport`].
//!
//! There is no handshake or authentication here (lot 1.4 adds the session
//! protocol on top of the event channel). A server learns about a client from
//! its first valid datagram; the client announces itself with a keep-alive
//! on its first update. A silent peer is dropped after
//! [`EndpointConfig::timeout`].

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::Duration;

use renet::{Bytes, RenetClient, RenetServer};

use crate::channel::{connection_config, Channel, MAX_MAP_PAYLOAD};
use crate::transport::{Datagram, Transport, TransportError};
use crate::wire::{Incoming, Link};

/// Bytes of map data per reliable message, so each message fills one packet
/// without `renet` having to slice it.
const MAP_CHUNK_SIZE: usize = 1000;
const CHUNK_MORE: u8 = 0;
const CHUNK_LAST: u8 = 1;

/// Timing and capacity settings of an endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndpointConfig {
    /// A keep-alive is sent when nothing else was sent for this long.
    pub keepalive: Duration,
    /// A peer silent for this long is considered gone.
    pub timeout: Duration,
    /// Most clients a server accepts at once.
    pub max_peers: usize,
}

impl Default for EndpointConfig {
    fn default() -> Self {
        Self {
            keepalive: Duration::from_secs(1),
            timeout: Duration::from_secs(10),
            max_peers: 64,
        }
    }
}

/// Server-side identifier of a connected client, stable for the connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PeerId(pub u64);

/// Connection change on a server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerEvent {
    /// A new client sent its first valid datagram.
    Connected(PeerId),
    /// A client timed out, errored or was disconnected.
    Disconnected(PeerId),
}

/// Why a message was not queued.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SendError {
    /// This side may not send on that channel.
    #[error("channel {0:?} cannot be sent on from this side")]
    WrongDirection(Channel),
    /// Empty payloads are not allowed.
    #[error("payload is empty")]
    Empty,
    /// The payload exceeds the channel's maximum.
    #[error("payload of {len} bytes exceeds the {max}-byte maximum")]
    TooLarge {
        /// Payload length.
        len: usize,
        /// Maximum for the channel.
        max: usize,
    },
    /// The channel's send buffer is full; retry after the next update.
    #[error("channel {0:?} send buffer is full")]
    ChannelFull(Channel),
    /// The connection is closed.
    #[error("not connected")]
    Disconnected,
    /// No such client.
    #[error("unknown peer {0:?}")]
    UnknownPeer(PeerId),
}

/// Splits a payload into the `renet` messages that carry it.
fn encode_messages(channel: Channel, payload: &[u8]) -> Result<Vec<Bytes>, SendError> {
    if payload.is_empty() {
        return Err(SendError::Empty);
    }
    if channel != Channel::MapTransfer {
        return Ok(vec![Bytes::copy_from_slice(payload)]);
    }
    if payload.len() > MAX_MAP_PAYLOAD {
        return Err(SendError::TooLarge {
            len: payload.len(),
            max: MAX_MAP_PAYLOAD,
        });
    }
    let chunks = payload.chunks(MAP_CHUNK_SIZE);
    let last = chunks.len() - 1;
    Ok(chunks
        .enumerate()
        .map(|(i, chunk)| {
            let mut message = Vec::with_capacity(chunk.len() + 1);
            message.push(if i == last { CHUNK_LAST } else { CHUNK_MORE });
            message.extend_from_slice(chunk);
            Bytes::from(message)
        })
        .collect())
}

fn send_frames<T: Transport>(transport: &mut T, to: T::Addr, frames: Vec<Vec<u8>>) {
    for frame in frames {
        if let Err(e) = transport.send_to(to, &frame) {
            tracing::warn!(error = %e, "failed to send a datagram");
        }
    }
}

// ---------------------------------------------------------------------------

/// The client side of a connection to one server.
#[derive(Debug)]
pub struct ClientEndpoint<T: Transport> {
    transport: T,
    server: T::Addr,
    config: EndpointConfig,
    renet: RenetClient,
    link: Link,
    now: Duration,
    announced: bool,
    map_rx: Vec<u8>,
}

impl<T: Transport> ClientEndpoint<T> {
    /// A client that talks to `server` through `transport`, default settings.
    pub fn new(transport: T, server: T::Addr) -> Self {
        Self::with_config(transport, server, EndpointConfig::default())
    }

    /// Like [`new`](Self::new) with explicit settings.
    pub fn with_config(transport: T, server: T::Addr, config: EndpointConfig) -> Self {
        let mut renet = RenetClient::new(connection_config());
        renet.set_connected();
        Self {
            transport,
            server,
            config,
            renet,
            link: Link::new(Duration::ZERO),
            now: Duration::ZERO,
            announced: false,
            map_rx: Vec::new(),
        }
    }

    /// Advances time by `dt`: reads the transport, processes packets and
    /// flushes queued messages. Call once per tick.
    ///
    /// # Errors
    /// A transport read error.
    pub fn update(&mut self, dt: Duration) -> Result<(), TransportError> {
        self.now += dt;
        self.transport.update(dt);
        while let Some(Datagram { from, data }) = self.transport.poll()? {
            if from != self.server {
                continue;
            }
            if let Incoming::Packet(packet) = self.link.decode(&data, self.now) {
                self.renet.process_packet(&packet);
            }
        }
        self.link.expire(self.now);
        self.renet.update(dt);

        if self.now.saturating_sub(self.link.last_recv) >= self.config.timeout {
            self.renet.disconnect();
        }
        for packet in self.renet.get_packets_to_send() {
            let frames = self.link.encode(&packet, self.now);
            send_frames(&mut self.transport, self.server, frames);
        }
        let idle = self.now.saturating_sub(self.link.last_send);
        if !self.announced || idle >= self.config.keepalive {
            self.announced = true;
            let frame = self.link.keepalive(self.now);
            send_frames(&mut self.transport, self.server, vec![frame]);
        }
        Ok(())
    }

    /// Queues a message; it is sent by the next [`update`](Self::update).
    ///
    /// # Errors
    /// See [`SendError`].
    pub fn send(&mut self, channel: Channel, payload: &[u8]) -> Result<(), SendError> {
        if !channel.client_may_send() {
            return Err(SendError::WrongDirection(channel));
        }
        if !self.renet.is_connected() {
            return Err(SendError::Disconnected);
        }
        let messages = encode_messages(channel, payload)?;
        if !self.renet.can_send_message(channel.id(), payload.len()) {
            return Err(SendError::ChannelFull(channel));
        }
        for message in messages {
            self.renet.send_message(channel.id(), message);
        }
        Ok(())
    }

    /// Next received message on `channel`, if any.
    pub fn recv(&mut self, channel: Channel) -> Option<Vec<u8>> {
        if !channel.server_may_send() {
            return None;
        }
        if channel != Channel::MapTransfer {
            return self.renet.receive_message(channel.id()).map(|b| b.to_vec());
        }
        while let Some(chunk) = self.renet.receive_message(channel.id()) {
            let Some((&flag, data)) = chunk.split_first() else {
                continue;
            };
            self.map_rx.extend_from_slice(data);
            if self.map_rx.len() > MAX_MAP_PAYLOAD {
                tracing::error!("map transfer exceeds the maximum; disconnecting");
                self.map_rx.clear();
                self.renet.disconnect();
                return None;
            }
            if flag == CHUNK_LAST {
                return Some(std::mem::take(&mut self.map_rx));
            }
        }
        None
    }

    /// Whether the connection is usable (not timed out or closed).
    #[must_use]
    pub fn is_connected(&self) -> bool {
        self.renet.is_connected()
    }

    /// Closes the connection locally. The server notices by timeout.
    pub fn disconnect(&mut self) {
        self.renet.disconnect();
    }

    /// Round-trip time estimate in seconds, from `renet`.
    #[must_use]
    pub fn rtt(&self) -> f64 {
        self.renet.rtt()
    }

    /// The underlying transport.
    #[must_use]
    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// Mutable access to the underlying transport.
    pub fn transport_mut(&mut self) -> &mut T {
        &mut self.transport
    }
}

// ---------------------------------------------------------------------------

#[derive(Debug)]
struct Peer<A> {
    addr: A,
    link: Link,
}

/// The server side: any number of clients over one transport.
#[derive(Debug)]
pub struct ServerEndpoint<T: Transport> {
    transport: T,
    config: EndpointConfig,
    renet: RenetServer,
    by_addr: HashMap<T::Addr, PeerId>,
    peers: BTreeMap<PeerId, Peer<T::Addr>>,
    events: VecDeque<ServerEvent>,
    next_id: u64,
    now: Duration,
}

impl<T: Transport> ServerEndpoint<T> {
    /// A server reading and writing through `transport`, default settings.
    pub fn new(transport: T) -> Self {
        Self::with_config(transport, EndpointConfig::default())
    }

    /// Like [`new`](Self::new) with explicit settings.
    pub fn with_config(transport: T, config: EndpointConfig) -> Self {
        Self {
            transport,
            config,
            renet: RenetServer::new(connection_config()),
            by_addr: HashMap::new(),
            peers: BTreeMap::new(),
            events: VecDeque::new(),
            next_id: 1,
            now: Duration::ZERO,
        }
    }

    /// Advances time by `dt`: reads every waiting datagram, processes
    /// packets, drops dead peers and flushes queued messages. Call once at
    /// the start of each tick.
    ///
    /// # Errors
    /// A transport read error.
    pub fn update(&mut self, dt: Duration) -> Result<(), TransportError> {
        self.now += dt;
        self.transport.update(dt);
        while let Some(Datagram { from, data }) = self.transport.poll()? {
            self.receive(from, &data);
        }
        self.renet.update(dt);
        // renet keeps its own event queue; ours is the public one.
        while self.renet.get_event().is_some() {}

        let now = self.now;
        let timeout = self.config.timeout;
        let mut gone: Vec<PeerId> = self
            .renet
            .disconnections_id()
            .into_iter()
            .map(PeerId)
            .collect();
        for (id, peer) in &mut self.peers {
            peer.link.expire(now);
            if now.saturating_sub(peer.link.last_recv) >= timeout {
                gone.push(*id);
            }
        }
        for id in gone {
            self.remove(id);
        }

        let keepalive = self.config.keepalive;
        for (id, peer) in &mut self.peers {
            let Ok(packets) = self.renet.get_packets_to_send(id.0) else {
                continue;
            };
            for packet in packets {
                let frames = peer.link.encode(&packet, now);
                send_frames(&mut self.transport, peer.addr, frames);
            }
            if now.saturating_sub(peer.link.last_send) >= keepalive {
                let frame = peer.link.keepalive(now);
                send_frames(&mut self.transport, peer.addr, vec![frame]);
            }
        }
        Ok(())
    }

    fn receive(&mut self, from: T::Addr, data: &[u8]) {
        let now = self.now;
        let id = if let Some(&id) = self.by_addr.get(&from) {
            id
        } else {
            if self.peers.len() >= self.config.max_peers {
                return;
            }
            let mut link = Link::new(now);
            let incoming = link.decode(data, now);
            if incoming == Incoming::Invalid {
                return;
            }
            let id = PeerId(self.next_id);
            self.next_id += 1;
            self.renet.add_connection(id.0);
            self.by_addr.insert(from, id);
            self.peers.insert(id, Peer { addr: from, link });
            self.events.push_back(ServerEvent::Connected(id));
            if let Incoming::Packet(packet) = incoming {
                let _ = self.renet.process_packet_from(&packet, id.0);
            }
            return;
        };
        let Some(peer) = self.peers.get_mut(&id) else {
            return;
        };
        if let Incoming::Packet(packet) = peer.link.decode(data, now) {
            let _ = self.renet.process_packet_from(&packet, id.0);
        }
    }

    fn remove(&mut self, id: PeerId) {
        if let Some(peer) = self.peers.remove(&id) {
            self.by_addr.remove(&peer.addr);
            self.renet.remove_connection(id.0);
            self.events.push_back(ServerEvent::Disconnected(id));
        }
    }

    /// Next connection event, if any.
    pub fn poll_event(&mut self) -> Option<ServerEvent> {
        self.events.pop_front()
    }

    /// Connected clients in id order.
    pub fn peers(&self) -> impl Iterator<Item = PeerId> + '_ {
        self.peers.keys().copied()
    }

    /// Transport address of a client.
    #[must_use]
    pub fn addr(&self, peer: PeerId) -> Option<T::Addr> {
        self.peers.get(&peer).map(|p| p.addr)
    }

    /// Forgets a client at once. It is not told; it times out on its side.
    pub fn disconnect(&mut self, peer: PeerId) {
        self.remove(peer);
    }

    /// Queues a message for one client; sent by the next
    /// [`update`](Self::update).
    ///
    /// # Errors
    /// See [`SendError`].
    pub fn send(
        &mut self,
        peer: PeerId,
        channel: Channel,
        payload: &[u8],
    ) -> Result<(), SendError> {
        if !channel.server_may_send() {
            return Err(SendError::WrongDirection(channel));
        }
        if !self.peers.contains_key(&peer) {
            return Err(SendError::UnknownPeer(peer));
        }
        if !self.renet.is_connected(peer.0) {
            return Err(SendError::Disconnected);
        }
        let messages = encode_messages(channel, payload)?;
        if !self
            .renet
            .can_send_message(peer.0, channel.id(), payload.len())
        {
            return Err(SendError::ChannelFull(channel));
        }
        for message in messages {
            self.renet.send_message(peer.0, channel.id(), message);
        }
        Ok(())
    }

    /// Queues a message for every client. Clients whose buffer is full are
    /// skipped (and logged).
    ///
    /// # Errors
    /// [`SendError::WrongDirection`], [`SendError::Empty`] or
    /// [`SendError::TooLarge`], which apply to all clients alike.
    pub fn broadcast(&mut self, channel: Channel, payload: &[u8]) -> Result<(), SendError> {
        let ids: Vec<PeerId> = self.peers.keys().copied().collect();
        for id in ids {
            match self.send(id, channel, payload) {
                Err(
                    e @ (SendError::WrongDirection(_)
                    | SendError::Empty
                    | SendError::TooLarge { .. }),
                ) => return Err(e),
                Err(e) => tracing::warn!(peer = id.0, error = %e, "broadcast skipped a client"),
                Ok(()) => {}
            }
        }
        Ok(())
    }

    /// Next received message from `peer` on `channel`, if any.
    pub fn recv(&mut self, peer: PeerId, channel: Channel) -> Option<Vec<u8>> {
        if !channel.client_may_send() || !self.peers.contains_key(&peer) {
            return None;
        }
        self.renet
            .receive_message(peer.0, channel.id())
            .map(|b| b.to_vec())
    }

    /// Round-trip time estimate for a client in seconds, from `renet`.
    #[must_use]
    pub fn rtt(&self, peer: PeerId) -> f64 {
        self.renet.rtt(peer.0)
    }

    /// The underlying transport.
    #[must_use]
    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// Mutable access to the underlying transport.
    pub fn transport_mut(&mut self) -> &mut T {
        &mut self.transport
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_payload_chunks_reassemble_exactly() {
        let payload: Vec<u8> = (0..2500u32).map(|i| (i % 256) as u8).collect();
        let messages = encode_messages(Channel::MapTransfer, &payload).unwrap();
        assert_eq!(messages.len(), 3);
        assert!(messages.iter().all(|m| m.len() <= MAP_CHUNK_SIZE + 1));
        let flags: Vec<u8> = messages.iter().map(|m| m[0]).collect();
        assert_eq!(flags, [CHUNK_MORE, CHUNK_MORE, CHUNK_LAST]);
        let joined: Vec<u8> = messages.iter().flat_map(|m| m[1..].to_vec()).collect();
        assert_eq!(joined, payload);
    }

    /// Documents why the framing layer exists: `renet` slices at 1200 bytes of
    /// payload and adds its own header, so its packets overflow the datagram
    /// limit.
    #[test]
    fn renet_packets_can_exceed_the_datagram_limit() {
        let mut client = RenetClient::new(connection_config());
        client.set_connected();
        client.send_message(Channel::Event.id(), vec![7u8; 3000]);
        let largest = client
            .get_packets_to_send()
            .iter()
            .map(Vec::len)
            .max()
            .unwrap();
        assert!(
            largest >= crate::MAX_PACKET_SIZE,
            "largest packet {largest}"
        );
    }

    #[test]
    fn payload_limits_are_enforced() {
        assert_eq!(encode_messages(Channel::Event, &[]), Err(SendError::Empty));
        let big = vec![0; MAX_MAP_PAYLOAD + 1];
        assert!(matches!(
            encode_messages(Channel::MapTransfer, &big),
            Err(SendError::TooLarge { .. })
        ));
    }
}

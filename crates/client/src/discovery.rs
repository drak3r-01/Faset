//! LAN discovery over UDP broadcast.
//!
//! A [`ServerBrowser`] sends a small request to the discovery port (default
//! [`DISCOVERY_PORT`]) of every network interface; a hosting game answers
//! through a [`DiscoveryResponder`] with its [`ServerInfo`]. The browser
//! refreshes every [`REFRESH_INTERVAL`] and drops an entry after
//! [`ENTRY_TTL`] without a reply.
//!
//! Like the net crate, the browser reads no clock: time is passed in with
//! [`ServerBrowser::update`], so expiry is testable without waiting.
//!
//! Wire format (all integers big-endian): `"FSET"`, a kind byte (`0`
//! request, `1` reply), then for a request the `u16` protocol version, for a
//! reply: mode `u8`, players `u16`, max players `u16`, game port `u16`, and
//! the name and the map name as `u8` length plus UTF-8 bytes.

use std::io::{self, ErrorKind};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::time::Duration;

use faset_protocol::PROTOCOL_VERSION;

/// Default UDP port servers listen on for discovery requests.
pub const DISCOVERY_PORT: u16 = 7778;
/// Default game port.
pub const GAME_PORT: u16 = 7777;
/// Time between two request rounds.
pub const REFRESH_INTERVAL: Duration = Duration::from_secs(2);
/// An entry expires after this long without a reply.
pub const ENTRY_TTL: Duration = Duration::from_secs(6);

const MAGIC: &[u8; 4] = b"FSET";
const KIND_REQUEST: u8 = 0;
const KIND_REPLY: u8 = 1;
/// Longest name or map name sent, in bytes.
const MAX_TEXT: usize = 64;

/// What a server announces about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerInfo {
    /// Server name.
    pub name: String,
    /// Game mode id (same value as the handshake).
    pub mode: u8,
    /// Map name.
    pub map: String,
    /// Players present, bots included.
    pub players: u16,
    /// Maximum players.
    pub max_players: u16,
    /// UDP port to join on (the sender's IP is the host).
    pub game_port: u16,
}

fn truncated(s: &str) -> &[u8] {
    let mut end = s.len().min(MAX_TEXT);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s.as_bytes()[..end]
}

impl ServerInfo {
    /// Encodes the reply datagram. Names longer than 64 bytes are cut.
    #[must_use]
    pub fn encode_reply(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(24 + 2 * MAX_TEXT);
        out.extend_from_slice(MAGIC);
        out.push(KIND_REPLY);
        out.push(self.mode);
        out.extend_from_slice(&self.players.to_be_bytes());
        out.extend_from_slice(&self.max_players.to_be_bytes());
        out.extend_from_slice(&self.game_port.to_be_bytes());
        for text in [&self.name, &self.map] {
            let bytes = truncated(text);
            // `truncated` caps the length at MAX_TEXT, which fits a u8.
            out.push(u8::try_from(bytes.len()).unwrap_or(u8::MAX));
            out.extend_from_slice(bytes);
        }
        out
    }

    /// Decodes a reply datagram, or `None` if it is not a valid reply.
    #[must_use]
    pub fn decode_reply(data: &[u8]) -> Option<Self> {
        let rest = data.strip_prefix(MAGIC)?.strip_prefix(&[KIND_REPLY])?;
        let (&mode, rest) = rest.split_first()?;
        let (players, rest) = take_u16(rest)?;
        let (max_players, rest) = take_u16(rest)?;
        let (game_port, rest) = take_u16(rest)?;
        let (name, rest) = take_text(rest)?;
        let (map, rest) = take_text(rest)?;
        rest.is_empty().then_some(Self {
            name,
            mode,
            map,
            players,
            max_players,
            game_port,
        })
    }
}

fn take_u16(data: &[u8]) -> Option<(u16, &[u8])> {
    let (head, rest) = data.split_at_checked(2)?;
    Some((u16::from_be_bytes([head[0], head[1]]), rest))
}

fn take_text(data: &[u8]) -> Option<(String, &[u8])> {
    let (&len, rest) = data.split_first()?;
    let (text, rest) = rest.split_at_checked(usize::from(len))?;
    Some((String::from_utf8(text.to_vec()).ok()?, rest))
}

fn encode_request() -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    out.push(KIND_REQUEST);
    out.extend_from_slice(&PROTOCOL_VERSION.to_be_bytes());
    out
}

fn is_request(data: &[u8]) -> bool {
    data.strip_prefix(MAGIC)
        .and_then(|r| r.strip_prefix(&[KIND_REQUEST]))
        .and_then(take_u16)
        .is_some_and(|(_, rest)| rest.is_empty())
}

fn is_transient(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        ErrorKind::ConnectionReset | ErrorKind::ConnectionRefused | ErrorKind::Interrupted
    )
}

/// Answers discovery requests for one hosted game. Non-blocking: call
/// [`poll`](Self::poll) regularly (the host does it every server tick).
#[derive(Debug)]
pub struct DiscoveryResponder {
    socket: UdpSocket,
}

impl DiscoveryResponder {
    /// Binds the discovery port on every interface.
    ///
    /// # Errors
    /// Fails if the port cannot be bound (for example another game on this
    /// machine already answers on it).
    pub fn bind(port: u16) -> io::Result<Self> {
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, port))?;
        socket.set_nonblocking(true)?;
        Ok(Self { socket })
    }

    /// The bound local address.
    ///
    /// # Errors
    /// Fails if the socket address cannot be read.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    /// Answers every request waiting on the socket with `info`.
    pub fn poll(&self, info: &ServerInfo) {
        let reply = info.encode_reply();
        let mut buf = [0_u8; 64];
        loop {
            match self.socket.recv_from(&mut buf) {
                Ok((len, from)) => {
                    if is_request(&buf[..len]) {
                        if let Err(e) = self.socket.send_to(&reply, from) {
                            tracing::debug!(%from, error = %e, "discovery reply failed");
                        }
                    }
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => return,
                Err(e) if is_transient(&e) => {}
                Err(e) => {
                    tracing::warn!(error = %e, "discovery socket read failed");
                    return;
                }
            }
        }
    }
}

/// A server seen on the network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerEntry {
    /// Where to join: the reply's source IP and the announced game port.
    pub addr: SocketAddr,
    /// What the server announced in its latest reply.
    pub info: ServerInfo,
    /// Browser time of the latest reply.
    last_seen: Duration,
}

/// The list of servers found by broadcast.
#[derive(Debug)]
pub struct ServerBrowser {
    socket: UdpSocket,
    targets: Vec<SocketAddr>,
    entries: Vec<ServerEntry>,
    now: Duration,
    next_request: Duration,
}

impl ServerBrowser {
    /// A browser that broadcasts to `port` on every IPv4 interface (the
    /// directed broadcast address of each, plus the limited broadcast).
    ///
    /// # Errors
    /// Fails if the socket cannot be created.
    pub fn new(port: u16) -> io::Result<Self> {
        Self::with_targets(broadcast_targets(port))
    }

    /// A browser that sends requests to explicit `targets` instead of
    /// enumerating interfaces. Tests inject loopback here when broadcast is
    /// not routable (containers).
    ///
    /// # Errors
    /// Fails if the socket cannot be created.
    pub fn with_targets(targets: Vec<SocketAddr>) -> io::Result<Self> {
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
        socket.set_nonblocking(true)?;
        socket.set_broadcast(true)?;
        Ok(Self {
            socket,
            targets,
            entries: Vec::new(),
            now: Duration::ZERO,
            next_request: Duration::ZERO,
        })
    }

    /// The servers currently known, in order of first discovery.
    #[must_use]
    pub fn entries(&self) -> &[ServerEntry] {
        &self.entries
    }

    /// Advances the browser clock by `dt`, reads replies, sends a new request
    /// round when [`REFRESH_INTERVAL`] has passed (the first call always
    /// does) and drops entries silent for [`ENTRY_TTL`].
    pub fn update(&mut self, dt: Duration) {
        self.now += dt;
        self.receive();
        if self.now >= self.next_request {
            self.next_request = self.now + REFRESH_INTERVAL;
            let request = encode_request();
            for target in &self.targets {
                if let Err(e) = self.socket.send_to(&request, target) {
                    tracing::debug!(%target, error = %e, "discovery request failed");
                }
            }
        }
        let now = self.now;
        self.entries.retain(|e| now - e.last_seen < ENTRY_TTL);
    }

    fn receive(&mut self) {
        let mut buf = [0_u8; 512];
        loop {
            match self.socket.recv_from(&mut buf) {
                Ok((len, from)) => {
                    let Some(info) = ServerInfo::decode_reply(&buf[..len]) else {
                        continue;
                    };
                    let addr = SocketAddr::new(from.ip(), info.game_port);
                    let last_seen = self.now;
                    if let Some(e) = self.entries.iter_mut().find(|e| e.addr == addr) {
                        e.info = info;
                        e.last_seen = last_seen;
                    } else {
                        self.entries.push(ServerEntry {
                            addr,
                            info,
                            last_seen,
                        });
                    }
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => return,
                Err(e) if is_transient(&e) => {}
                Err(e) => {
                    tracing::warn!(error = %e, "discovery read failed");
                    return;
                }
            }
        }
    }
}

/// Directed broadcast address of every IPv4 interface, plus the limited
/// broadcast (which the OS routes through the default interface only).
fn broadcast_targets(port: u16) -> Vec<SocketAddr> {
    let mut ips = vec![Ipv4Addr::BROADCAST];
    match if_addrs::get_if_addrs() {
        Ok(ifaces) => {
            for iface in ifaces {
                if let if_addrs::IfAddr::V4(v4) = iface.addr {
                    if let Some(b) = v4.broadcast {
                        if !ips.contains(&b) {
                            ips.push(b);
                        }
                    }
                }
            }
        }
        Err(e) => tracing::warn!(error = %e, "cannot list network interfaces"),
    }
    ips.into_iter()
        .map(|ip| SocketAddr::new(IpAddr::V4(ip), port))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info() -> ServerInfo {
        ServerInfo {
            name: "Faset \u{e9}".into(),
            mode: 3,
            map: "arena".into(),
            players: 2,
            max_players: 16,
            game_port: 7777,
        }
    }

    #[test]
    fn reply_round_trips() {
        let i = info();
        assert_eq!(ServerInfo::decode_reply(&i.encode_reply()), Some(i));
    }

    #[test]
    fn long_names_are_cut_on_a_char_boundary() {
        let mut i = info();
        i.name = "\u{e9}".repeat(100);
        let back = ServerInfo::decode_reply(&i.encode_reply()).unwrap();
        assert_eq!(back.name, "\u{e9}".repeat(32));
    }

    #[test]
    fn garbage_is_rejected() {
        assert!(ServerInfo::decode_reply(b"").is_none());
        assert!(ServerInfo::decode_reply(b"FSET\x01").is_none());
        let mut bad = info().encode_reply();
        bad.push(0);
        assert!(ServerInfo::decode_reply(&bad).is_none());
        assert!(!is_request(b"FSET\x01\x00\x01"));
        assert!(is_request(&encode_request()));
    }
}

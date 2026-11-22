//! Where a player joins (lot 6.2): a typed address, a favourite or an entry
//! of the LAN list.

use std::net::{SocketAddr, ToSocketAddrs};

use crate::discovery::{ServerEntry, GAME_PORT};

/// A server to connect to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JoinTarget {
    /// `host:port` (or a bare host, which uses the default game port).
    Address(String),
    /// A server found on the LAN.
    Lan(ServerEntry),
}

impl JoinTarget {
    /// The socket address to connect to, `None` for an address that does
    /// not resolve.
    #[must_use]
    pub fn resolve(&self) -> Option<SocketAddr> {
        match self {
            Self::Lan(entry) => Some(entry.addr),
            Self::Address(address) => {
                let address = address.trim();
                let first = |r: std::io::Result<std::vec::IntoIter<SocketAddr>>| {
                    r.ok().and_then(|mut it| it.next())
                };
                first(address.to_socket_addrs())
                    .or_else(|| first((address, GAME_PORT).to_socket_addrs()))
            }
        }
    }

    /// The `host:port` text kept in favourites and history.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Address(a) => a.trim().to_owned(),
            Self::Lan(entry) => entry.addr.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_resolve_with_the_default_port() {
        let port = |s: &str| JoinTarget::Address(s.into()).resolve().map(|a| a.port());
        assert_eq!(port("127.0.0.1:9000"), Some(9000));
        assert_eq!(port("127.0.0.1"), Some(GAME_PORT));
        assert_eq!(port(" 127.0.0.1 "), Some(GAME_PORT));
        assert_eq!(port("not an address:::"), None);
    }
}

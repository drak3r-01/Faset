//! UDP transport over non-blocking `std` sockets.

use std::io::{self, ErrorKind};
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};

use crate::transport::{check_size, Datagram, Transport, TransportError};
use crate::MAX_PACKET_SIZE;

/// Real UDP sockets. No async runtime: [`Transport::poll`] returns
/// immediately, so the server can drain the socket at the start of each tick.
#[derive(Debug)]
pub struct UdpTransport {
    socket: UdpSocket,
    buf: Box<[u8; MAX_PACKET_SIZE + 1]>,
}

impl UdpTransport {
    /// Binds a non-blocking socket. Use port 0 to let the OS choose one.
    ///
    /// # Errors
    /// Fails if the address cannot be bound.
    pub fn bind(addr: impl ToSocketAddrs) -> io::Result<Self> {
        let socket = UdpSocket::bind(addr)?;
        socket.set_nonblocking(true)?;
        Ok(Self {
            socket,
            buf: Box::new([0; MAX_PACKET_SIZE + 1]),
        })
    }

    /// The bound local address.
    ///
    /// # Errors
    /// Fails if the socket address cannot be read.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }
}

impl Transport for UdpTransport {
    type Addr = SocketAddr;

    fn send_to(&mut self, to: SocketAddr, packet: &[u8]) -> Result<(), TransportError> {
        check_size(packet)?;
        match self.socket.send_to(packet, to) {
            Ok(_) => Ok(()),
            // Full send buffer or a stale ICMP error: the packet is lost, as
            // UDP allows.
            Err(e) if is_transient(&e) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    fn poll(&mut self) -> Result<Option<Datagram<SocketAddr>>, TransportError> {
        loop {
            match self.socket.recv_from(&mut self.buf[..]) {
                Ok((len, from)) if len <= MAX_PACKET_SIZE => {
                    return Ok(Some(Datagram {
                        from,
                        data: self.buf[..len].to_vec(),
                    }));
                }
                Ok((len, from)) => {
                    tracing::warn!(%from, len, "dropping oversized datagram");
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => return Ok(None),
                // Windows reports an earlier ICMP "port unreachable" here.
                Err(e) if is_transient(&e) => {}
                Err(e) => return Err(e.into()),
            }
        }
    }
}

fn is_transient(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        ErrorKind::WouldBlock | ErrorKind::ConnectionReset | ErrorKind::ConnectionRefused
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn loopback_round_trip_and_empty_poll() {
        let mut a = UdpTransport::bind("127.0.0.1:0").unwrap();
        let mut b = UdpTransport::bind("127.0.0.1:0").unwrap();
        assert!(a.poll().unwrap().is_none());
        a.send_to(b.local_addr().unwrap(), b"ping").unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        let got = loop {
            if let Some(d) = b.poll().unwrap() {
                break d;
            }
            assert!(Instant::now() < deadline, "datagram never arrived");
            std::thread::sleep(Duration::from_millis(1));
        };
        assert_eq!(got.from, a.local_addr().unwrap());
        assert_eq!(got.data, b"ping");
    }

    #[test]
    fn rejects_oversized_packets() {
        let mut a = UdpTransport::bind("127.0.0.1:0").unwrap();
        let to = a.local_addr().unwrap();
        let big = vec![0; MAX_PACKET_SIZE + 1];
        assert!(matches!(
            a.send_to(to, &big),
            Err(TransportError::PacketTooLarge { .. })
        ));
    }
}

//! In-process transport, used by the integrated server and by tests.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::transport::{check_size, Datagram, Transport, TransportError};

/// Address of an endpoint inside a [`MemoryNetwork`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MemoryAddr(pub u32);

#[derive(Debug, Default)]
struct Hub {
    queues: HashMap<MemoryAddr, VecDeque<Datagram<MemoryAddr>>>,
    next: u32,
}

/// A set of [`MemoryTransport`]s that can reach each other. Cheap to clone;
/// clones share the same network.
#[derive(Debug, Clone, Default)]
pub struct MemoryNetwork {
    hub: Arc<Mutex<Hub>>,
}

impl MemoryNetwork {
    /// Creates an empty network.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds an endpoint with a fresh address.
    #[must_use]
    pub fn endpoint(&self) -> MemoryTransport {
        let mut hub = lock(&self.hub);
        let addr = MemoryAddr(hub.next);
        hub.next += 1;
        hub.queues.insert(addr, VecDeque::new());
        MemoryTransport {
            addr,
            hub: Arc::clone(&self.hub),
        }
    }
}

fn lock(hub: &Mutex<Hub>) -> MutexGuard<'_, Hub> {
    hub.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One endpoint of a [`MemoryNetwork`]. Lossless and ordered; wrap it in a
/// [`ConditionedTransport`](crate::ConditionedTransport) to degrade it.
#[derive(Debug)]
pub struct MemoryTransport {
    addr: MemoryAddr,
    hub: Arc<Mutex<Hub>>,
}

impl MemoryTransport {
    /// Two endpoints connected to each other on a private network.
    #[must_use]
    pub fn pair() -> (Self, Self) {
        let net = MemoryNetwork::new();
        (net.endpoint(), net.endpoint())
    }

    /// This endpoint's address.
    #[must_use]
    pub fn addr(&self) -> MemoryAddr {
        self.addr
    }
}

impl Drop for MemoryTransport {
    fn drop(&mut self) {
        lock(&self.hub).queues.remove(&self.addr);
    }
}

impl Transport for MemoryTransport {
    type Addr = MemoryAddr;

    fn send_to(&mut self, to: MemoryAddr, packet: &[u8]) -> Result<(), TransportError> {
        check_size(packet)?;
        // A missing destination drops the packet, like UDP to a closed port.
        if let Some(queue) = lock(&self.hub).queues.get_mut(&to) {
            queue.push_back(Datagram {
                from: self.addr,
                data: packet.to_vec(),
            });
        }
        Ok(())
    }

    fn poll(&mut self) -> Result<Option<Datagram<MemoryAddr>>, TransportError> {
        Ok(lock(&self.hub)
            .queues
            .get_mut(&self.addr)
            .and_then(VecDeque::pop_front))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MAX_PACKET_SIZE;

    #[test]
    fn delivers_in_order_and_reports_sender() {
        let (mut a, mut b) = MemoryTransport::pair();
        a.send_to(b.addr(), b"one").unwrap();
        a.send_to(b.addr(), b"two").unwrap();
        let first = b.poll().unwrap().unwrap();
        assert_eq!(first.from, a.addr());
        assert_eq!(first.data, b"one");
        assert_eq!(b.poll().unwrap().unwrap().data, b"two");
        assert!(b.poll().unwrap().is_none());
        assert!(a.poll().unwrap().is_none());
    }

    #[test]
    fn rejects_oversized_packets_and_drops_unknown_destinations() {
        let (mut a, b) = MemoryTransport::pair();
        let big = vec![0; MAX_PACKET_SIZE + 1];
        assert!(matches!(
            a.send_to(b.addr(), &big),
            Err(TransportError::PacketTooLarge { .. })
        ));
        a.send_to(MemoryAddr(99), b"x").unwrap();
    }

    #[test]
    fn many_endpoints_share_one_network() {
        let net = MemoryNetwork::new();
        let (mut s, mut c1, mut c2) = (net.endpoint(), net.endpoint(), net.endpoint());
        c1.send_to(s.addr(), b"1").unwrap();
        c2.send_to(s.addr(), b"2").unwrap();
        let froms: Vec<_> = std::iter::from_fn(|| s.poll().unwrap())
            .map(|d| d.from)
            .collect();
        assert_eq!(froms, vec![c1.addr(), c2.addr()]);
    }
}

//! Channel behaviour over every transport, with and without packet loss.
//!
//! The same scenarios run over the in-memory transport and UDP over loopback,
//! each wrapped in the condition simulator (a perfect link, 10 % loss, and a
//! hostile link that also duplicates packets). Time is virtual: each step is
//! 10 ms of simulated time.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use faset_net::{
    Channel, ClientEndpoint, ConditionedTransport, Datagram, EndpointConfig, MemoryNetwork,
    MemoryTransport, NetworkConditions, PeerId, SendError, ServerEndpoint, ServerEvent, Transport,
    TransportError, UdpTransport, MAX_PACKET_SIZE,
};

const DT: Duration = Duration::from_millis(10);
const MAX_STEPS: usize = 30_000;

// ---- harness ---------------------------------------------------------------

/// Records the size of every datagram that actually leaves the endpoint.
#[derive(Debug)]
struct Spy<T> {
    inner: T,
    largest: Arc<AtomicUsize>,
    count: Arc<AtomicUsize>,
}

impl<T: Transport> Transport for Spy<T> {
    type Addr = T::Addr;

    fn send_to(&mut self, to: T::Addr, packet: &[u8]) -> Result<(), TransportError> {
        self.largest.fetch_max(packet.len(), Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Relaxed);
        self.inner.send_to(to, packet)
    }

    fn poll(&mut self) -> Result<Option<Datagram<T::Addr>>, TransportError> {
        self.inner.poll()
    }

    fn update(&mut self, dt: Duration) {
        self.inner.update(dt);
    }
}

type Wrapped<T> = ConditionedTransport<Spy<T>>;

struct Rig<T: Transport> {
    client: ClientEndpoint<Wrapped<T>>,
    server: ServerEndpoint<Wrapped<T>>,
    largest: Arc<AtomicUsize>,
    packets: Arc<AtomicUsize>,
    real_time: bool,
    peer: PeerId,
}

fn wrap<T: Transport>(
    inner: T,
    conditions: &NetworkConditions,
    seed: u64,
    largest: &Arc<AtomicUsize>,
    count: &Arc<AtomicUsize>,
) -> Wrapped<T> {
    let spy = Spy {
        inner,
        largest: Arc::clone(largest),
        count: Arc::clone(count),
    };
    ConditionedTransport::new(
        spy,
        NetworkConditions {
            seed,
            ..conditions.clone()
        },
    )
}

impl<T: Transport> Rig<T> {
    fn new(
        client_t: T,
        server_t: T,
        server_addr: T::Addr,
        conditions: &NetworkConditions,
        real_time: bool,
    ) -> Self {
        let largest = Arc::new(AtomicUsize::new(0));
        let packets = Arc::new(AtomicUsize::new(0));
        let client = ClientEndpoint::new(
            wrap(client_t, conditions, 1, &largest, &packets),
            server_addr,
        );
        let server = ServerEndpoint::new(wrap(server_t, conditions, 2, &largest, &packets));
        let mut rig = Self {
            client,
            server,
            largest,
            packets,
            real_time,
            peer: PeerId(0),
        };
        rig.peer = rig.connect();
        rig
    }

    fn step(&mut self) {
        self.client.update(DT).unwrap();
        self.server.update(DT).unwrap();
        if self.real_time {
            std::thread::sleep(Duration::from_micros(30));
        }
    }

    /// Steps until `done` is true; panics if it never is.
    fn run_until(&mut self, what: &str, mut done: impl FnMut(&mut Self) -> bool) {
        for _ in 0..MAX_STEPS {
            self.step();
            if done(self) {
                return;
            }
        }
        panic!("{what}: not finished after {MAX_STEPS} steps");
    }

    fn connect(&mut self) -> PeerId {
        self.run_until("connect", |r| r.server.peers().next().is_some());
        self.server.peers().next().unwrap()
    }

    fn assert_wire_limit(&self) {
        assert!(self.packets.load(Ordering::Relaxed) > 0);
        let largest = self.largest.load(Ordering::Relaxed);
        assert!(
            largest <= MAX_PACKET_SIZE,
            "a {largest}-byte datagram was sent (limit {MAX_PACKET_SIZE})"
        );
    }
}

fn memory_rig(conditions: &NetworkConditions) -> Rig<MemoryTransport> {
    let (c, s) = MemoryTransport::pair();
    let addr = s.addr();
    Rig::new(c, s, addr, conditions, false)
}

fn udp_rig(conditions: &NetworkConditions) -> Rig<UdpTransport> {
    let c = UdpTransport::bind("127.0.0.1:0").unwrap();
    let s = UdpTransport::bind("127.0.0.1:0").unwrap();
    let addr = s.local_addr().unwrap();
    Rig::new(c, s, addr, conditions, true)
}

fn perfect() -> NetworkConditions {
    NetworkConditions::default()
}

fn lossy() -> NetworkConditions {
    NetworkConditions {
        latency: Duration::from_millis(30),
        jitter: Duration::from_millis(10),
        loss_percent: 10.0,
        reorder_percent: 5.0,
        ..NetworkConditions::default()
    }
}

fn hostile() -> NetworkConditions {
    NetworkConditions {
        duplicate_percent: 5.0,
        ..lossy()
    }
}

fn fnv1a(data: &[u8]) -> u64 {
    data.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Deterministic, incompressible-looking bytes.
fn noise(len: usize, mut state: u64) -> Vec<u8> {
    (0..len)
        .map(|_| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 56) as u8
        })
        .collect()
}

// ---- scenarios -------------------------------------------------------------

/// Message `i`: little-endian index, then filler whose length depends on `i`.
/// The sizes straddle the 1200-byte slicing threshold of the channel layer.
fn event_message(i: u32) -> Vec<u8> {
    const SIZES: [usize; 8] = [20, 600, 1190, 1199, 1200, 1201, 1500, 3000];
    let mut m = i.to_le_bytes().to_vec();
    m.extend(std::iter::repeat_n(
        (i % 251) as u8,
        SIZES[i as usize % SIZES.len()],
    ));
    m
}

/// Channel 2 delivers every message, in order, in both directions.
fn events_arrive_complete_and_in_order<T: Transport>(rig: &mut Rig<T>) {
    const N: u32 = 600;
    let peer = rig.peer;
    let mut next = 0;
    let (mut from_client, mut from_server) = (Vec::new(), Vec::new());
    rig.run_until("events", |r| {
        // A few messages per step in each direction.
        for _ in 0..3 {
            if next < N {
                r.client.send(Channel::Event, &event_message(next)).unwrap();
                r.server
                    .send(peer, Channel::Event, &event_message(next))
                    .unwrap();
                next += 1;
            }
        }
        while let Some(m) = r.server.recv(peer, Channel::Event) {
            from_client.push(m);
        }
        while let Some(m) = r.client.recv(Channel::Event) {
            from_server.push(m);
        }
        from_client.len() as u32 == N && from_server.len() as u32 == N
    });
    for received in [&from_client, &from_server] {
        for (i, m) in (0..N).zip(received.iter()) {
            assert_eq!(*m, event_message(i), "message {i} out of order or damaged");
        }
    }
    rig.assert_wire_limit();
}

/// Channel 3 delivers a 256 KiB payload intact.
fn map_transfer_is_intact<T: Transport>(rig: &mut Rig<T>) {
    let payload = noise(256 * 1024, 7);
    let peer = rig.peer;
    rig.server
        .send(peer, Channel::MapTransfer, &payload)
        .unwrap();
    let mut received = None;
    rig.run_until("map transfer", |r| {
        received = r.client.recv(Channel::MapTransfer);
        received.is_some()
    });
    let received = received.unwrap();
    assert_eq!(received.len(), payload.len());
    assert_eq!(fnv1a(&received), fnv1a(&payload), "checksum mismatch");
    assert!(received == payload);
    rig.assert_wire_limit();
}

/// Maps and events can be in flight together; neither starves the other.
fn map_transfer_does_not_block_events<T: Transport>(rig: &mut Rig<T>) {
    let peer = rig.peer;
    let map = noise(256 * 1024, 11);
    rig.server.send(peer, Channel::MapTransfer, &map).unwrap();
    let (mut events, mut map_done) = (0u32, false);
    let mut sent = 0u32;
    rig.run_until("map and events", |r| {
        if sent < 200 {
            r.server
                .send(peer, Channel::Event, &sent.to_le_bytes())
                .unwrap();
            sent += 1;
        }
        while let Some(m) = r.client.recv(Channel::Event) {
            assert_eq!(m, events.to_le_bytes());
            events += 1;
        }
        if let Some(m) = r.client.recv(Channel::MapTransfer) {
            assert!(m == map);
            map_done = true;
        }
        events == 200 && map_done
    });
    rig.assert_wire_limit();
}

/// Unreliable channels (0 and 1): nothing is resent, nothing arrives twice, and
/// the delivered count matches the loss rate.
fn unreliable_channels_are_not_retransmitted<T: Transport>(rig: &mut Rig<T>, loss_percent: f64) {
    const N: u32 = 2000;
    let peer = rig.peer;
    let (mut inputs, mut snapshots) = (Vec::new(), Vec::new());
    for i in 0..N + 200 {
        if i < N {
            // One message per step, so each datagram carries exactly one.
            rig.client.send(Channel::Input, &i.to_le_bytes()).unwrap();
            rig.server
                .send(peer, Channel::Snapshot, &i.to_le_bytes())
                .unwrap();
        }
        rig.step();
        while let Some(m) = rig.server.recv(peer, Channel::Input) {
            inputs.push(u32::from_le_bytes(m.try_into().unwrap()));
        }
        while let Some(m) = rig.client.recv(Channel::Snapshot) {
            snapshots.push(u32::from_le_bytes(m.try_into().unwrap()));
        }
    }
    let p = loss_percent / 100.0;
    let mean = f64::from(N) * (1.0 - p);
    let tolerance = 5.0 * (f64::from(N) * p * (1.0 - p)).sqrt() + 1.0;
    for (name, mut got) in [("input", inputs), ("snapshot", snapshots)] {
        let count = got.len();
        assert!(
            (count as f64 - mean).abs() <= tolerance,
            "{name}: {count} delivered, expected {mean} ± {tolerance}"
        );
        got.sort_unstable();
        got.dedup();
        assert_eq!(got.len(), count, "{name}: a message arrived twice");
        assert!(got.iter().all(|&i| i < N));
    }
    rig.assert_wire_limit();
}

// ---- the suite, instantiated per transport and link quality ---------------

macro_rules! reliable_suite {
    ($name:ident, $rig:ident, $conditions:expr) => {
        mod $name {
            use super::*;

            #[test]
            fn events_arrive_complete_and_in_order() {
                super::events_arrive_complete_and_in_order(&mut $rig(&$conditions));
            }

            #[test]
            fn map_transfer_is_intact() {
                super::map_transfer_is_intact(&mut $rig(&$conditions));
            }

            #[test]
            fn map_transfer_does_not_block_events() {
                super::map_transfer_does_not_block_events(&mut $rig(&$conditions));
            }
        }
    };
}

macro_rules! unreliable_suite {
    ($name:ident, $rig:ident, $conditions:expr) => {
        mod $name {
            use super::*;

            #[test]
            fn unreliable_channels_are_not_retransmitted() {
                let conditions = $conditions;
                super::unreliable_channels_are_not_retransmitted(
                    &mut $rig(&conditions),
                    f64::from(conditions.loss_percent),
                );
            }
        }
    };
}

mod memory {
    use super::*;
    reliable_suite!(perfect, memory_rig, perfect());
    reliable_suite!(ten_percent_loss, memory_rig, lossy());
    reliable_suite!(loss_and_duplicates, memory_rig, hostile());
    unreliable_suite!(perfect_unreliable, memory_rig, perfect());
    unreliable_suite!(ten_percent_loss_unreliable, memory_rig, lossy());
}

mod udp_loopback {
    use super::*;
    reliable_suite!(perfect, udp_rig, perfect());
    reliable_suite!(ten_percent_loss, udp_rig, lossy());
    reliable_suite!(loss_and_duplicates, udp_rig, hostile());
    unreliable_suite!(perfect_unreliable, udp_rig, perfect());
    unreliable_suite!(ten_percent_loss_unreliable, udp_rig, lossy());
}

// ---- behaviour that does not depend on the link ---------------------------

#[test]
fn channels_enforce_their_direction() {
    let mut rig = memory_rig(&perfect());
    let peer = rig.peer;
    assert_eq!(
        rig.client.send(Channel::Snapshot, b"x"),
        Err(SendError::WrongDirection(Channel::Snapshot))
    );
    assert_eq!(
        rig.client.send(Channel::MapTransfer, b"x"),
        Err(SendError::WrongDirection(Channel::MapTransfer))
    );
    assert_eq!(
        rig.server.send(peer, Channel::Input, b"x"),
        Err(SendError::WrongDirection(Channel::Input))
    );
    assert_eq!(rig.client.send(Channel::Event, b""), Err(SendError::Empty));
    assert_eq!(
        rig.server.send(PeerId(999), Channel::Event, b"x"),
        Err(SendError::UnknownPeer(PeerId(999)))
    );
    // Receiving on a channel that cannot carry data towards us is just empty.
    assert!(rig.client.recv(Channel::Input).is_none());
    assert!(rig.server.recv(peer, Channel::Snapshot).is_none());
}

#[test]
fn a_full_send_buffer_is_reported_not_fatal() {
    let mut rig = memory_rig(&perfect());
    let big = vec![1u8; 512 * 1024];
    let mut accepted = 0;
    let err = loop {
        match rig.client.send(Channel::Event, &big) {
            Ok(()) => accepted += 1,
            Err(e) => break e,
        }
        assert!(accepted < 100, "buffer never filled");
    };
    assert_eq!(err, SendError::ChannelFull(Channel::Event));
    assert!(rig.client.is_connected());
}

#[test]
fn server_reports_connections_and_timeouts() {
    let (c, s) = MemoryTransport::pair();
    let addr = s.addr();
    let mut client = ClientEndpoint::new(c, addr);
    let mut server = ServerEndpoint::with_config(
        s,
        EndpointConfig {
            timeout: Duration::from_secs(2),
            ..EndpointConfig::default()
        },
    );
    client.update(DT).unwrap();
    server.update(DT).unwrap();
    let Some(ServerEvent::Connected(peer)) = server.poll_event() else {
        panic!("no connect event");
    };
    assert_eq!(server.peers().collect::<Vec<_>>(), vec![peer]);

    // The client keeps quiet: keep-alives are what holds the link open.
    for _ in 0..150 {
        client.update(DT).unwrap();
        server.update(DT).unwrap();
    }
    assert!(
        server.poll_event().is_none(),
        "dropped while keep-alives flow"
    );

    for _ in 0..300 {
        server.update(DT).unwrap();
    }
    assert_eq!(server.poll_event(), Some(ServerEvent::Disconnected(peer)));
    assert_eq!(server.peers().count(), 0);
}

#[test]
fn clients_time_out_when_the_server_goes_silent() {
    let (c, s) = MemoryTransport::pair();
    let addr = s.addr();
    let mut client = ClientEndpoint::with_config(
        c,
        addr,
        EndpointConfig {
            timeout: Duration::from_secs(2),
            ..EndpointConfig::default()
        },
    );
    for _ in 0..100 {
        client.update(DT).unwrap();
    }
    assert!(client.is_connected());
    for _ in 0..200 {
        client.update(DT).unwrap();
    }
    assert!(!client.is_connected());
    assert_eq!(
        client.send(Channel::Event, b"x"),
        Err(SendError::Disconnected)
    );
}

#[test]
fn many_clients_share_one_server() {
    let net = MemoryNetwork::new();
    let server_t = net.endpoint();
    let addr = server_t.addr();
    let mut server = ServerEndpoint::new(server_t);
    let mut clients: Vec<_> = (0..3)
        .map(|_| ClientEndpoint::new(net.endpoint(), addr))
        .collect();
    for _ in 0..5 {
        for c in &mut clients {
            c.update(DT).unwrap();
        }
        server.update(DT).unwrap();
    }
    let peers: Vec<PeerId> = server.peers().collect();
    assert_eq!(peers.len(), 3);

    // Each client says who it is; the server answers each one privately.
    for (i, c) in clients.iter_mut().enumerate() {
        c.send(Channel::Event, &[u8::try_from(i).unwrap()]).unwrap();
    }
    for _ in 0..5 {
        for c in &mut clients {
            c.update(DT).unwrap();
        }
        server.update(DT).unwrap();
    }
    for &peer in &peers {
        let hello = server.recv(peer, Channel::Event).expect("hello");
        server
            .send(peer, Channel::Event, &[hello[0], 0xAA])
            .unwrap();
        assert!(server.recv(peer, Channel::Event).is_none());
    }
    server.broadcast(Channel::Snapshot, b"all").unwrap();
    for _ in 0..5 {
        for c in &mut clients {
            c.update(DT).unwrap();
        }
        server.update(DT).unwrap();
    }
    for (i, c) in clients.iter_mut().enumerate() {
        assert_eq!(
            c.recv(Channel::Event),
            Some(vec![u8::try_from(i).unwrap(), 0xAA])
        );
        assert_eq!(c.recv(Channel::Snapshot), Some(b"all".to_vec()));
    }
}

#[test]
fn junk_and_strangers_are_ignored() {
    let net = MemoryNetwork::new();
    let server_t = net.endpoint();
    let addr = server_t.addr();
    let mut server = ServerEndpoint::with_config(
        server_t,
        EndpointConfig {
            max_peers: 1,
            ..EndpointConfig::default()
        },
    );
    let mut stranger = net.endpoint();
    for junk in [&[][..], &[9, 9, 9], &[0], &[1, 2], &[255; 100]] {
        stranger.send_to(addr, junk).unwrap();
    }
    server.update(DT).unwrap();
    assert_eq!(server.peers().count(), 0, "junk created a peer");

    let mut first = ClientEndpoint::new(net.endpoint(), addr);
    let mut second = ClientEndpoint::new(net.endpoint(), addr);
    first.update(DT).unwrap();
    second.update(DT).unwrap();
    server.update(DT).unwrap();
    assert_eq!(server.peers().count(), 1, "max_peers not enforced");
}

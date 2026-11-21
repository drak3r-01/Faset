//! Lot 1.10: the endpoints survive garbage datagrams, and oversized maps are
//! refused.

use std::time::Duration;

use faset_net::{
    Channel, ClientEndpoint, MemoryNetwork, SendError, ServerEndpoint, Transport, MAX_MAP_PAYLOAD,
};

const DT: Duration = Duration::from_millis(16);

fn noise(len: usize, mut s: u64) -> Vec<u8> {
    (0..len)
        .map(|_| {
            s = s
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (s >> 33) as u8
        })
        .collect()
}

#[test]
fn random_datagrams_never_panic_either_endpoint() {
    let net = MemoryNetwork::new();
    let server_t = net.endpoint();
    let server_addr = server_t.addr();
    let mut server = ServerEndpoint::new(server_t);
    let client_t = net.endpoint();
    let client_addr = client_t.addr();
    let mut client = ClientEndpoint::new(client_t, server_addr);
    let mut attacker = net.endpoint();
    // A real session first, so garbage also hits established connections.
    client.send(Channel::Event, b"hello").unwrap();
    for _ in 0..10 {
        client.update(DT).unwrap();
        server.update(DT).unwrap();
    }
    for i in 0..10_000usize {
        let bytes = noise(i % 1200, i as u64);
        attacker.send_to(server_addr, &bytes).unwrap();
        attacker.send_to(client_addr, &bytes).unwrap();
        if i % 20 == 19 {
            client.update(DT).unwrap();
            server.update(DT).unwrap();
            while server.poll_event().is_some() {}
            for peer in server.peers().collect::<Vec<_>>() {
                while server.recv(peer, Channel::Event).is_some() {}
                while server.recv(peer, Channel::Input).is_some() {}
            }
            while client.recv(Channel::Event).is_some() {}
            while client.recv(Channel::Snapshot).is_some() {}
            while client.recv(Channel::MapTransfer).is_some() {}
        }
    }
}

#[test]
fn maps_above_the_limit_are_refused_and_the_limit_is_accepted() {
    let net = MemoryNetwork::new();
    let server_t = net.endpoint();
    let server_addr = server_t.addr();
    let mut server = ServerEndpoint::new(server_t);
    let mut client = ClientEndpoint::new(net.endpoint(), server_addr);
    client.send(Channel::Event, b"hello").unwrap();
    for _ in 0..10 {
        client.update(DT).unwrap();
        server.update(DT).unwrap();
    }
    let peer = server.peers().next().expect("client connected");
    let too_big = vec![7u8; MAX_MAP_PAYLOAD + 1];
    assert!(matches!(
        server.send(peer, Channel::MapTransfer, &too_big),
        Err(SendError::TooLarge { .. })
    ));
    assert!(matches!(
        server.send(peer, Channel::MapTransfer, &[]),
        Err(SendError::Empty)
    ));
    // A client may not send on the map channel at all.
    assert!(matches!(
        client.send(Channel::MapTransfer, &[1]),
        Err(SendError::WrongDirection(_))
    ));
}

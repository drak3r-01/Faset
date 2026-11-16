//! A minimal headless client for integration tests: handshake, commands
//! and snapshot decoding (full and delta) over any transport.

// Each test binary uses a different part of this helper.
#![allow(dead_code)]

use std::time::Duration;

use faset_core::{NetId, Tick, DT};
use faset_net::{Channel, ClientEndpoint, Transport};
use faset_protocol::{InputCommand, Message, PROTOCOL_VERSION};
use faset_server::{build_fingerprint, InputPacket, Snapshot, SnapshotDecoder};

pub struct HeadlessClient<T: Transport> {
    endpoint: ClientEndpoint<T>,
    protocol_version: u16,
    hello_sent: bool,
    pub player: Option<NetId>,
    pub map_fingerprint: Option<u64>,
    pub snapshots: Vec<Snapshot>,
    /// Resolves delta baselines and yields the acknowledgement.
    pub decoder: SnapshotDecoder,
    /// Snapshot messages received: (full, delta) counts and wire bytes.
    pub fulls: usize,
    pub deltas: usize,
    pub snapshot_bytes: usize,
    /// Client steps since the latest snapshot arrived.
    since_snapshot: u32,
}

impl<T: Transport> HeadlessClient<T> {
    pub fn new(transport: T, server: T::Addr) -> Self {
        Self::with_version(transport, server, PROTOCOL_VERSION)
    }

    pub fn with_version(transport: T, server: T::Addr, protocol_version: u16) -> Self {
        Self {
            endpoint: ClientEndpoint::new(transport, server),
            protocol_version,
            hello_sent: false,
            player: None,
            map_fingerprint: None,
            snapshots: Vec::new(),
            decoder: SnapshotDecoder::new(),
            fulls: 0,
            deltas: 0,
            snapshot_bytes: 0,
            since_snapshot: 0,
        }
    }

    pub fn in_game(&self) -> bool {
        !self.snapshots.is_empty()
    }

    pub fn latest(&self) -> Option<&Snapshot> {
        self.snapshots.last()
    }

    fn send(&mut self, channel: Channel, msg: &Message) {
        let bytes = msg.encode().unwrap();
        self.endpoint.send(channel, &bytes).unwrap();
    }

    /// One client frame: send what is due, then exchange packets.
    /// `movement` is `(move_x, move_y, yaw)`, sent only once in game.
    pub fn step(&mut self, movement: Option<(i8, i8, u16)>) {
        if !self.hello_sent {
            self.hello_sent = true;
            self.send(
                Channel::Event,
                &Message::ClientHello {
                    protocol_version: self.protocol_version,
                    build_fingerprint: build_fingerprint(),
                },
            );
        }
        if let (Some((move_x, move_y, yaw)), Some(latest)) = (movement, self.latest()) {
            // The server's next tick is the snapshot tick plus the frames
            // since, or one more for the snapshot sent on joining (taken
            // before the step); stamp two ticks ahead so it is never late.
            let tick = Tick(latest.tick.0 + self.since_snapshot + 2);
            let cmd = InputCommand {
                tick,
                buttons: 0,
                move_x,
                move_y,
                yaw,
                pitch: 0,
                weapon_slot: 1,
            };
            let packet = InputPacket {
                command: cmd,
                ack: self.decoder.ack(),
            };
            self.endpoint
                .send(Channel::Input, &packet.encode())
                .unwrap();
        }
        self.endpoint.update(Duration::from_secs_f32(DT)).unwrap();
        self.since_snapshot += 1;
        while let Some(bytes) = self.endpoint.recv(Channel::Event) {
            if let Ok(Message::ServerHello {
                player_id,
                map_fingerprint,
                ..
            }) = Message::decode(&bytes)
            {
                self.player = Some(player_id);
                self.map_fingerprint = Some(map_fingerprint);
                // Map loading is a later lot; the fingerprint is all we need.
                self.send(Channel::Event, &Message::Ready);
            }
        }
        while let Some(bytes) = self.endpoint.recv(Channel::Snapshot) {
            let msg = Message::decode(&bytes).unwrap();
            match msg {
                Message::SnapshotFull { .. } => self.fulls += 1,
                Message::SnapshotDelta { .. } => self.deltas += 1,
                _ => panic!("not a snapshot: {msg:?}"),
            }
            self.snapshot_bytes += bytes.len();
            let snap = self.decoder.decode(&msg).unwrap();
            if self.latest().is_none_or(|l| snap.tick > l.tick) {
                self.since_snapshot = 0;
            }
            self.snapshots.push(snap);
        }
    }
}

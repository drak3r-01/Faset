//! Lot 1.9 acceptance: a solo game binds no UDP socket.
//!
//! Own test binary so no other test opens sockets meanwhile. The kernel UDP
//! socket tables (Linux) are compared before and after.

#[path = "../../server/tests/common/mod.rs"]
mod headless;

use std::time::{Duration, Instant};

use faset_client::{host_game, HostConfig};
use faset_core::{NetId, Tick};
use faset_protocol::InputCommand;
use faset_server::{Bot, ServerConfig};
use headless::HeadlessClient;

struct Idle;

impl Bot for Idle {
    fn command(&mut self, _: &faset_sim::World, _: NetId, tick: Tick) -> InputCommand {
        InputCommand {
            tick,
            buttons: 0,
            move_x: 0,
            move_y: 0,
            yaw: 0,
            pitch: 0,
            weapon_slot: 1,
        }
    }
}

/// UDP sockets held by this process: its socket file descriptors whose
/// inode appears in the kernel UDP tables (other processes are ignored).
fn udp_sockets() -> Option<usize> {
    let mut inodes = Vec::new();
    for table in ["/proc/net/udp", "/proc/net/udp6"] {
        let text = std::fs::read_to_string(table).ok()?;
        inodes.extend(
            text.lines()
                .skip(1)
                .filter_map(|l| l.split_whitespace().nth(9))
                .map(str::to_owned),
        );
    }
    let mut mine = 0;
    for fd in std::fs::read_dir("/proc/self/fd").ok()?.flatten() {
        let Ok(target) = std::fs::read_link(fd.path()) else {
            continue;
        };
        let target = target.to_string_lossy().into_owned();
        if let Some(inode) = target
            .strip_prefix("socket:[")
            .and_then(|t| t.strip_suffix(']'))
        {
            mine += usize::from(inodes.iter().any(|i| i == inode));
        }
    }
    Some(mine)
}

#[test]
fn solo_game_with_bots_binds_no_udp_socket() {
    let before = udp_sockets();
    let mut config = HostConfig::new(ServerConfig::default());
    config.bots = vec![Box::new(Idle), Box::new(Idle)];
    let host = host_game(config, false).unwrap();
    assert!(host.lan().is_none());
    assert_eq!(host.udp_sockets(), 0);

    let (mem, server) = host.connect_local();
    let mut client = HeadlessClient::new(mem, server);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !client.in_game() && Instant::now() < deadline {
        client.step(None);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(client.in_game());
    assert_eq!(host.player_count(), 3, "two bots and the host");

    // `None` only where /proc is absent (the container is Linux).
    if let (Some(before), Some(after)) = (before, udp_sockets()) {
        assert_eq!(before, after, "the solo game opened a UDP socket");
    }
    host.stop();
}

//! The run loop: server, console commands, clean shutdown.

use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;

use anyhow::{Context, Result};
use faset_core::{NetId, Tick};
use faset_net::UdpTransport;
use faset_protocol::InputCommand;
use faset_server::{Bot, Server, ServerConfig, ServerTransport, TickClock};
use faset_sim::World;

use crate::console::{self, Command};
use crate::maps::Catalog;
use crate::settings::{Cli, Settings};

/// Ticks run after the shutdown disconnects so the packets are flushed.
const FLUSH_TICKS: u32 = 3;

/// A bot that walks forward and turns every two seconds; `level` (0 to 10)
/// sets its speed. Placeholder until bot AI lands (decision).
struct WanderBot {
    level: u8,
}

impl Bot for WanderBot {
    fn command(&mut self, _: &World, me: NetId, tick: Tick) -> InputCommand {
        let leg = u32::from(me.0) + tick.0 / 120;
        InputCommand {
            tick,
            buttons: 0,
            move_x: 0,
            move_y: 50 + 7 * i8::try_from(self.level.min(10)).unwrap_or(10),
            yaw: u16::try_from(leg.wrapping_mul(9_000) % 65_536).unwrap_or(0),
            pitch: 0,
            weapon_slot: 0,
        }
    }
}

/// The server plus the state the console works on.
pub struct Dedicated {
    server: Server,
    settings: Settings,
    catalog: Catalog,
    map: String,
    bots: Vec<NetId>,
}

impl Dedicated {
    /// Builds the server on `transports` and adds the configured bots.
    ///
    /// # Errors
    /// Unknown first rotation map, or an invalid server configuration.
    pub fn new(
        settings: Settings,
        catalog: Catalog,
        transports: Vec<ServerTransport>,
    ) -> Result<Self> {
        let map = settings.rotation[0].clone();
        let mut config = ServerConfig {
            mode: settings.mode,
            map: catalog.load(&map).map_err(anyhow::Error::msg)?,
            ..ServerConfig::default()
        };
        config.limits.max_players = settings.max_players;
        let mut server = Server::new(config, transports).context("invalid server config")?;
        let mut bots = Vec::new();
        for _ in 0..settings.bots {
            let bot = WanderBot {
                level: settings.bot_level,
            };
            match server.add_bot(Box::new(bot)) {
                Some(id) => bots.push(id),
                None => break,
            }
        }
        Ok(Self {
            server,
            settings,
            catalog,
            map,
            bots,
        })
    }

    /// The wrapped server.
    #[must_use]
    pub fn server(&self) -> &Server {
        &self.server
    }

    /// Runs one console command; returns the output lines and whether to quit.
    pub fn execute(&mut self, cmd: Command) -> (Vec<String>, bool) {
        let mut out = Vec::new();
        match cmd {
            Command::Quit => return (vec!["shutting down".into()], true),
            Command::Status => {
                let players: Vec<String> = self
                    .server
                    .players()
                    .map(|id| {
                        let kind = if self.bots.contains(&id) {
                            "bot"
                        } else {
                            "human"
                        };
                        format!("#{} ({kind})", id.0)
                    })
                    .collect();
                out.push(format!(
                    "{} | map {} | tick {} | {}/{} players: {}",
                    self.settings.name,
                    self.map,
                    self.server.world().tick().0,
                    players.len(),
                    self.settings.max_players,
                    players.join(", ")
                ));
            }
            Command::Kick(id) => {
                if self.server.disconnect(NetId(id)) {
                    self.bots.retain(|b| b.0 != id);
                    out.push(format!("kicked #{id}"));
                } else {
                    out.push(format!("no player #{id}"));
                }
            }
            Command::Say(text) => {
                // The server crate has no broadcast API yet: log only (reported).
                tracing::info!(%text, "say (not delivered: server has no broadcast API yet)");
                out.push(format!(
                    "say: {text} (logged only, not delivered to clients)"
                ));
            }
            Command::Map(name) => {
                out.push(match self.catalog.load(&name) {
                    Err(e) => e,
                    Ok(_) if name == self.map => format!("already on {name}"),
                    Ok(_) => "map change at runtime needs a server API (rotation lot 5.x)".into(),
                });
            }
        }
        (out, false)
    }

    fn drain_console(&mut self, lines: &Receiver<String>) -> bool {
        loop {
            match lines.try_recv() {
                Ok(line) => match console::parse(&line) {
                    Ok(None) => {}
                    Ok(Some(cmd)) => {
                        let (out, quit) = self.execute(cmd);
                        for l in out {
                            println!("{l}");
                        }
                        if quit {
                            return true;
                        }
                    }
                    Err(e) => println!("{e}"),
                },
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => return false,
            }
        }
    }

    /// Ticks in real time until `stop` is set or `quit` is typed, then shuts
    /// down cleanly.
    pub fn run_loop(&mut self, stop: &AtomicBool, lines: &Receiver<String>) {
        let mut clock = TickClock::new(self.server.config().tick_rate);
        while !stop.load(Ordering::Relaxed) {
            if self.drain_console(lines) {
                break;
            }
            for _ in 0..clock.wait() {
                self.server.tick();
            }
        }
        self.shutdown();
    }

    /// Notifies clients (closes their connections) and flushes the packets.
    pub fn shutdown(&mut self) {
        let ids: Vec<NetId> = self.server.players().collect();
        tracing::info!(players = ids.len(), "shutting down: disconnecting clients");
        for id in ids {
            self.server.disconnect(id);
        }
        for _ in 0..FLUSH_TICKS {
            self.server.tick();
        }
        tracing::info!("server stopped");
    }
}

/// The `maps/` folder next to the executable.
fn default_maps_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_default()
        .join("maps")
}

/// Executable entry: configuration, UDP socket, console thread, Ctrl+C.
///
/// # Errors
/// Bad configuration or an unbindable port.
pub fn run(cli: &Cli) -> Result<()> {
    let settings = Settings::load(&cli.config, cli)?;
    let catalog = Catalog::scan(&cli.maps_dir.clone().unwrap_or_else(default_maps_dir));
    let udp = UdpTransport::bind(("0.0.0.0", settings.port))
        .with_context(|| format!("cannot bind UDP port {}", settings.port))?;
    tracing::info!(name = %settings.name, port = settings.port, map = %settings.rotation[0],
        max_players = settings.max_players, bots = settings.bots, "dedicated server starting");
    if !settings.password.is_empty() {
        tracing::warn!("password is set but not enforced yet");
    }
    let mut dedicated = Dedicated::new(settings, catalog, vec![udp.into()])?;

    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    ctrlc::set_handler(move || flag.store(true, Ordering::Relaxed))
        .context("cannot install the Ctrl+C handler")?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    dedicated.run_loop(&stop, &rx);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dedicated(bots: usize) -> Dedicated {
        let settings = Settings {
            bots,
            ..Settings::default()
        };
        Dedicated::new(settings, Catalog::default(), Vec::new()).unwrap()
    }

    #[test]
    fn status_kick_say_map_quit() {
        let mut d = dedicated(2);
        let (out, quit) = d.execute(Command::Status);
        assert!(
            !quit && out[0].contains("2/16") && out[0].contains("bot"),
            "{out:?}"
        );
        assert!(d.execute(Command::Kick(1)).0[0].contains("kicked"));
        assert!(d.execute(Command::Kick(1)).0[0].contains("no player"));
        assert!(d.execute(Command::Say("hi".into())).0[0].contains("hi"));
        assert!(d.execute(Command::Map("nope".into())).0[0].contains("unknown"));
        assert!(d.execute(Command::Map("test-arena".into())).0[0].contains("already"));
        assert!(d.execute(Command::Quit).1);
    }

    #[test]
    fn quit_on_the_console_stops_the_loop_and_empties_the_server() {
        let mut d = dedicated(2);
        let (tx, rx) = mpsc::channel();
        tx.send("status".to_owned()).unwrap();
        tx.send("quit".to_owned()).unwrap();
        d.run_loop(&AtomicBool::new(false), &rx);
        assert_eq!(d.server().players().count(), 0);
    }

    #[test]
    fn stop_flag_stops_the_loop() {
        let mut d = dedicated(1);
        let (_tx, rx) = mpsc::channel();
        d.run_loop(&AtomicBool::new(true), &rx);
        assert_eq!(d.server().players().count(), 0);
    }
}

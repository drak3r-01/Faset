# Server guide

The dedicated server (`dedicated`) is a headless Faset server: no window, no
graphics, no audio. It runs the simulation, accepts players over UDP and is
configured by a `server.toml` file, command-line options, and a console on
standard input.

> Status: pre-alpha. Some options are stored but not enforced yet; they are marked
> below.

## Running the server

### Natively

Build the release binary inside Docker (see the [developer guide](developer-guide.md))
and run `dedicated` from the folder that holds `server.toml`:

```text
dedicated --config server.toml
```

The server listens on UDP port 7777 by default. Extra maps are read from a `maps/`
folder next to the executable (or from `--maps-dir`).

### In Docker (`server-runtime` image)

The `server-runtime` image is a Debian slim image that contains only the `dedicated`
binary. It runs with `/srv` as working directory: the configuration is `/srv/server.toml`
and extra maps are read from `/srv/maps`. It publishes UDP port 7777.

```text
docker run --rm -it -p 7777:7777/udp -v "$PWD/server.toml:/srv/server.toml" -v "$PWD/maps:/srv/maps" faset-server
```

On a development machine, `.\dev.ps1 run-server [dedicated args]` builds the
`server` compose service and starts it with UDP 7777 published and the console
attached to your terminal.

Stop the server by typing `quit` in its console, or with Ctrl+C. Both disconnect the
players cleanly before the process exits.

## Command-line options

Every option overrides the same key of `server.toml`.

| Option | Key | Meaning |
|---|---|---|
| `--config <file>` | | Configuration file to read if it exists (default `server.toml`). |
| `--name <text>` | `name` | Server name, shown in logs. |
| `--port <n>` | `port` | UDP port to listen on (default 7777). |
| `--max-players <n>` | `max_players` | Maximum players, humans and bots together (1 to 64). |
| `--mode <n>` | `mode` | Game mode id sent in the handshake when no game mode is hosted. |
| `--game-mode <id>` | `game_mode` | Hosted mode: `fps.ffa` or `fps.team`. Empty for none. |
| `--seed <n>` | `seed` | World seed (weapon spread, respawn draws). |
| `--rotation <a,b,c>` | `rotation` | Maps played in order, comma separated. |
| `--rotation-seconds <n>` | `rotation_seconds` | Seconds per map (the match length when a mode is hosted). |
| `--rotation-order <order>` | `rotation_order` | `sequential` or `random` (each map once before any repeat). |
| `--bots <n>` | `bots` | Number of bots. |
| `--bot-level <0-10>` | `bot_level` | Bot skill. |
| `--password <text>` | `password` | Join password. Stored only, not enforced yet. |
| `--maps-dir <folder>` | | Folder of extra map files. |

Use `dedicated --help` for the full list generated from the code.

## server.toml

Every key is optional. Unknown keys are rejected, so a typing mistake is reported
at start-up. A complete example with the defaults is `crates/dedicated/server.toml`
in the repository.

```toml
name = "Faset server"
port = 7777
max_players = 16          # humans and bots together, 1 to 64
mode = 0
game_mode = ""            # "fps.ffa", "fps.team", or "" for free movement
seed = 0
rotation = ["test-arena"] # names of embedded maps or of files in maps/
rotation_seconds = 300
rotation_order = "sequential"  # or "random"
bots = 0
bot_level = 5             # 0 to 10
password = ""             # stored only: not enforced yet
```

| Key | Default | Notes |
|---|---|---|
| `name` | `Faset server` | |
| `port` | `7777` | UDP. |
| `max_players` | `16` | Must be at least `bots`. |
| `mode` | `0` | Used when `game_mode` is empty. |
| `game_mode` | `""` | `fps.ffa` (free-for-all) or `fps.team` (team deathmatch). With a mode, the server runs a match on each map. |
| `seed` | `0` | |
| `rotation` | `["test-arena"]` | At least one map. Names resolve first against the embedded maps (`test-arena`, `example`), then against files in `maps/` (the file name without extension). |
| `rotation_seconds` | `300` | Must be positive. |
| `rotation_order` | `sequential` | |
| `bots` | `0` | |
| `bot_level` | `5` | 0 to 10. Levels 0-2 easy, 3-5 normal, 6-8 hard, 9-10 expert. |
| `password` | `""` | Not enforced yet. Players are not asked for it. |

## Console commands

Type one command per line in the server's console:

| Command | Effect |
|---|---|
| `status` | Prints the server name, the current map, the tick, and the players (humans and bots) with their ids. |
| `kick <id>` | Disconnects the player with that id. The id can be written `#3`. Bots can be kicked too. |
| `map <name>` | Checks that the map exists. Changing the map by hand is **not supported yet**: the rotation switches maps. |
| `say <text>` | Writes the text to the log. **Not delivered to players yet**: the server has no broadcast message yet. |
| `quit` (also `exit`, `stop`) | Disconnects every player and stops the server. |

An unknown command prints the list of valid commands.

## Networking

| Port | Protocol | Use |
|---|---|---|
| 7777 | UDP | Game traffic. Players connect to it. Configured by `port`. |
| 7778 | UDP | LAN discovery. Used by the client's server browser to list games on the local network. It is answered by a player who hosts a game in the client; the dedicated server does not answer discovery requests yet. |

Logs use the `RUST_LOG` environment variable (for example `RUST_LOG=debug`);
the default level is `info`.

### Local network play

Players on the same network can connect with the server's LAN address and port
(for example `192.168.1.20:7777`). Allow UDP 7777 through the server's firewall.

### Internet play: manual port forwarding

Faset has no master server and no automatic NAT traversal. To let players from
outside your network join:

1. Give the server computer a fixed local address (a DHCP reservation in the router).
2. In the router, forward **UDP port 7777** to that local address.
3. Allow UDP 7777 in the server computer's firewall.
4. Give your public IP address to the players, who connect with `public-ip:7777`.

Your public IP address may change; check it again before a session.

## Operating notes

- Run the server on a stable machine; the match state is in memory and is lost when
  the process stops.
- The rotation changes map every `rotation_seconds`. With a single map and no mode,
  there is no rotation.
- Logs go to standard output. The console reads standard input, so an unattended
  server (for example under a process manager) should be started with its input
  closed or from an attached terminal.

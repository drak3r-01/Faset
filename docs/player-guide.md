# Player guide

Faset is a fast, low-poly multiplayer FPS. This guide covers starting the game,
the controls, the settings, and hosting and joining games on your local network.

> Status: pre-alpha. Some features listed in the game are not finished yet; the
> limitations are noted below.

## Starting the game

Run `faset.exe` (Windows) or the `faset` binary of the release archive. The main
menu offers:

- **Play**: host a game (you run the server inside your client).
- **Join**: connect to a server by address, from favourites, recent servers or
  the list of servers on the local network.
- **Classes**: choose your base class and loadout.
- **Editor**: the map editor (in development).
- **Settings**: display, quality, controls, audio, network, game and accessibility.
- **Quit**.

Command-line options of the client:

| Option | Effect |
|---|---|
| `--settings <file>` | Read and write this settings file instead of the default one. |
| `--safe` | Start with safe display settings (use it if the game does not start correctly). |
| `--no-vsync` | Turn vertical sync off for this run. |
| `--maps <folder>` | Folder of map files (default `assets/maps`). |
| `--seconds <n>` | Close the client after `n` seconds (for automated runs). |

Your settings are stored in `settings.toml` in the Faset folder of your user
profile (`%APPDATA%\Faset` on Windows). Favourites and recent servers are kept in
the same folder.

## Controls

The default key bindings are below. Every action can be changed in
**Settings > Controls**, where a conflict between two actions is reported.

| Action | Default |
|---|---|
| Move forward / backward | `W` / `S` |
| Strafe left / right | `A` / `D` |
| Jump | `Space` (hold to keep jumping, if the option is on) |
| Crouch | `Left Ctrl` (hold, or toggle with the option) |
| Fire | Left mouse button |
| Aim down sights | Right mouse button (hold, or toggle with the option) |
| Reload | `R` |
| Next / previous weapon | `F` / `Q` (also the mouse wheel) |
| Weapon slot 1 / 2 / 3 | `1` / `2` / `3` |
| Scoreboard | `Tab` (hold) |
| Chat | `T` |
| Use | `E` (no effect yet) |
| Sprint | `Left Shift` (no effect yet) |

Notes:

- Sprint and Use are bound but not sent to the server yet.
- Chat is bound but the chat window is not available yet.
- The pause menu (`Escape`) offers Resume, Classes, Settings and Leave match.
  Escape cannot be bound to another action.

## Settings

Settings are grouped in categories:

- **Display**: window mode (windowed, borderless, fullscreen), resolution, monitor,
  vertical sync, FPS cap, field of view, brightness.
- **Quality**: render scale, upscaler (native, TAA, FSR 1, and FSR 3 or DLSS where
  supported), upscale level, sharpness, anti-aliasing, shadows, ambient occlusion,
  bloom, outlines, weather density, screen effects, draw distance.
- **Controls**: mouse sensitivity, aim sensitivity, mouse DPI, invert vertical axis,
  crouch and aim as toggles, hold to keep jumping, gamepad, key bindings.
- **Audio**: master, effects and interface volumes; output device; key sound.
- **Network**: pseudonym (the name shown to other players), snapshot frequency,
  interpolation delay (automatic or manual), game port, local network discovery,
  network graph.
- **Game**: reticle style, size, thickness, gap and opacity; HUD scale; speed
  indicator; damage numbers; weapon position.
- **Accessibility**: colour-blind filter, reduce shake, reduce flashes, interface
  scale.
- **Language**: English or Français.

Changing the resolution asks you to confirm it within a few seconds; otherwise the
previous resolution comes back.

## Hosting a game

Use **Play** to host. You choose:

- the map and the mini-game (free-for-all or team deathmatch);
- the maximum number of players (humans and bots together, up to 16);
- the number of bots and their level (0 to 10);
- whether to rotate through every map;
- **Open to the local network**, so other players on your Wi-Fi or LAN can find
  the game.

The password field is saved with the match settings but is **not enforced yet**:
anyone who knows the address can join. Do not rely on it.

Your own client is the server when you host. The game listens on UDP port **7777**.
If the Windows firewall asks, allow Faset to access private networks.

## Joining a game

Use **Join** and choose one of:

- **Address**: type `host` or `host:port`. The default port is **7777**.
- **Favourites**: servers you marked with *Favourite*.
- **Recent servers**: the last servers you connected to.
- **Servers on the local network**: the list of hosts that answer on the local
  network (UDP port **7778**, discovery is automatic while the list is open).
  The host must have chosen to open the game to the local network.

Press **Connect** to join. If the connection fails, check the address and port,
and that the host has not closed the game or changed its port.

To play over the internet, the host must forward UDP port 7777 on the router
(see the [server guide](server-guide.md#internet-play-manual-port-forwarding)).
There is no matchmaking and no automatic NAT traversal: you connect by IP address.

## Troubleshooting

- **The game does not start or is black**: start with `faset --safe`, then check
  the render settings. Update your graphics driver.
- **Lag or jumps while playing**: lower the render scale and the draw distance, or
  set the interpolation delay to automatic. The network graph (Settings > Network)
  shows your round-trip time and packet loss.
- **Server not listed on the local network**: both computers must be on the same
  subnet, the host must have opened the game to the local network, and the
  firewall must allow UDP 7777 and 7778.

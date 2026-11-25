<div align="center">

# ◆ FASET

**A fast, low-poly, multiplayer FPS — 100% Rust, raw Vulkan, procedural everything.**

![Rust](https://img.shields.io/badge/Rust-2024-orange?logo=rust)
![Vulkan](https://img.shields.io/badge/Vulkan-1.3-red?logo=vulkan)
![Docker](https://img.shields.io/badge/build-Docker--only-2496ED?logo=docker&logoColor=white)
![Status](https://img.shields.io/badge/status-pre--alpha-yellow)
![Platforms](https://img.shields.io/badge/targets-Windows%20%7C%20Linux%20%7C%20macOS-lightgrey)

*Bunny hops, slide hops, rotating maps every 5 minutes, up to 16 players (bots included).*

</div>

---

## Vision

Faset is a movement-first arena shooter. Every prop, character, weapon, sky and weather
effect is generated procedurally, so the whole game stays tiny, stylised and moddable.
The simulation is deterministic and server-authoritative; the renderer is a deliberately
simple forward Vulkan engine that runs on desktop GPUs, software Vulkan (lavapipe) and
MoltenVK.

## Highlights (planned)

| Area | What to expect |
|---|---|
| **Movement** | Source-style air acceleration, bunny hop, slide hop, surf-friendly collision |
| **Netcode** | Fixed-tick sim, delta snapshots, client prediction + reconciliation, lag compensation, UDP behind a `Transport` trait |
| **Servers** | Embedded (host a game), LAN discovery, headless dedicated server in a container |
| **Rendering** | Raw Vulkan (`ash`), WGSL shaders compiled via `naga`, stylised lighting, cascaded shadows, procedural sky and weather |
| **Upscaling** | FSR 1 rewritten as shaders (universal); FSR 2/3 and DLSS via FFI (Windows x86_64) |
| **Content** | Procedural props, characters and weapons; RON maps with a validator and built-in editor |
| **Modes** | Mini-game framework; first modes: free-for-all and team deathmatch |
| **Bots** | Navigation graph, pathfinding, perception, aim and difficulty levels |

## Architecture

Dependencies only point downwards: the simulation knows neither rendering nor the concrete
network, and the dedicated server never pulls graphics crates (enforced by a `cargo tree` test).

```
core ─┬─ protocol ─┬─ net
      │            └─ sim ── minigame-api ─┬─ mode-fps
      ├─ mapfmt ───────┘                   └─ bots
      ├─ assets ── render ── upscale
      ├─ audio
      └─ ui ── editor
server (sim+net+minigame-api+bots) ── dedicated   |   client (everything but dedicated)
tools/map-tool · xtask · shaders
```

The renderer never reads the ECS world: each frame the client extracts a plain `RenderWorld`
(camera, instances with current/previous matrices, lights, ambience), which keeps rendering
testable with hand-made scenes and the simulation testable without a GPU.

## Building

> **All tooling runs in Docker. Nothing is built or installed on the host.**

The dev image and the `xtask` entry points are defined in `docker-compose.yml` and
`xtask/`. The Windows wrapper `dev.ps1` forwards any command to `cargo xtask`:

```powershell
.\dev.ps1 ci                      # the quality gate: check, test, render-test, maps, deny
.\dev.ps1 run-server --port 7777  # build and run the dedicated server container
```

The full command list and the crate map are in the [developer guide](docs/developer-guide.md).

## Documentation

- [Player guide](docs/player-guide.md): controls, settings, hosting and joining on the LAN.
- [Server guide](docs/server-guide.md): `server.toml`, command-line options, console
  commands, the `server-runtime` Docker image, ports and manual port forwarding.
- [Developer guide](docs/developer-guide.md): Docker services, `dev.ps1` and `xtask`
  commands, crate map, release pipeline.
- [Map design guide](docs/map-design-guide.md) and the [lots overview](docs/lots/OVERVIEW.md).

## Roadmap

58 lots across eight phases, each lot = one branch, one pull request.

| Milestone | Demo |
|---|---|
| **J0** | Empty build and tests in Docker, Windows binary produced |
| **J1** | Two headless clients move on a containerised dedicated server despite 5% packet loss |
| **J2** | Lit low-poly scene with sky, weather and FSR 1 |
| **J3** | First playable: bunny hop on a RON map, multiplayer |
| **J4** | Full FPS mode against bots |
| **J5** | Usable editor and twelve maps in rotation |
| **J6** | Complete menus and settings |
| **J7** | `v0.1.0` release by tag |

Phases: 0 scoping & foundation · 1 network & servers · 2 Vulkan engine & art direction ·
3 map format · 4 mini-game framework & FPS mode · 5 editor & map rotation · 6 menus, settings,
audio · 7 quality & release.

## Conventions

- Everything in the repository is in **English** (code, comments, commits, docs, asset names).
- Units: metre, second, radian. Right-handed, Y up, −Z forward.
- Data in RON, user settings in TOML. Conventional Commits.
- Errors: `thiserror` in libraries, `anyhow` in executables.
- Quality gate before merge: `cargo xtask ci` inside Docker.

## Status

Pre-alpha. Current version: `0.1.0`.

## License

Faset is free software licensed under the GNU General Public License, version 3 only
(`GPL-3.0-only`). See [LICENSE](LICENSE) for the full text. Third-party components keep
their own licences, as checked by `cargo xtask deny`. The AMD FidelityFX files in
`crates/upscale/ffx/` keep their own licence (`crates/upscale/ffx/LICENSE.txt`).

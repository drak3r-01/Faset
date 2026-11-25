# Developer guide

This guide describes how the workspace is organised, how to build and test Faset,
and the commands you run day to day. Read the project rules in `CLAUDE.md` and the
lot workflow in `docs/lots/README.md` before changing code.

## Ground rules

- **Everything runs in Docker.** Nothing is built, tested or installed on the host.
  The host runs only `git`, `gh` and the `dev.ps1` wrapper.
- Everything in the repository is in English: code, comments, commits and docs.
- Conventional Commits for commit messages.
- One lot = one branch (`lot/X.Y-slug`) = one pull request.
- Units: metre, second, radian. Right-handed, Y up, -Z forward.
- Data in RON, user settings in TOML.
- Errors: `thiserror` in libraries, `anyhow` in executables. Logs with `tracing`.
- The quality gate before merging is `cargo xtask ci`, run inside Docker.
- Mods are out of scope: there is no scripting or modding API.

## Docker

`docker-compose.yml` defines two services:

| Service | Image | Purpose |
|---|---|---|
| `dev` | `faset-dev` (target `dev`) | Compile, test, lint and package. Mounts the repository at `/workspace`. Named volumes keep the Cargo registry, the target directory, sccache and the Windows SDK cache between runs. |
| `server` | `faset-server` (target `server-runtime`) | The dedicated server only (see the [server guide](server-guide.md)). Publishes UDP 7777. |

The dev image pins the Rust toolchain of `rust-toolchain.toml` and includes the
cross-compilation tools (`cargo-xwin`, `cargo-deny`, `cargo-nextest`, `sccache`),
the Vulkan software driver (lavapipe) for headless rendering tests, and the
aarch64 Linux linker.

## dev.ps1 and dev.sh

`dev.ps1` (Windows) and `dev.sh` (POSIX) are thin wrappers: every argument is passed
to `cargo xtask` inside the `dev` service, except `run-server`.

```powershell
.\dev.ps1 ci                          # the full quality gate
.\dev.ps1 check                       # format and lint checks
.\dev.ps1 test                        # unit and integration tests
.\dev.ps1 run-server --port 7777      # build and run the server container
```

## xtask commands

`cargo xtask` (run through `.\dev.ps1 <command>`) is the only task runner.

| Command | What it does |
|---|---|
| `check` | `cargo fmt --check`, then clippy on all targets and features with warnings denied. |
| `test` | Runs the test suite. |
| `render-test` | Runs the rendering tests on the software Vulkan driver (lavapipe). |
| `build --target <t>` | Builds the placeholder or real binaries for one target. Targets: `x86_64-pc-windows-msvc`, `aarch64-pc-windows-msvc`, `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`. |
| `package [--target <t>]` | Packages the binaries of one target (or of all of them) into archives in `dist/`, with SHA-256 checksums. |
| `check-tag <tag>` | Fails unless the tag (`vX.Y.Z`) matches the workspace version. Used by the release pipeline. |
| `maps validate` | Validates the map files of the repository. |
| `balance [--format text\|csv]` | Prints the class and weapon balance report (time-to-kill figures). |
| `deny` | `cargo deny check`: licences, advisories, bans and sources (`deny.toml`). |
| `ci` | `check`, `test`, `render-test`, `maps validate` and `deny`, in that order. |

macOS targets are not built in Docker: the release pipeline builds them on macOS
runners.

## Release pipeline

`.github/workflows/release.yml` runs only on a pushed tag. It checks that the tag
matches the workspace version, runs `cargo xtask ci`, builds the six targets, packages
them, creates the GitHub release with checksums and generated notes, and publishes the
`server-runtime` image to the GitHub container registry. A failing step stops the rest.

To prepare a release, set the workspace version in `Cargo.toml` (it is inherited by
every crate), update the documentation, and merge. Tagging is the release decision.

## Crate map

Dependencies point downwards: the simulation knows neither rendering nor the concrete
network, and the dedicated server never links graphics crates (checked by a test).

| Crate (package name) | Path | Role |
|---|---|---|
| `faset-core` | `crates/core` | Shared base types, identifiers, time, maths, settings and key bindings, translations (`lang/`). |
| `faset-protocol` | `crates/protocol` | Wire format: bit writer and reader, quantisers, messages. |
| `faset-net` | `crates/net` | Datagram transports (UDP, in-memory), network conditions simulator, the four game channels. |
| `faset-sim` | `crates/sim` | Deterministic fixed-step simulation: ECS world, ordered systems, character controller. |
| `faset-minigame-api` | `crates/minigame-api` | Mini-game framework: registry, match phases, descriptors. |
| `faset-mode-fps` | `crates/mode-fps` | FPS mode: weapons, spread, recoil, damage, classes, pickups, scoring. |
| `faset-mode-race` | `crates/mode-race` | Witness mode: race between two objective zones. |
| `faset-bots` | `crates/bots` | Bot navigation graph, pathfinding, perception, aim and difficulty levels. |
| `faset-mapfmt` | `crates/mapfmt` | Versioned RON map format. |
| `faset-server` | `crates/server` | Authoritative game server: connections, command buffer, snapshots, rotation. |
| `faset-assets` | `crates/assets` | Mesh recipes (RON), palettes, validator, thumbnails. |
| `faset-shaders` | `crates/shaders` | WGSL shaders, compiled to SPIR-V by the build script. |
| `faset-render` | `crates/render` | Vulkan 1.2 renderer: device, swapchain, frame graph, lighting, sky, weather, water. |
| `faset-upscale` | `crates/upscale` | Upscaler trait, render scale, FSR 1 (and FSR 2/3, DLSS behind features). |
| `faset-audio` | `crates/audio` | Audio engine: voices, effects synthesised from RON recipes, 3D spatialisation. |
| `faset-ui` | `crates/ui` | egui interface and the frame graph pass that draws it. |
| `dedicated` | `crates/dedicated` | Headless server executable: `server.toml`, CLI, console, map catalogue, hosted FPS match with bots. |
| `faset-client` | `crates/client` | Game client (binary `faset`): app states, menus, hosting and joining, LAN discovery, prediction, interpolation, HUD. |
| `faset-editor` | `crates/editor` | In-game map editor (core). |
| `asset-tool` | `tools/asset-tool` | Validates and renders mesh recipes. |
| `map-tool` | `tools/map-tool` | Validates maps; writes JSON reports and view images. |
| `xtask` | `xtask` | Developer task runner (`cargo xtask`). |
| `probe` | `probe` | Placeholder binary used by the cross-target build checks. |

The `dedicated` crate and the `faset` client binary are the two shipped executables.

## Maps

Maps are RON files in the `mapfmt` format. The map design rules are in
[`docs/map-design-guide.md`](map-design-guide.md); the map designs are in
`docs/maps/`. `.\dev.ps1 maps validate` checks the repository's maps, and `map-tool`
validates a single map and writes a report.

## Settings and data files

- User settings: `settings.toml` in the user's Faset folder (see the player guide).
- Favourites and recent servers: RON files in the same folder.
- The dedicated server reads `server.toml` and the `maps/` folder next to it.

## Working on a lot

1. Read `docs/lots/README.md`, then the single lot file `docs/lots/<lot>-*.md`.
2. Create the branch `lot/<lot>-<slug>` from an up-to-date `main`.
3. Make the change inside the crates the lot owns; do not change the public interface
   of a crate that the lot does not own.
4. Run `.\dev.ps1 ci` before opening the pull request.
5. Open one pull request per lot and merge it when green.

# Lots overview

Summary table of the 58 lots of the plan, plus the internal lots (4.11, 6.6 to 6.13) and phase 8 "world" (8.1 to 8.12). Model and effort are taken from the header of each `docs/lots/<lot>-*.md` file. Status at the time of writing: merged 0.1-0.4, 1.1-1.4, 2.1-2.2; in progress 1.5 and 2.3; the others are to do.

| Lot | Title | Summary | Model / effort | Status |
|---|---|---|---|---|
| 0.1 | Read CLAUDE.md, Graphify, correct the plan | Check the project rules and correct sections 1 and 2 of the reference plan. | Opus, medium | Merged |
| 0.2 | Dev Docker image, host scripts, workspace skeleton | Make `cargo xtask ci` pass on an empty workspace, entirely in Docker. | Sonnet, medium | Merged |
| 0.3 | Cross-targets, lavapipe image, `build` and `package` | Build the placeholder binaries for the Docker targets and package them with checksums. | Sonnet, medium | Merged |
| 0.4 | Tag release pipeline, macOS build, `server-runtime` image | Produce a complete release from a version tag. | Sonnet, medium | Merged |
| 1.1 | Core and protocol: types, binary codec, quantisation | Provide the base types and a bit-exact wire format, checked by round-trip tests. | Sonnet, medium | Merged |
| 1.2 | Sim: ECS world, fixed tick, system order, base collisions | Make the simulation deterministic, replayable identically from a recorded input trace. | Opus, medium | To do |
| 1.3 | Net: transport, UDP, memory, network conditions simulator, channels | Provide the `Transport` abstraction and four reliable or unreliable channels, tested at 10 % loss. | Sonnet, high | Merged |
| 1.4 | Server: connection, command buffer, tick, full snapshots | Accept clients and send full snapshots so that two clients see each other. | Sonnet, medium | Merged |
| 1.5 | Delta snapshots, acknowledgements, reference frames | Encode snapshots as deltas against the last acknowledged one, with 64 reference frames. | Opus, medium | In progress |
| 1.6 | Clock sync, prediction and reconciliation on the client | Predict local movement, reconcile with the snapshots and smooth the corrections without teleporting. | Opus, high | To do |
| 1.7 | Remote interpolation and lag compensation | Display remote entities with a deliberate delay and rewind the hitboxes on the server (250 ms max). | Opus, medium | To do |
| 1.8 | Dedicated server: configuration, console, clean shutdown, Docker | Deliver the headless `dedicated` executable, configured by `server.toml`, in the `server-runtime` image. | Sonnet, medium | To do |
| 1.9 | Integrated server, LAN opening and discovery | Host a game inside the client, open it on the LAN and discover it by UDP broadcast. | Sonnet, medium | To do |
| 1.10 | Network endurance and robustness | Prove that the network stack withstands malformed input and long sessions without leaks. | Sonnet, medium | To do |
| 2.1 | Vulkan instance, swapchain, frame loop, offscreen target | Create the Vulkan 1.2 device, the windowed frame loop and an offscreen target read back as PNG. | Opus, medium | Merged |
| 2.2 | Meshes, instances, reverse-Z camera, jitter, motion vectors, culling | Draw instanced meshes with an infinite reverse-Z projection, jitter and frustum culling. | Opus, medium | Merged |
| 2.3 | WGSL shader chain, hot reload, procedural materials | Compile the WGSL shaders to SPIR-V and select material patterns by identifier. | Sonnet, medium | In progress |
| 2.4 | Stylised lighting, point lights, fog, ambience profiles | Implement stylised lighting, 32 lights, height fog and nine ambience profiles. | Opus, medium | To do |
| 2.5 | Cascaded shadows | Add sun shadows with three cascades, stable while the camera moves. | Sonnet, high | To do |
| 2.6 | Procedural sky: time, stars, clouds, space | Render a procedural sky driven by the time of day and the latitude, which also lights the scene. | Sonnet, high | To do |
| 2.7 | Weather: particles, shelter, humidity, storm, transitions | Render visual weather (rain, snow, sand, fog) with stateless GPU particles. | Sonnet, high | To do |
| 2.8 | Water surface and underwater medium | Render a water surface from Gerstner waves and an underwater medium with caustics. | Sonnet, medium | To do |
| 2.9 | Post-processing: contours, bloom, tonemapping, grading, FXAA | Implement the post-processing chain on the HDR buffer, driven by the ambience profile. | Sonnet, medium | To do |
| 2.10 | Temporal anti-aliasing, `Upscaler` trait, render scale, FSR 1 | Render at a reduced internal resolution and upscale cleanly, with TAA and FSR 1. | Sonnet, high | To do |
| 2.11 | FSR 2/3 through dynamic loading | Add FSR 2/3 as an option, loaded at run time, on Windows x86_64 only. | Opus, high | To do |
| 2.12 | DLSS through NVIDIA NGX | Add DLSS behind the `dlss` feature, active on RTX cards on Windows x86_64 only. | Opus, high | To do |
| 2.13 | Assets: mesh recipes, palettes, validator, thumbnails | Turn the RON prop recipes into meshes, with a validator and off-screen thumbnails. | Sonnet, medium | To do |
| 2.14 | Prop library by theme (about 150 recipes) | Write about 150 validated prop recipes across nine themes, without code changes. | Haiku, low | To do |
| 2.15 | Characters, weapons, procedural animation, viewmodel | Build characters and weapons from rigid blocks, animated by damped springs. | Sonnet, medium | To do |
| 2.16 | `egui` integration, theme, debug overlay | Draw the `egui` interfaces at display resolution, with a theme and a profiling overlay. | Sonnet, medium | To do |
| 2.17 | Audio engine: synthesised effects, 3D spatialisation | Create the `audio` crate with 32 prioritised voices, effects built from RON recipes and 3D spatialisation. | Sonnet, medium | To do |
| 3.1 | `mapfmt`: map format | Implement the versioned RON format, rewritten identically on the example, with a BLAKE3 fingerprint. | Opus, medium | To do |
| 3.2 | Map build: meshes, collisions, per-vertex occlusion | Turn a map into CPU meshes and a collision world, with baked ambient occlusion. | Sonnet, medium | To do |
| 3.3 | `map-tool`: validator, report, top and perspective views | Validate maps with precise rejection reasons, a JSON report and view images. | Sonnet, medium | To do |
| 4.1 | `minigame-api`, registry, match phases, witness race mode | Create the mini-game framework and a witness race mode, without changing the API. | Opus, medium | To do |
| 4.2 | Full character controller | Implement the Quake-style controller in `sim`, deterministic and shared by client and server. | Opus, high | To do |
| 4.3 | Slide, slide-hop, wall jump, trampolines, zones | Add the movement techniques and zone effects, checked by reference traces. | Opus, medium | To do |
| 4.4 | Hitscan weapons, spread, recoil, damage chain, respawn | Implement the hitscan firing pipeline, predicted by the client and validated by the server. | Sonnet, high | To do |
| 4.5 | Projectiles, explosions, melee | Add projectile weapons, explosions with knockback and melee to the firing pipeline. | Sonnet, medium | To do |
| 4.6 | Classes, loadouts, quirky weapons, balance tool | Define six classes and their weapons in RON data, with `xtask balance` (TTK). | Sonnet medium, then Haiku (data) | To do |
| 4.7 | Pickups, score, team and free-for-all modes | Implement pickups, score, timers and two modes on `minigame-api`, playable headless. | Sonnet, medium | To do |
| 4.8 | Mode HUD, camera and feel | Draw the FPS HUD with egui and implement the camera and the feel constants. | Sonnet, medium | To do |
| 4.9 | Bots: navigation graph and pathfinding | Build the navigation graph from a loaded map and provide A* with smoothing. | Sonnet, high | To do |
| 4.10 | Bots: decision, perception, aim, four levels | Make the bots play with utility decisions, delayed perception, ballistic aim and four levels. | Sonnet, high | To do |
| 5.1 | Map rotation, preloading, transfer by fingerprint | Change map every 5 minutes and download the missing maps by fingerprint. | Sonnet, medium | To do |
| 5.2 | Editor: camera, selection, handles, volumes, undo | Let the user build a map in the editor and undo everything step by step. | Sonnet, high | To do |
| 5.3 | Editor: library, gameplay, ambience, symmetry | Let the user create a complete map in the editor without editing the file by hand. | Sonnet, medium | To do |
| 5.4 | Editor: continuous validation, instant test, files | Validate live, test instantly, save, export and import maps. | Sonnet, medium | To do |
| 5.5 | Map trials by bots, statistics, design guide | Have 16 bots play a map without rendering, and produce a report and a design guide. | Sonnet, medium | To do |
| 5.6 | Catalogue of twelve original maps | Deliver twelve valid maps, tested by bots and checked by hand. | Opus, medium | To do |
| 6.1 | Application states, main menu, loading | Give the client a stacked state machine and all the main screens. | Sonnet, medium | To do |
| 6.2 | Host with bots and LAN, join, LAN list, favourites | Let the user host (with bots, optionally on the LAN) and join from the menus. | Sonnet, medium | To do |
| 6.3 | Classes and equipment, budget, preview, server validation | Build a loadout within a point budget, see it in 3D, and have the server validate it. | Sonnet, medium | To do |
| 6.4 | Settings: structure, metadata, persistence, display, quality | A single settings structure, a menu generated from metadata, applied without restarting. | Sonnet, medium | To do |
| 6.5 | Settings: controls, audio, network, accessibility, translation | Finish the settings, detect key conflicts, and provide a complete FR/EN interface. | Sonnet medium, then Haiku (texts) | To do |
| 6.9 | Shooting feel (internal lot) | Muzzle flash, tracers, recoil, impacts per material, markers, sounds, FPS weapon without clipping. | Sonnet, high | To do |
| 6.10 | Terrain with relief (internal lot) | Smooth heightmap, slope blending, slope collisions, bot navigation, sculpting in the editor. | Opus, high | To do |
| 6.11 | Props and vegetation (internal lot) | Prop rendering, generated low-poly vegetation, GPU instancing, wind, LOD. | Opus, high | To do |
| 6.12 | Landscape beyond bounds (internal lot) | Terrain and scenery beyond the playable area, natural borders per map, fog and sky. | Sonnet, high | To do |
| 6.13 | Level design (internal lot) | Three non-square maps designed on paper, then tested with the bots. | Opus, high | To do |
| 8.1 | Large terrain: format v5, chunk streaming, clipmap LOD | Smooth 1 to 2 km terrain, format v5, streamed chunks, LOD, material blending. | Opus, high | To do |
| 8.2 | Far landscape: LOD for vegetation and props, landmarks, atmosphere | Fill the horizon: vegetation and prop LOD, distant landmarks, atmospheric haze. | Opus, high | To do |
| 8.3 | World generator and editor sculpting tools | Terrain generator (erosion, rivers) and sculpting and vegetation brush tools in the editor. | Sonnet, high | To do |
| 8.4 | Natural boundaries and playable zone | A free-form playable zone closed by the scenery (cliff, water, forest), without invisible walls. | Sonnet, high | To do |
| 8.5 | Organic modelling primitives for prop recipes | New modelling operators (lathe, extrusion, bevel, noise) for realistic props. | Opus, high | To do |
| 8.6 | Prop library, outdoor themes | About 100 outdoor props (rocks, fences, boats, benches...) with variants. | Sonnet, medium | To do |
| 8.7 | Prop library, built themes | About 100 urban, industrial and interior props (cars, stalls, containers...) with variants. | Sonnet, medium | To do |
| 8.8 | Vegetation v2 and lighting polish | Richer vegetation, grass following the ground, ambient occlusion, grading per theme. | Sonnet, high | To do |
| 8.9 | Rebuild Pine Hollow and Mossgate | Rebuild Pine Hollow and Mossgate as real landscapes, tested by bots. | Opus, high | To do |
| 8.10 | Rebuild Driftwood Cove and Rust Harbor | Rebuild Driftwood Cove and Rust Harbor as real landscapes, tested by bots. | Opus, high | To do |
| 8.11 | Rebuild Red Mesa, Greyfold Quarry and Nightjar Market | Rebuild Red Mesa, Greyfold Quarry and Nightjar Market, tested by bots. | Opus, high | To do |
| 8.12 | World performance and validation | Measure and hold the world-phase budgets on the seven maps. | Sonnet, medium | To do |
| 7.1 | Profiling and optimisation of rendering and network | Measure and reach each budget of section 20 without changing the public interfaces. | Sonnet, medium | To do |
| 7.2 | Platform validation and packaging | Check that each target builds and produce an archive with checksums per target. | Sonnet, medium | To do |
| 7.3 | English documentation, release notes, tag v0.1.0 | Finalise the documentation and publish `v0.1.0` through the release pipeline. | Haiku, low | To do |

## Milestones

J0 after 0.4 · J1 after 1.8 · J2 after 2.10 · J3 after 4.2 · J4 after 4.10 · J5 after 5.6 · J6 after 6.5 · J7 after 7.3 (`v0.1.0`).

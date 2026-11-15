# Faset development plan — sections 1 and 2

This file holds sections 1 and 2 of the reference development plan, translated into English and corrected by lot 0.1. Sections 3 to 22 stay in the reference plan; each lot file in `docs/lots/` carries the parts it needs.

## 1. Scope and validated decisions

The reference plan is the source of truth for development. It fixes the architecture, the algorithms and the split into lots.

### Decisions validated by Gabriel on 2026-11-15

| Topic | Decision |
| --- | --- |
| Plan and split into lots | Approved |
| Upscaling | FSR 1 rewritten as WGSL shaders is the universal base. FSR 2/3 and DLSS are integrated through FFI, the only exceptions to 100 % Rust |
| DLSS target | Windows x86_64 only |
| Shader language | WGSL, compiled to SPIR-V by `naga` |
| Host PC | Windows x86_64. First build target: `x86_64-pc-windows-msvc` |
| macOS | Through MoltenVK, built by GitHub CI |
| GitHub CI | One pipeline, triggered only by pushing a tag: test, then build, then release |
| Player capacity | 16 players per match, bots included, with technical headroom up to 32 |
| First modes | Free-for-all and team deathmatch |

### Former hypotheses and their status

| Hypothesis in the original plan | Status | Resolution |
| --- | --- | --- |
| `CLAUDE.md` had not been read yet | Resolved (lot 0.1) | `CLAUDE.md` exists at the repository root and is authoritative. It points to `docs/lots/` for lot work and states the coding rules repeated below. |
| The Graphify rules of section 2 are a working hypothesis (Graphify assumed to be an MCP server) | Resolved, corrected (lot 0.1) | Graphify is used through the `graphify` MCP server, not a CLI. Its tools are `graph_stats`, `god_nodes`, `query_graph`, `get_node`, `get_neighbors`, `get_community`, `shortest_path`, `get_pr_impact`, `list_prs` and `triage_prs`. Section 2 is rewritten accordingly (corrected again after lot 0.1, which wrongly described a CLI). |
| Capacity: 16 players including bots, headroom to 32 | Confirmed | Kept unchanged (decision table above). |
| First modes: free-for-all and team deathmatch | Confirmed | Kept unchanged (decision table above). |
| Crate prefix stays neutral (`core`, `sim`, ...) until the game is named | Resolved, decision (lot 0.1) | The game is now named Faset. Crates keep their neutral short names in the plan, the lot files and the directory layout (`crates/core`, `crates/sim`, ...). Their Cargo package names take the `faset-` prefix (`faset-core`, `faset-sim`, ...), because a package named `core` would shadow Rust's built-in `core` crate. `xtask` keeps its conventional unprefixed name. |

### General conventions

- Everything that enters the repository is in English: code, comments, commits, documentation, map and prop names.
- Units: metre, second, radian. Right-handed frame, Y up, -Z forward.
- Data in RON, user settings in TOML.
- `unsafe` is forbidden outside the `render` and `upscale` crates (`#![forbid(unsafe_code)]` everywhere else).
- Errors: `thiserror` in libraries, `anyhow` in executables. Logging with `tracing`.
- Commits follow Conventional Commits.

## 2. Session workflow and Graphify integration

Each lot is executed by a single session that always follows the same seven steps. Graphify is used through the `graphify` MCP server, which holds a knowledge graph of the code (stored in `graphify-out/`) and answers scoped questions about it. It is used only once the repository contains Rust code; the graph currently in `graphify-out/` comes from another project and must be rebuilt.

### Graphify MCP tools

| Tool | Use |
| --- | --- |
| `query_graph` | First step for any codebase question, before any grep, search or source-file read, whenever `graphify-out/graph.json` exists. |
| `get_node`, `get_neighbors` | Focused view of one concept and its direct relationships. |
| `shortest_path` | Relationship between two concepts, types or crates. |
| `get_community`, `god_nodes`, `graph_stats` | Broad navigation and structure of the graph. |
| `get_pr_impact`, `list_prs`, `triage_prs` | Impact and triage of pull requests. |

The MCP exposes no rebuild tool. Refresh the graph after code changes only if the MCP allows it; otherwise say so in the lot report.

Supporting rules from `CLAUDE.md`:

- If `graphify-out/wiki/index.md` exists, use it for broad navigation instead of browsing raw sources.
- Read `graphify-out/GRAPH_REPORT.md` only for a broad architecture review, or when `query_graph`, `shortest_path` and `get_neighbors` are not enough.
- Every subagent prompt that explores code repeats the "query Graphify (MCP) first" rule.
- A committed `graphify-out/graph.json` is rewritten whole on every build, so a large diff to it in a pull request is expected and is not flagged as churn.

### The seven session steps

1. Read `CLAUDE.md` and the lot file (`docs/lots/<lot>-*.md`), which is self-contained.
2. Query Graphify through its MCP tools (`query_graph`, `get_neighbors`, `shortest_path`) about the crates, types and functions the lot file names, instead of browsing files. This is the main token saving.
3. Read only the raw lines that the graph or an earlier read has already located.
4. Create the branch `lot/<id>-<slug>` and write the code. Every command runs in Docker, never on the host.
5. Run `cargo xtask ci` in Docker. GitHub CI only runs on tags, so this local command is the only quality gate before merge.
6. Refresh the graphify graph if the MCP allows it, then make the final commit (Conventional Commits, in English).
7. Write a short report: delivered, tests passed, deviations from this document.

### Lot file

Each session receives a lot file of less than one page, taken from the reference plan:

- goal in one sentence and reference section;
- crates touched and crates forbidden to modify;
- public interfaces to respect or create;
- acceptance criteria that can be checked by a command;
- explicit out-of-scope list.

Items the plan leaves unspecified are marked as decisions in the lot file. The session picks a value, records it and mentions it in its report.

### Merge rules

- One lot, one branch, one pull request.
- A lot never changes the public interface of a crate it does not own. If it must, it stops and reports.
- Every deviation from the plan is recorded before merge, in this file for sections 1 and 2 and in the lot report otherwise, so the plan stays the reference.

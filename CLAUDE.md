## graphify

This project has a knowledge graph at graphify-out/ with god nodes, community structure, and cross-file relationships.

Rules:
- When graphify-out/graph.json exists, use the `query_graph` tool of the `graphify` MCP server before your first grep, search or source-file read for a codebase question, and put this rule in every subagent prompt that explores code. Read raw files only for lines the graph or an earlier read already located. Use `shortest_path` for relationships and `get_node` / `get_neighbors` for focused concepts. Graphify is used only through the MCP server (tools: graph_stats, god_nodes, query_graph, get_node, get_neighbors, get_community, shortest_path, get_pr_impact, list_prs, triage_prs), never as a CLI. These return a scoped subgraph, usually much smaller than GRAPH_REPORT.md or raw grep output.
- If graphify-out/wiki/index.md exists, use it for broad navigation instead of raw source browsing.
- Read graphify-out/GRAPH_REPORT.md only for broad architecture review or when the MCP query tools do not surface enough context.
- After modifying code, refresh the graph if the MCP exposes a way to do it; otherwise say in the report that the graph is stale.
- A committed graphify-out/graph.json is rewritten whole on every build, so a large diff to it in a pull request is expected. Do not flag it as churn or suggest skipping the graph rebuild or its hooks.

## Lot files and project rules

- Development is split into 58 lots. For any lot, read `docs/lots/README.md` (index) and the single file `docs/lots/<lot>-*.md` FIRST and with priority: it is self-contained (goal, crates, spec, interfaces, acceptance criteria, out of scope). Do not load the full reference plan.
- One lot = one branch (`lot/X.Y-slug`) = one pull request. Never change the public interface of a crate the lot does not own: stop and report.
- Items marked "decision" in a lot file are unspecified by the plan: pick a value, record it, and mention it in the report.
- All builds, tests and tooling run in Docker only, never on the host. Run `cargo xtask ci` in Docker before merging (GitHub CI only runs on tags).
- Everything in the repository is in English (code, comments, commits, docs). Conventional Commits.
- Units: metre, second, radian; right-handed, Y up, -Z forward. Data in RON, user settings in TOML.
- Errors: `thiserror` in libraries, `anyhow` in executables; logging with `tracing`.
- Do not use graphify until the repository contains Rust code; the graph currently in `graphify-out/` is from another project and must be rebuilt.

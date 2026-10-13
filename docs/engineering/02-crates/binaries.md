# Crates: binaries

The three crates that produce executables: `agentops-server`, `agentops-mcp-server` and `agentops`. Read this before changing how processes are wired or configured; process-level architecture is in [01-architecture.md](../01-architecture.md), "Binaries".

Index and field definitions: [02-crate-reference.md](../02-crate-reference.md).

## agentops-server

- **Path**: `agentops-core/crates/agentops-server`
- **Binary**: `agentops-server` (`src/main.rs`).
- **Purpose**: The merged REST server. It composes `agentops-api`, `docbrain-api` and `agentops-heavy-api` into one Axum process on one port, following the "one open-core, no price gate" model (module doc).
- **Entry points**:
  - `run()`: reads env, builds the routers, upserts the `agentops` docbrain library, binds `AGENTOPS_ADDR` (default `127.0.0.1:8420`). Also called by `agentops serve-api`.
  - `connect_sh` module: `GET /connect.sh`, a deliberately unauthenticated script. It installs the CLI if missing and runs `agentops connect --remote <this server> --agents ... --yes --device-login`. No API key is ever embedded in or read by it (module doc).
- **Depends on**: `api`, `heavy-api`, `manifest`, `mcp`, `docbrain-api`, `docbrain-graph`, `docbrain-mcp`.
- **Used by**: `cli`.
- **Invariants**:
  - Route collisions are resolved by construction, never by "last one wins" (module doc). `/health` comes from heavy-api only and `/tools` from `agentops-api`. docbrain is nested under `/docbrain`, and dashboard routes come from heavy-api.
  - `build_router_without_dashboard_routes` runs in `spawn_blocking`, because it may connect to Postgres, whose store must not start inside a running runtime (comment in `run`).
- **Known issues**: a private copy of `agentops_data_dir` (`findings.md` F16); the self-seeded docbrain library is invisible to hosted orgs (F3).
- **Tests**: 7 (`src/connect_sh.rs`).

## agentops-mcp-server

- **Path**: `agentops-core/crates/agentops-mcp-server`
- **Binary**: `agentops-mcp-server` (`src/main.rs`). This is what `agentops connect` registers.
- **Purpose**: The unified stdio MCP server. It merges `agentops-mcp`, `docbrain-mcp` and, when Qdrant is configured, `agentops-heavy-mcp`, so a client needs one server entry instead of three.
- **Entry points**: `Dispatch` (`list_tools`, `call_tool`), `run_stdio`.
- **Configuration**: `AGENTOPS_ACCESS_MODE` (`advisor` default, `full`), `AGENTOPS_DOCBRAIN_DB`/`AGENTOPS_DATA_DIR`, `AGENTOPS_QDRANT_URL` and `AGENTOPS_QDRANT_COLLECTION` (optional; loading BGE-M3 can take time on first run).
- **Depends on**: `heavy-embeddings`, `heavy-mcp`, `mcp`, `docbrain-graph`, `docbrain-mcp`.
- **Used by**: none.
- **Invariants** (module doc):
  - Only `agentops-mcp`'s tools respect `AccessMode`. Adding a capability system to the other two crates was "out of scope for this merge, whose goal is removing tiering".
  - Tool names are unique across the three tables; collisions were renamed before the merge.
  - Dispatch checks each table's own name list in order rather than parsing "unknown tool" errors.
  - Without `AGENTOPS_QDRANT_URL`, heavy tools are absent; the process does not fail to start.
- **Tests**: 4 (`src/lib.rs`).

## agentops-cli

- **Path**: `agentops-core/crates/agentops-cli`
- **Binary**: `agentops` (`src/main.rs`).
- **Purpose**: The single entry-point CLI. "Every subcommand is a thin wrapper: parse args, call an already-built library function, print the result" (module doc of `src/main.rs`).
- **Subcommands**: listed by purpose in `01-architecture.md`. Larger ones have their own module:

  | Module | Command | Job |
  | --- | --- | --- |
  | `hook_capture.rs` | `hook-capture --event <name>` | The handler behind every Claude Code hook `connect --with-hooks` installs. |
  | `compress_output.rs` | `compress-output --kind <kind>` | Compresses a command's output, as one stage of a `PreToolUse`-rewritten pipeline. |
  | `usage.rs` | `usage sync` | Parses a Claude Code JSONL transcript and syncs per-session token usage. |
  | `usage_report.rs` | `usage report` | Replays where a session's tokens went. |
  | `librarian.rs` | `librarian` | Dry-run evaluation of cheap-model gotcha cataloguing; local only, never writes back to the server. |

- **Notable functions in `src/main.rs`**: `install`, `connect`, `connect_global`, `device_flow_login`, `migrate_graph`, and the per-tool MCP config writers (Claude Code JSON, Codex TOML and others).
- **Depends on**: 17 crates, including `server`.
- **Used by**: none.
- **Invariants**:
  - No business logic in the CLI; it reuses the same use cases as the MCP tools (module doc).
  - `connect` writes MCP entries that run `agentops-mcp-server` with `AGENTOPS_ACCESS_MODE` in `env` (tests at `src/main.rs`, e.g. `parsed["mcpServers"]["agentops"]["command"] == "agentops-mcp-server"`).
  - `serve-api` sets env vars from its flags before the Tokio runtime starts, then calls `agentops_server::run`. On the current toolchain `std::env::set_var` needs `unsafe` (gotcha 53998).
- **Known issues**: `serve` and `docbrain-serve` still run the pre-merge split servers (`findings.md` F17).
- **Known history**: gotchas 54240 (hooks written in the wrong schema never fired), 53972 (`PostToolUse` `tool_response` shape), 53975 (`connect --remote` failed with an SSH host alias), 54236 (JSONL repeats usage per content block).
- **Tests**: 65 (`src/hook_capture.rs` 27, `src/main.rs` 12, `src/compress_output.rs` 7, `src/usage.rs` 7, `src/librarian.rs` 6, `src/usage_report.rs` 6). Some need `AGENTOPS_TEST_DATABASE_URL`.

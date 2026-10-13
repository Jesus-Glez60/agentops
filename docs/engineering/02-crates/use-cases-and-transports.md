# Crates: use cases and transports

The core tool table and its use cases (`agentops-mcp`), the single-operator REST adapter (`agentops-api`), and the multi-tenant REST and `/mcp` adapter (`agentops-heavy-api`). Read this before adding a tool or a route; contracts are in [07-mcp-and-rest-contracts.md](../07-mcp-and-rest-contracts.md) and `docs/openapi.yaml`.

Index and field definitions: [02-crate-reference.md](../02-crate-reference.md).

## agentops-mcp

- **Path**: `agentops-core/crates/agentops-mcp`
- **Purpose**: The 22-tool core table and the use-case functions behind it, plus a stdio MCP server. Handlers stay thin; the use cases are shared with the CLI.
- **Modules**:

  | Module | Job |
  | --- | --- |
  | `tools.rs` | `tool_specs()`: one `ToolSpec` row per tool (name, description, `access`, annotations, input schema, handler). `list_tools(mode)`, `call_tool(mode, name, args)`, `AccessMode`. |
  | `scan.rs` | `scan_and_persist`, `scan_and_persist_with_lsp`, `persist`, `enrich_with_macro_expansions`: the one scan-and-persist path, used by `scan_repo` and `agentops install`. |
  | `notes.rs` | `add_note`, `ingest_notes_dir`: shared by the MCP tools and `agentops note`/`ingest-notes`. |
  | `init.rs` | `init_agents_md`: bootstraps `AGENTS.md` and `NOTES_PATH`; shared with `agentops install`. |
  | `docgen.rs` | `generate_docs`: renders `repo-map.md`; shared with `agentops docgen`. |
  | `budget.rs` | Output caps: `DEFAULT_CHAR_BUDGET = 4000`, `GUIDE_CHAR_BUDGET = 2000`, `cap`, `cap_sections`, `compress_json`, `detect_cache_unsafe`. |
  | `session_guide.rs` | `get_session_guide`'s prioritized, size-budgeted session summary; `build_pinned_context`, `build_repo_briefing`. |
  | `consolidate.rs` | The only place in this crate that touches `candle`: runs embedding consolidation (from `end_session`) and loads the active projector for search. |
  | `watch.rs` | File-watcher auto-rescan behind `agentops watch`. |
  | `sync_docs.rs` | Automatic library discovery (registry, then GitHub) behind `agentops sync-docs`. |
  | `store.rs` | Re-exports `agentops-store-open`. |
  | `server.rs`, `protocol.rs` | Stdio JSON-RPC loop (`run_stdio`) and hand-rolled MCP wire types. |

- **Depends on**: `agents-md`, `docgen`, `embeddings`, `embeddings-train`, `graph`, `graph-pg`, `llm`, `lsp-client`, `notes`, `retrieval`, `scanner`, `store-open`, `docbrain-graph`, `docbrain-ingest`.
- **Used by**: `api`, `cli`, `heavy-api`, `mcp-server`, `server`.
- **Invariants**:
  - One table drives listing, dispatch and access gating, replacing three hand-synced lists (`src/tools.rs` module doc).
  - Full-only tools are hidden from Advisor's listing **and** refused by `call_tool` (doc comment in `call_tool`).
  - Every tool takes a repo `path` and opens its store per call through `open_store`, unless a shared Postgres store is scoped.
  - The automatic every-scan note re-match must not bump `confirmed_at`/`updated_at` (gotcha 53965).
  - Large outputs are capped and pageable through `fetch_content` (commit `31c1723`).
  - Wire types are hand-rolled instead of using a pre-1.0 MCP SDK (`src/protocol.rs` module doc).
- **Known history**: gotchas 54077 (`scan_repo` only rescans an existing working copy), 54078 (batch upsert must dedupe), 53966 (rescans never embedded notes), 54117 (`detect_cache_unsafe` tokenizer and timestamps).
- **Tests**: 127 (`src/tools.rs` 41, `src/scan.rs` 27, `src/budget.rs` 18, `src/docgen.rs` 12, `src/session_guide.rs` 8, `src/sync_docs.rs` 6, `src/notes.rs` 5, `src/init.rs` 4, `src/server.rs` 3, `src/consolidate.rs` 2, `src/watch.rs` 1). 5 are `#[ignore]`; some need `AGENTOPS_TEST_DATABASE_URL`.

## agentops-api

- **Path**: `agentops-core/crates/agentops-api`
- **Purpose**: Single-operator REST adapter. It wraps `agentops_mcp::call_tool`, and adds dashboard endpoints backed by the local scan registry (`agentops-manifest`).
- **Routes** (17): `/health`, `/tools`, `/tools/{name}`, `/nodes`, `/scans`, `/activity`, `/gotchas`, `/local-search`, and `/repos/{name}/...` (`rescan`, `docs`, `graph`, `hotspots`, `usage`, `nodes/{id}`, `nodes/{id}/curation`, `nodes/{id}/graph`, `edges/{id}/reinforce`).
- **Modules**: `repos.rs` (scans, rescan, activity), `search.rs` (local search, node detail), `subgraph.rs` (multi-hop subgraph, whole-repo graph), `docs.rs` (Documentation Viewer data), `usage.rs` (usage aggregation).
- **Entry points**: `build_router(mode, api_key_hash, manifest_path)`, `build_router_without_health`, `build_router_without_dashboard_routes`, `run`, `parse_kind`, `GOTCHA_BUCKETS`.
- **Depends on**: `docgen`, `embeddings`, `graph`, `manifest`, `mcp`, `scanner`, `security`.
- **Used by**: `heavy-api`, `server`.
- **Invariants** (module doc):
  - `AppState` carries an optional shared `PostgresGraphStore`; handlers resolve stores through `resolve_store`, never `open_store` directly.
  - Every `GraphStore` call from a handler runs in `spawn_blocking` (doc comment on `call_tool_handler`).
  - In `agentops-server`, the dashboard routes are excluded (`build_router_without_dashboard_routes`) because `agentops-heavy-api` serves tenant-scoped versions at the same paths (gotcha 53986).
- **Decision**: "Whole-repo graph view (/repos/{name}/graph) is deliberately uncapped -- NODE_CAP only applies to the seed-centered BFS".
- **Tests**: 79 (`src/subgraph.rs` 28, `src/repos.rs` 17, `src/search.rs` 15, `src/usage.rs` 9, `src/lib.rs` 7, `src/docs.rs` 3). 3 are `#[ignore]`.

## agentops-heavy-api

- **Path**: `agentops-core/crates/agentops-heavy-api`
- **Purpose**: The multi-tenant REST and MCP adapter. It serves repo connections and indexing, accounts, integrations and teams. It also serves tenant-scoped dashboard and library routes, Linear modules, webhooks and the hosted `/mcp` endpoint.
- **Modules and routes**:

  | Module | Routes | Job |
  | --- | --- | --- |
  | `lib.rs` | 38: `/health`, `/repos/*`, `/docs/search*`, `/consolidate/model`, dashboard, libraries, `/mcp` | App state, router assembly, repo connection lifecycle, auth middleware. |
  | `accounts_integrations.rs` | 28: `/auth/*` (signup, login, 2FA, sessions, API keys, CLI device flow), `/bootstrap/config`, `/integrations/*` (incl. OAuth start/callback) | Accounts and the credentials vault. |
  | `team.rs` | 18: `/team/*`, `/invites/*`, `/me/memberships`, `/me/switch-tenant` | Members, roles, invites, audit log, MCP access mode, ownership transfer, org deletion. |
  | `linear_webhook.rs` | 5: `/webhooks/linear`, `/linear/*` | Linear auto-kickoff and task push/summarize. |
  | `github_app_routes.rs` | `/webhooks/github` | GitHub App install flow and webhook receiver, including push-triggered reindex. |
  | `contact_routes.rs` | `/contact-leads` | Public lead capture for the marketing site. |
  | `mcp_http.rs` | (`/mcp` handler) | MCP Streamable HTTP, stateless: one JSON response per POST, no SSE. |
  | `indexing.rs` | (jobs) | Clone, scan, embed, docgen pipeline with polled progress. |
  | `dashboard.rs`, `libraries_http.rs` | (handlers) | Tenant-scoped versions of `agentops-api`'s and `docbrain-api`'s routes. |
  | `tenant_repo.rs` | (helpers) | `require_tenant_auth`, `resolve_connection_path`, `register_repo`, `unregister_repo`. |

- **Entry points**: `build_full_router(db_path, include_tools)`, `build_router`, `build_team_router`, `build_accounts_integrations_router`, `merge_github_webhook_route`, `merge_linear_webhook_route`, `run` (unused, `findings.md` F8).
- **Depends on**: 21 crates, more than any other crate.
- **Used by**: `server`.
- **Invariants**:
  - The encrypted private key blob is never returned: response DTOs are built by hand, never by serializing `RepoConnection` (module doc).
  - Two auth layers. `require_api_key_or_session` covers most routes. `/mcp` gets its own `tenant_repo::require_tenant_auth`, which accepts a session or a personal key only, never the instance-wide key (`src/lib.rs` comments, `src/tenant_repo.rs`).
  - Tenant-scoped docbrain content goes to one SQLite file per tenant (`docbrain_db_path_for_org`). Qdrant does not isolate tenants (module doc).
  - Routes are mounted at bare paths, not under a prefix (gotcha 53955).
  - Every `agentops_mcp::call_tool` runs in `spawn_blocking` inside `with_shared_postgres_store` (`src/mcp_http.rs`).
- **Known history**: gotchas 54215/54150 (webhook auto-reindex after upgrading a Discovered connection), 54214 (duplicate connections), 54243 (stub merge leaves `agentops-remote.json` stale), 54007 (embeddings stage hardcoded SQLite), 53954 (logs via `println!`, no `tracing`), 54153 (`/mcp` requires real auth).
- **Tests**: 177 (`src/lib.rs` 58, `src/accounts_integrations.rs` 39, `src/team.rs` 36, `src/linear_webhook.rs` 17, `src/github_app_routes.rs` 12, `src/contact_routes.rs` 5, `src/dashboard.rs` 5, `src/tenant_repo.rs` 3, `src/indexing.rs` 2). 6 are `#[ignore]`; some need Postgres or Qdrant.

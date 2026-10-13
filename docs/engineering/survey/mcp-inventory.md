# Survey: MCP tool inventory (Phase 1)

Every MCP tool AgentOps defines, which surface exposes it, and under which access mode. Input for `07-mcp-and-rest-contracts.md`; counted from source, not from a live server.

Source: `docs/engineering/scripts/mcp-tools.sh` plus reading each dispatch site. Measured 2026-10-12 on `main` @ `8565155`.

> Corrections found in Phase 4 (2026-10-13); `07-mcp-and-rest-contracts.md` has the corrected tables:
> - `add_note` has no `items` input; that was the JSON Schema keyword inside `tags`. The handler also reads an undeclared `notes_path`.
> - `discover_library` writes: it registers the library it finds (`add_library`).

## Tool tables

Four places define tools:

| Table | File | Tools | Has access mode | Has MCP annotations |
| --- | --- | --- | --- | --- |
| `agentops-mcp` | `agentops-core/crates/agentops-mcp/src/tools.rs` (`tool_specs`) | 22 | Yes, `ToolSpec.access` | Yes |
| `docbrain-mcp` | `docbrain-core/crates/docbrain-mcp/src/tools.rs` | 12 | No | No |
| `agentops-heavy-mcp` | `agentops-core/crates/agentops-heavy-mcp/src/tools.rs` | 5 | No | No |
| hosted-only | `agentops-core/crates/agentops-heavy-api/src/mcp_http.rs` | 2 | No | Yes |

Total distinct tool names: 41.

## Access-mode enforcement

- `AccessMode` has two values, `Advisor` and `Full` (`agentops-core/crates/agentops-mcp/src/tools.rs`).
- `list_tools(mode)` hides `Full`-only tools under `Advisor`. `call_tool` also refuses them, as a "defensive backstop" (same file, `call_tool`). Hidden **and** rejected, not one or the other.
- Only the `agentops-mcp` table is gated. Docbrain, heavy and hosted-only tools are exposed in both modes on every surface.
- The mode comes from different places per surface:
  - stdio `agentops-mcp-server`: env `AGENTOPS_ACCESS_MODE`, default `advisor` (`agentops-core/crates/agentops-mcp-server/src/main.rs`).
  - REST `/tools` on `agentops-server`: same env var, read once per process (`agentops-core/crates/agentops-server/src/lib.rs`).
  - hosted `/mcp`: per-org column `organizations.mcp_access_mode`, default `'advisor'` (`agentops-core/crates/agentops-teams/src/lib.rs:127`), read per call by `resolve_access_mode` (`agentops-core/crates/agentops-heavy-api/src/mcp_http.rs`).

## Surface counts

| Surface | Composition | Advisor | Full |
| --- | --- | --- | --- |
| stdio `agentops-mcp-server` | `agentops-mcp` + `docbrain-mcp` (+ `agentops-heavy-mcp` when `AGENTOPS_QDRANT_URL` is set) | 26 (31) | 34 (39) |
| hosted `POST /mcp` | `agentops-mcp` + `docbrain-mcp` + `register_repo`, `unregister_repo` | 28 | 36 |
| REST `GET /tools` (`agentops-server`) | `agentops-mcp` only | 14 | 22 |
| REST `GET /docbrain/tools` (`agentops-server`) | `docbrain-mcp` only | 12 | 12 |
| stdio `agentops serve` (CLI) | `agentops-mcp` only (`agentops_mcp::run_stdio`) | 14 | 22 |
| stdio `agentops docbrain-serve` (CLI) | `docbrain-mcp` only (`docbrain_mcp::run_stdio`) | 12 | 12 |

Notes:

- Hosted `/mcp` deliberately omits heavy tools: "needs its own tenant-safe `SemanticIndex` threading, out of scope for this fix" (`agentops-core/crates/agentops-heavy-api/src/mcp_http.rs`, module doc).
- `agentops-heavy-api` also defines `/tools` with heavy tools, but `agentops-server` builds it with `build_full_router(&heavy_db_path, false)`, so that route is not mounted.
- `agentops connect` registers the unified `agentops-mcp-server` binary, not `agentops serve` (`agentops-core/crates/agentops-cli/src/main.rs`, MCP entry with `"command": "agentops-mcp-server"`). The two CLI stdio servers are pre-merge leftovers; see `findings.md` F17.
- Tool names were de-duplicated before the merge (`agentops-mcp-server` module doc): `semantic_search` → `local_semantic_search` (core), `get_changelog` → `list_scans` (core), heavy `search_docs`/`index_docs` → `search_docs_indexed`/`index_docs_indexed`.
- The README's "about 40 tools" matches no single surface. See `findings.md` F1.

## agentops-mcp tools (22)

R/W comes from the `read_only_hint` annotation. Inputs marked `*` are required.

| Tool | Access | R/W | Inputs |
| --- | --- | --- | --- |
| `status` | Advisor | R | `path*` |
| `list_gotchas` | Advisor | R | `path*`, `session_id` |
| `get_symbol` | Advisor | R | `path*`, `name*`, `file`, `session_id`, `max_chars` |
| `list_scans` | Advisor | R | `path*`, `limit` |
| `related_context` | Advisor | R | `path*`, `symbol_id*`, `top_k`, `session_id`, `max_chars`, `compact` |
| `fetch_content` | Advisor | R | `path*`, `id*`, `offset`, `limit` |
| `get_session_guide` | Advisor | R | `path*`, `session_id*` |
| `get_session` | Advisor | R | `path*`, `session_id*` |
| `list_tasks` | Advisor | R | `path*` |
| `get_task_activity` | Advisor | R | `path*`, `task_id*` |
| `local_semantic_search` | Advisor | R | `path*`, `query*`, `top_k`, `kind` (enum lacks `doc_section`, `tool_output`), `mode`, `graph_expand` |
| `add_note` | Advisor | **W** | `path*`, `title*`, `body*`, `note_type`, `tags`, `items`, `with_embeddings`, `session_id` |
| `ingest_notes` | Advisor | **W** | `path*`, `notes_path`, `with_embeddings`, `session_id` |
| `persist_blueprint_doc` | Advisor | **W** | `path*`, `doc_id*`, `title*`, `markdown*`, `note_title*`, `note_type`, `session_id` |
| `scan_repo` | Full | W | `path*`, `with_embeddings`, `session_id` |
| `explain_symbol` | Full | W, open-world (LLM call) | `path*`, `symbol_id*`, `session_id`, `max_chars` |
| `end_session` | Full | W | `path*`, `session_id*` |
| `report_usage` | Full | W | `path*`, `session_id*`, `agent*`, `calls`, `output_tokens`, `total_context_tokens`, `peak_context`, `categories` |
| `create_task` | Full | W | `path*`, `title*`, `description`, `priority`, `assignee`, `session_id` |
| `update_task_status` | Full | W | `path*`, `task_id*`, `status*` |
| `init_agents_md` | Full | W | `path*`, `notes_path` |
| `generate_docs` | Full | W | `path*` |

Three write tools are `Advisor` on purpose. The comment on `add_note` says advisor mode "exists to block destructive/costly actions"; recording a gotcha or decision is neither (`agentops-core/crates/agentops-mcp/src/tools.rs:96-102`). `ingest_notes` and `persist_blueprint_doc` have no comment of their own; see `questions.md` Q6.

On hosted `/mcp`, `path` is a connection id or URL, rewritten to a server path by `resolve_connection_path` before dispatch. The hosted store's repo name is `<connection_id>--<tenant>` (gotcha 54242, "Hosted store repo name is `<connection_id>--<tenant>`, not the connection id -- routes must accept both").

## docbrain-mcp tools (12)

No annotations exist, so R/W is **inferred from the description and inputs**; Phase 3 verifies it against each handler.

| Tool | R/W (inferred) | Inputs |
| --- | --- | --- |
| `list_libraries` | R | — |
| `get_library` | R | `slug*` |
| `discover_library` | R, open-world (registry lookup) | `ecosystem*`, `name*` |
| `register_library` | W | `slug*`, `name*`, `docs_url*`, `github_repo`, `description` |
| `scrape_library` | W, open-world (fetches pages) | `slug*`, `version*`, `max_pages`, `background` |
| `sync_changelogs` | W, open-world (GitHub) | `slug*`, `github_token`, `background` |
| `get_docs` | R | `slug*`, `version*`, `max_tokens`, `include_examples` |
| `search_docs` | R | `query*`, `slug`, `top_k`, `max_tokens`, `include_examples` |
| `get_code_examples` | R | `node_id*` |
| `get_changelog` | R | `slug*`, `from_version*`, `to_version*` |
| `list_changelog` | R | `slug*` |
| `get_job_status` | R | `job_id*` |

On hosted `/mcp`, docbrain calls go to one SQLite file per tenant (`docbrain_db_path_for_org`, `agentops-core/crates/agentops-heavy-api/src/lib.rs:431`). Decision note: "Multi-tenant docbrain isolation via one SQLite file per tenant, not a TenantContext".

## agentops-heavy-mcp tools (5, stdio only)

No annotations; R/W inferred.

| Tool | R/W (inferred) | Inputs |
| --- | --- | --- |
| `semantic_search` | R | `path*`, `query*`, `top_k`, `session_id` |
| `semantic_index` | W | `path*` |
| `search_docs_indexed` | R | `slug*`, `query*`, `top_k` |
| `index_docs_indexed` | W | `slug*`, `db_path`, `org` |
| `consolidate_model` | W, long-running | `path*` |

## Hosted-only tools (2)

Special-cased in `handle_tools_call` before the generic dispatch, so no access-mode check applies.

| Tool | R/W | Inputs |
| --- | --- | --- |
| `register_repo` | W (`idempotent_hint: true`) | `repo_url` or `local_id`, `name` |
| `unregister_repo` | W (`destructive_hint: true`) | `id*` |

## Where a new tool is added

- Core tool: one `ToolSpec` row in `tool_specs()` (`agentops-core/crates/agentops-mcp/src/tools.rs`). Listing, dispatch and access gating all derive from that row; the module doc explains this replaced three hand-synced lists. It then appears on stdio, hosted `/mcp` and REST `/tools` with no further wiring.
- Docbrain tool: `docbrain-core/crates/docbrain-mcp/src/tools.rs`; appears on stdio, hosted `/mcp`, and `/docbrain/tools`.
- Tenant-aware tool: special-case in `agentops-core/crates/agentops-heavy-api/src/mcp_http.rs` (hosted only), as `register_repo` does.

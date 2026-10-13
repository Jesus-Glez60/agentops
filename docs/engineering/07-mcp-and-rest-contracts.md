# MCP and REST contracts

Every MCP tool AgentOps defines, which surface exposes it, and under which access mode. Read this before adding a tool, changing a tool's arguments, or pointing a client at a server.

The measured inventory behind this doc is [survey/mcp-inventory.md](survey/mcp-inventory.md); counts are re-derived by `docs/engineering/scripts/mcp-tools.sh`. Where this doc and the survey differ, this doc is newer.

## Tool tables

Tools are defined in four places. Each surface composes some of them.

| Table | Defined in | Tools | Access mode | MCP annotations |
| --- | --- | --- | --- | --- |
| Core | `tool_specs` in `agentops-core/crates/agentops-mcp/src/tools.rs` | 22 | yes, `ToolSpec.access` | yes |
| docbrain | `docbrain-core/crates/docbrain-mcp/src/tools.rs` | 12 | no | no |
| Semantic index | `agentops-core/crates/agentops-heavy-mcp/src/tools.rs` | 5 | no | no |
| Hosted-only | `agentops-core/crates/agentops-heavy-api/src/mcp_http.rs` | 2 | no | yes |

41 distinct names exist. No name appears in two tables: collisions were renamed before the tables were merged (module doc of `agentops-core/crates/agentops-mcp-server/src/lib.rs`):

| Old name | Table | New name | Kept by |
| --- | --- | --- | --- |
| `semantic_search` | core | `local_semantic_search` | semantic index |
| `get_changelog` | core | `list_scans` | docbrain |
| `search_docs` | semantic index | `search_docs_indexed` | docbrain |
| `index_docs` | semantic index | `index_docs_indexed` | docbrain |

## Surfaces

| Surface | Transport | Tables | Advisor | Full | Auth |
| --- | --- | --- | --- | --- | --- |
| `agentops-mcp-server` | stdio | core + docbrain (+ semantic index if `AGENTOPS_QDRANT_URL` is set) | 26 (31) | 34 (39) | none; local process |
| `POST /mcp` on `agentops-server` | Streamable HTTP, stateless | core + docbrain + hosted-only | 28 | 36 | session or personal API key (`require_tenant_auth`) |
| `GET /tools`, `POST /tools/{name}` | REST | core | 14 | 22 | instance key `AGENTOPS_API_KEY_HASH`, or none if unset |
| `GET /docbrain/tools`, `POST /docbrain/tools/{name}` | REST | docbrain | 12 | 12 | instance key |
| `agentops serve` | stdio | core | 14 | 22 | none |
| `agentops docbrain-serve` | stdio | docbrain | 12 | 12 | none |

`agentops connect` registers `agentops-mcp-server` with the client, or the hosted `/mcp` URL with `--remote`. The two CLI stdio servers predate the merge (`findings.md` F17).

Hosted `/mcp` excludes the semantic-index table. Qdrant has no tenant concept, so those tools would need "tenant-safe `SemanticIndex` threading" first (`mcp_http.rs` module doc).

The `agentops-heavy-api` router can also mount its own `/tools` with the semantic-index table. `agentops-server` builds it without that route, to avoid a duplicate-route panic in `Router::merge`.

## Wire protocol

All MCP surfaces hand-roll JSON-RPC 2.0 instead of using an SDK. The core and docbrain copies are "deliberately duplicated, not shared" so the two products version independently (`agentops-core/crates/agentops-mcp/src/protocol.rs` module doc).

| Item | Value |
| --- | --- |
| Protocol version | `MCP_PROTOCOL_VERSION = "2024-11-05"` |
| Methods | `initialize`, `tools/list`, `tools/call` |
| Capabilities | `{ "tools": {} }` only; no resources, prompts or streaming |
| stdio framing | one JSON message per line (`run_stdio`) |
| HTTP framing | one JSON response per `POST`; no SSE |
| Notification (no `id`) | stdio: no reply. HTTP: `202 Accepted`, empty body. |
| Unknown method | JSON-RPC error `-32601` |
| `tools/call` without `name` | JSON-RPC error `-32602` |
| Unparseable request | JSON-RPC error `-32603` with "parse error"; HTTP also returns 400 |

The JSON-RPC spec's parse-error code is -32700; these servers send `-32603` (`findings.md` F34).

`serverInfo.name` differs per surface: `agentops-mcp`, `agentops-mcp-server`, or `agentops-heavy-api` on hosted `/mcp`.

### Tool results

Every tool returns a `CallToolResult`: one `text` content block and an `isError` flag. No tool returns structured JSON content. Failures are results, not protocol errors:

| Case | MCP surfaces | REST `POST /tools/{name}` |
| --- | --- | --- |
| Handler error | `isError: true`, full error chain (`{e:#}`) | 200 with the error result |
| Full-only tool in Advisor mode | `isError: true`, "requires Full access mode" | 200 with the error result |
| Unknown tool | `isError: true`, "unknown tool" | 404, `{ "error": ... }` |

On `agentops-mcp-server`, a Full-only tool called in Advisor mode reports "unknown tool". `Dispatch::call_tool` checks the mode-filtered listing before it reaches `call_tool`'s refusal.

The error chain uses `{e:#}` because a bare `{e}` showed only "db error" for Postgres failures (comment in `call_tool`).

## Access modes

`AccessMode` has two values (`agentops-core/crates/agentops-mcp/src/tools.rs`):

- `Advisor`: the default. Read tools plus three knowledge-writing tools.
- `Full`: everything.

The default is `Advisor` because the MCP client can be any editor or agent, and the server cannot know or trust which (`questions.md` A-5).

A Full-only tool is both hidden and refused. `list_tools(mode)` omits it; `call_tool` refuses it as a "defensive backstop" for a client that calls a tool it was never shown.

Only the core table is gated. docbrain, semantic-index and hosted-only tools are available in both modes. The `agentops-mcp-server` module doc says retrofitting a capability system into those crates was out of scope for the merge.

### Where the mode comes from

| Surface | Source | Read |
| --- | --- | --- |
| `agentops-mcp-server`, CLI `serve` | `AGENTOPS_ACCESS_MODE` env var, default `advisor` | at start |
| REST `/tools` | same env var | at start |
| Hosted `/mcp` | `organizations.mcp_access_mode`, default `'advisor'` (`agentops-core/crates/agentops-teams/src/lib.rs`) | every call (`resolve_access_mode`) |

An org admin changes the hosted mode with `PUT /team/mcp-access-mode`. Hosted `/mcp` falls back to the process mode when no teams store is configured. Why REST and hosted `/mcp` use different sources is open (`questions.md` Q5b).

### Knowledge writes in Advisor mode

Three core write tools are Advisor-mode on purpose. The comment on `add_note` explains: Advisor mode "exists to block destructive/costly actions"; recording a gotcha or decision is neither. `ingest_notes` uses "same reasoning". `persist_blueprint_doc` gives no reason (`questions.md` Q6).

Read tools also write, in a small way. Calls such as `get_symbol` and `list_gotchas` record a "hit" session event, under `session_id` or `UNATTRIBUTED_SESSION` ("unattributed") when none is passed (`maybe_record_session_event`).

## Tool inventory

`*` marks a required input. R/W for the core table comes from `read_only_hint`. For docbrain and semantic-index tools it comes from reading each handler.

### Core tools

| Tool | Access | R/W | Inputs | Returns |
| --- | --- | --- | --- | --- |
| `status` | Advisor | R | `path*` | Last scan's counts plus the repo-state summary, or "no scans recorded yet". |
| `list_gotchas` | Advisor | R | `path*`, `session_id` | One line per gotcha, title and node id; `Reduced` last with its reason. |
| `get_symbol` | Advisor | R | `path*`, `name*`, `file`, `session_id`, `max_chars` | Symbol source, then the gotchas and decisions that affect it, with staleness and curation markers. |
| `list_scans` | Advisor | R | `path*`, `limit` (default 10) | One line per scan with file and symbol deltas. |
| `related_context` | Advisor | R | `path*`, `symbol_id*`, `top_k`, `session_id`, `max_chars`, `compact` | Pattern-completed related symbols and their notes. |
| `fetch_content` | Advisor | R | `path*`, `id*`, `offset`, `limit` | A character slice of one node's full content. |
| `get_session_guide` | Advisor | R | `path*`, `session_id*` | Session-resume summary, at most 2,000 characters. |
| `get_session` | Advisor | R | `path*`, `session_id*` | The session's recorded events. |
| `list_tasks` | Advisor | R | `path*` | Tasks for the repo. |
| `get_task_activity` | Advisor | R | `path*`, `task_id*` | Events recorded under the task's `session_id`. |
| `local_semantic_search` | Advisor | R | `path*`, `query*`, `top_k`, `kind`, `mode`, `graph_expand` | Ranked hits; see [05-retrieval.md](05-retrieval.md). |
| `add_note` | Advisor | **W** | `path*`, `title*`, `body*`, `note_type`, `tags`, `with_embeddings`, `session_id`; also reads undeclared `notes_path` | "Wrote <file> (<type>) and ingested it" with edge counts. |
| `ingest_notes` | Advisor | **W** | `path*`, `notes_path`, `with_embeddings`, `session_id` | Notes ingested, edges written and reinforced. |
| `persist_blueprint_doc` | Advisor | **W** | `path*`, `doc_id*`, `title*`, `markdown*`, `note_title*`, `note_type`, `session_id` | "Merged '<doc_id>' into <repo>'s Blueprint doc group." |
| `scan_repo` | Full | W | `path*`, `with_embeddings`, `session_id` | Files, symbols and dependency edges written; files and symbols pruned. |
| `explain_symbol` | Full | W, open-world | `path*`, `symbol_id*`, `session_id`, `max_chars` | The explanation, stored as a `Definition` node. Costs one LLM call. |
| `end_session` | Full | W | `path*`, `session_id*` | Embedding consolidation report; see [06-notes-and-consolidation.md](06-notes-and-consolidation.md). |
| `report_usage` | Full | W | `path*`, `session_id*`, `agent*`, `calls`, `output_tokens`, `total_context_tokens`, `peak_context`, `categories` | "Recorded agent-estimated usage", or "Ignored" when a transcript-based report already exists; agent estimates never replace one. |
| `create_task` | Full | W | `path*`, `title*`, `description`, `priority`, `assignee`, `session_id` | "Created task <id>: <title>". |
| `update_task_status` | Full | W | `path*`, `task_id*`, `status*` | "Task <id> moved to <status>". |
| `init_agents_md` | Full | W | `path*`, `notes_path` | "Wrote <AGENTS.md path> (NOTES_PATH: ...)". Overwrites `AGENTS.md` (gotcha 54112). |
| `generate_docs` | Full | W | `path*` | "Wrote <repo-map.md path>". |

`local_semantic_search`'s `kind` enum omits `doc_section` and `tool_output` (`findings.md` F19). `update_task_status` accepts `todo`, `in_progress`, `in_review`, `done` and `cancelled` (`parse_task_status`).

### docbrain tools

docbrain tools take no `path`. On stdio they use one SQLite file; on hosted `/mcp` they use one file per tenant (`docbrain_db_path_for_org`). See [08-docbrain.md](08-docbrain.md).

| Tool | R/W | Inputs | Notes |
| --- | --- | --- | --- |
| `list_libraries` | R | none | |
| `get_library` | R | `slug*` | |
| `discover_library` | **W**, open-world | `ecosystem*`, `name*` | Queries a package registry and registers the library if found (`add_library`). |
| `register_library` | W | `slug*`, `name*`, `docs_url*`, `github_repo`, `description` | |
| `scrape_library` | W, open-world | `slug*`, `version*`, `max_pages`, `background` | Can run as a background job. Hosted `/mcp` injects the tenant's Firecrawl key if one is stored. |
| `sync_changelogs` | W, open-world | `slug*`, `github_token`, `background` | Reads GitHub releases. |
| `get_docs` | R | `slug*`, `version*`, `max_tokens`, `include_examples` | |
| `search_docs` | R | `query*`, `slug`, `top_k`, `max_tokens`, `include_examples` | |
| `get_code_examples` | R | `node_id*` | |
| `get_changelog` | R | `slug*`, `from_version*`, `to_version*` | Library versions, unrelated to `list_scans`. |
| `list_changelog` | R | `slug*` | |
| `get_job_status` | R | `job_id*` | Polls `scrape_library` and `sync_changelogs` jobs. |

### Semantic-index tools (stdio only, with Qdrant)

| Tool | R/W | Inputs |
| --- | --- | --- |
| `semantic_search` | R | `path*`, `query*`, `top_k`, `session_id` |
| `semantic_index` | W | `path*` |
| `search_docs_indexed` | R | `slug*`, `query*`, `top_k` |
| `index_docs_indexed` | W | `slug*`, `db_path`, `org` |
| `consolidate_model` | W, 10 to 45+ minutes of CPU | `path*` |

The `agentops-mcp-server` module doc says `search_docs_indexed` and `index_docs_indexed` "may turn out to be redundant" with docbrain's own search.

### Hosted-only tools

`handle_tools_call` handles these before generic dispatch, because they need the tenant and app state that `call_tool`'s `(mode, name, arguments)` signature does not carry.

| Tool | Annotations | Inputs | Effect |
| --- | --- | --- | --- |
| `register_repo` | idempotent | `repo_url` or `local_id`, `name` | Registers the repo as a `Discovered`, `Pending` connection. A person still connects it in the web UI. `local_id` registers a repo with no git remote. |
| `unregister_repo` | destructive, idempotent | `id*` | Removes the connection. Graph and notes data stay; the web UI's "Remove repository" does the full cleanup. |

## Hosted path resolution

On a local surface, `path` is a filesystem path, and the store's repo key is the directory name (`repo_name`).

On hosted `/mcp`, `path` is never trusted as a literal path (`mcp_http.rs` module doc). `resolve_connection_path` (`agentops-core/crates/agentops-heavy-api/src/tenant_repo.rs`) accepts one of three forms, each only within the caller's tenant:

1. A connection id.
2. A store repo name, `<connection_id>--<tenant>`, as returned by `/gotchas` and `/search`. Only the caller's own tenant suffix is stripped.
3. A connection's `repo_url`.

It returns the checkout path `<repo_checkouts_dir>/<connection_id>--<tenant>`. The store's repo name is therefore `<connection_id>--<tenant>`, not the connection id (gotcha 54242). An unknown value returns an error result that suggests `register_repo`.

Only the `path` argument is rewritten. Arguments listed in `HOSTED_FORBIDDEN_ARGS` (currently `notes_path`) are refused with an `isError` result before dispatch, because the handler would treat them as literal server paths (`findings.md` F31).

Hosted calls run inside `spawn_blocking` and `with_shared_postgres_store`. The first avoids a nested Tokio runtime panic in the Postgres adapter. The second reuses one connection pool; per-call pools deadlocked Postgres under load (comments in `handle_tools_call`).

## Output budgets

Tools that could inline unbounded node content cap it (`agentops-core/crates/agentops-mcp/src/budget.rs`):

| Mechanism | Limit | Used by | Overflow |
| --- | --- | --- | --- |
| `cap` | `DEFAULT_CHAR_BUDGET = 4000` characters, or the call's `max_chars` | `get_symbol` (source and each note), `related_context`, `explain_symbol` | Appends "call fetch_content with id N for the full content". |
| `cap_sections` | `GUIDE_CHAR_BUDGET = 2000` characters in total | `get_session_guide` | Drops the lowest-priority sections first; no pointer, because a guide is an ephemeral summary. |
| `fetch_content` | caller's `offset`/`limit`, in characters | anyone holding a node id | Pages through the full content. |

Truncation always lands on a UTF-8 character boundary.

`compress_json` and `compress_json_then_cap` exist but no tool handler calls them yet. The doc comment calls them "staged infrastructure" and the `#[allow(dead_code)]` deliberate.

`list_gotchas` has no budget. It returns titles and ids only, so its size grows with the number of gotchas, not their length.

## Session correlation

Any core tool that accepts `session_id` records a session event under it (`maybe_record_session_event`). The id is chosen by the client; the server never issues one. Decision "Module 6 session correlation is protocol-native via session_id, not a new subsystem" records why. It adds no server or protocol concept, so it works for any MCP or REST client.

| Tool | Reads |
| --- | --- |
| `get_session` | Every event for the session, in order. |
| `get_session_guide` | A prioritized summary of the session: active task, files touched, decisions, errors, git operations, stated intent. Falls back to a repo briefing when the session has no events. |
| `get_task_activity` | Events recorded under the `session_id` stored on a task. |
| `end_session` | Requires at least one recorded event before it consolidates. |

`add_note` records a decision with event kind `decision`, which feeds the guide's "Decisions" section. Other writes record `activity`; reads record `hit`.

The same guide is built for Claude Code's `SessionStart` hook by `agentops-cli`, so the tool and the hook share one implementation (`agentops-core/crates/agentops-mcp/src/session_guide.rs` module doc). Pinned nodes are injected on every `SessionStart` and `UserPromptSubmit` (`build_pinned_context`). See [13-agent-integration.md](13-agent-integration.md).

## REST parity

REST mirrors the tool tables, not each tool:

- `POST /tools/{name}` calls the same `agentops_mcp::call_tool` the MCP surfaces use, so arguments and results are identical. The body is the tool's `arguments` object.
- `POST /docbrain/tools/{name}` does the same for docbrain.

The two REST routes differ from hosted `/mcp` in three ways:

1. Auth is the instance-wide key only; a personal API key is rejected (`check_bearer_api_key`). Without `AGENTOPS_API_KEY_HASH` the routes would be unauthenticated, so `agentops-server` refuses to start on a non-loopback address without it ([09-auth-teams-security.md](09-auth-teams-security.md), "Unset instance key").
2. `path` is a literal server path, not a connection id. The routes assume a trusted operator.
3. The mode is the process's `AGENTOPS_ACCESS_MODE`, not the org's setting.

So a hosted user cannot record a note with `POST /tools/add_note` and a personal key. `docs/vault-protocol.md` now says so; `docs/openapi.yaml` still describes `apiKeyAuth` loosely (`findings.md` F33).

Dashboard REST routes such as `/gotchas`, `/local-search` and `/repos/{id}/nodes/{node_id}/curation` are separate handlers for the web UI, not tool wrappers. Their behavior can differ from the matching tool; `/local-search` is dense-only, for example ([05-retrieval.md](05-retrieval.md)).

### OpenAPI spec

`docs/openapi.yaml` is hand-written, not generated. It was added with decision "README, OpenAPI spec, llms.txt, and docbrain self-seed shipped". Any route change needs a matching manual edit. Known gaps:

- `/mcp`'s description says it exposes only the core tool set. It also exposes docbrain and the two hosted-only tools.
- `apiKeyAuth` describes both `agentops api-key generate` and `POST /auth/api-keys` keys. `/tools/*` accepts only the first kind (`findings.md` F33).
- `/tools/{name}` documents only a 200; an unknown tool returns 404.

## Adding a tool

| Kind | Where | What you get for free |
| --- | --- | --- |
| Core | one `ToolSpec` row in `tool_specs()`: `name`, `description`, `access`, `annotations`, `input_schema`, `handler` | Listing, dispatch and access gating on stdio, hosted `/mcp`, REST `/tools` and CLI `serve` |
| docbrain | `docbrain-core/crates/docbrain-mcp/src/tools.rs` | stdio, hosted `/mcp` (per-tenant store), REST `/docbrain/tools` |
| Needs the tenant or app state | a special case in `handle_tools_call`, plus a `ToolDefinition` added to the hosted `tools/list` | hosted `/mcp` only |

Rules for a core tool:

- Pick `access` deliberately. `Advisor` makes a write tool callable by every client by default.
- Every core tool takes `path` first. On hosted `/mcp`, only `path` is rewritten, so any other argument that names a file or directory must be validated by the handler.
- Accept `session_id` if the tool should appear in session activity, and call `maybe_record_session_event`.
- Cap any node content with `budget::cap` and the call's `max_chars`.
- Make the handler synchronous. Async surfaces already wrap `call_tool` in `spawn_blocking`.
- Declare every argument the handler reads in `input_schema`.
- Update `docs/openapi.yaml` only if a REST route changes; tools are listed at runtime by `GET /tools`.

`ToolSpec` replaced three hand-synced lists (listing, dispatch, access) that could drift (`tool_specs` doc comment).

## Open questions

- [Q5b](questions.md): why hosted `/mcp` reads the mode per org but REST `/tools` per process.
- [Q6](questions.md): which write tools are meant to be open in Advisor mode.
- Related findings: F17 (CLI stdio servers), F19 (`kind` enum), F31 (hosted `notes_path`), F33 (REST note path for hosted users), F34 (parse-error code).

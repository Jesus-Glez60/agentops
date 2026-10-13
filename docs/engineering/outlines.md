# Outlines (Phase 2)

One outline per deliverable in `BRIEF.md`: headings plus one line on what goes there and its source. Approve, cut or move sections here; no full document is written before its outline is approved.

## Author review (2026-10-12)

- **Status: approved**, with these changes:
  1. `02-crate-reference.md` is split into semantic groups: an index file plus `02-crates/<group>.md`. The coverage check runs over all of them.
  2. `01-architecture.md` keeps the added `## Cross-cutting invariants` section.
  3. `10-frontends.md` keeps the UI decisions and expands them as needed.
  4. The runbook in `11-deployment-and-operations.md` is complete: the recorded gotchas cover every failure hit during development.
  5. ADRs and docs get as much detail as the evidence supports.

Conventions for every doc:
- Opens with a two-sentence summary (drafted below as **Summary**).
- Ends with `## Open questions`, linking the open items in `questions.md` that affect it.
- Sources marked *(verify)* were located by name but not yet read; the doc is not written from them until they are.

---

## 00-index.md

**Summary:** Entry point to the AgentOps engineering docs. Read first; it says which doc answers which question and in what order a new maintainer should read them.

- `## Documents`: one line per doc 01–13, plus `adr/` and `findings.md`. Source: the docs themselves (written last).
- `## Reading order`: new maintainer path (01 → 02 → 03 → 07 → the subsystem they touch); agent path (07 → 02 → topic doc). Source: dependency of docs on each other.
- `## Glossary`: node, edge, gotcha, decision, note, definition, curation/prominence, consolidation, advisor/full, heavy (historical, per `questions.md` A-2), connection, tenant/org, docbrain. Source: `NodeKind`/`EdgeRelation` in `agentops-core/crates/agentops-graph/src/lib.rs`, `AccessMode` in `agentops-core/crates/agentops-mcp/src/tools.rs`, author answers.
- `## Related documentation`: links to `README.md` (install/connect), `docs/openapi.yaml`, `docs/vault-protocol.md`, `SECURITY.md`, `CONTRIBUTING.md`, `apps/web/CLAUDE.md`. Source: repo files.

## 01-architecture.md

**Summary:** How AgentOps is put together: its layers, its three binaries and how a request flows through each transport. For anyone who needs the big picture before changing code.

- `## System overview`: what the system does in five sentences: scan → graph → retrieval + notes → MCP/REST/web. Source: `README.md`, `agentops-core/crates/agentops-server/src/lib.rs`.
- `## Component diagram`: Mermaid graph of the approved layers (`questions.md` A-4) with the main dependency edges. Source: `docs/engineering/scripts/crate-deps.sh`.
- `## Layers`: the approved layer table, one sentence per layer. Source: `survey/layers.md`, A-4.
- `## Binaries`: per binary, what it wires together and its config env vars.
  - `agentops-server`: merges `agentops_api::build_router_without_dashboard_routes`, `/docbrain` (docbrain-api), `agentops_heavy_api::build_full_router(.., false)`, `connect_sh::router`. Source: `agentops-core/crates/agentops-server/src/lib.rs`.
  - `agentops-mcp-server`: stdio `Dispatch` over agentops-mcp, docbrain-mcp, optional heavy-mcp. Source: `agentops-core/crates/agentops-mcp-server/src/lib.rs`, `main.rs`.
  - `agentops` (CLI): subcommands grouped by purpose; `serve-api` embeds `agentops_server::run`. Source: `agentops-core/crates/agentops-cli/src/main.rs` (`Command` enum).
- `## Request traces`: one traced call per transport, file by file.
  - stdio MCP `tools/call` → `Dispatch::call_tool` → `agentops_mcp::call_tool` → handler → `GraphStore`.
  - hosted `POST /mcp` → tenant auth → `handle_tools_call` → `resolve_connection_path` → `with_shared_postgres_store` → `call_tool`.
  - REST `POST /tools/{name}` → `call_tool_handler` → same `call_tool`.
  - Source: files above plus `agentops-core/crates/agentops-heavy-api/src/mcp_http.rs`, `agentops-core/crates/agentops-api/src/lib.rs`.
- `## Cross-cutting invariants`: one place for rules that span crates. Examples: no runtime network egress in `agentops-scanner` (root `Cargo.toml` comment, `SECURITY.md`); `spawn_blocking` around `GraphStore` calls in async handlers (doc comments in `agentops-api`/`agentops-heavy-api`); `agentops-retrieval` must not depend on `agentops-embeddings-train` (decision note).
- `## Architecture map`: whether and how `repo-map.md` is referenced. Blocked by Q9.

## 02-crate-reference.md

**Summary:** One entry per workspace crate: what it is for, its public entry points, what it depends on, its invariants and its tests. For anyone about to change a specific crate.

- `## How to read this reference`: entry template and ordering (by approved layer, then name).
- One `## <crate-name>` per crate, 36 total, each with:
  - **Purpose**: one or two sentences. Source: crate `lib.rs` module doc.
  - **Entry points**: main `pub` items. Source: `grep '^pub (fn|struct|enum|trait)'` per crate.
  - **Internal deps / dependents**: from `survey/layers.md`.
  - **Invariants**: from module docs and gotchas/decisions that name the crate (gotcha titles cited).
  - **Tests**: inline `#[cfg(test)]` modules and `tests/` dirs. Source: grep per crate; Postgres-backed tests need `AGENTOPS_TEST_DATABASE_URL` (gotcha 54247, "Postgres-backed tests skip silently unless AGENTOPS_TEST_DATABASE_URL reaches a live DB").
- `## Coverage check`: how to run `scripts/crate-deps.sh --check docs/engineering/02-crate-reference.md`.

## 03-data-model.md

**Summary:** What AgentOps stores, where, and how the schema evolves. For anyone changing node or edge types, storage backends or migrations.

- `## Graph model`: `NodeKind` (Symbol, File, Gotcha, Decision, Definition, Note) and `EdgeRelation` variants, with what creates each. Source: `agentops-core/crates/agentops-graph/src/lib.rs`.
- `## Node identity`: natural key and its known collisions. Source: gotchas 54079 ("Root cause of the duplicate-natural-key symbols ...") and 53901; `agentops-core/crates/agentops-scanner/src/types.rs` *(verify)*.
- `## Curation fields`: `curated`, `prominence`, `curation_reason`; `prominence_rank_multiplier`. Source: decision "Gotchas are curated, never resolved/dismissed ...", `agentops-graph`.
- `## Edge weights and decay`: weighted decaying edges, `reinforce_edge`. Source: decision "weighted decaying edges, auto note re-matching, and repo state added"; gotcha 53965 *(verify code)*.
- `## SQLite backend`: tables (`nodes`, `edges`, `nodes_fts`, `node_vectors`, `scan_history`, `session_events`, `session_usage`, `tasks`, `task_links`, `repo_state`, `doc_pages`, `node_versions`). Source: `agentops-core/crates/agentops-graph/src/sqlite.rs`; table list measured from `.context/graph.db`.
- `## Postgres backend`: `PostgresGraphStore`, pgvector, check constraints, `wipe_repo`. Source: `agentops-core/crates/agentops-graph-pg/src/lib.rs`; gotchas 54053, 54241, 54055, 54054.
- `## Backend selection`: `agentops-store-open` and `AGENTOPS_DATABASE_URL`. Source: `agentops-core/crates/agentops-store-open/src/lib.rs`.
- `## Where embeddings live`: `node_vectors` (sqlite-vec) vs pgvector vs Qdrant (BGE-M3, stdio heavy path only). Source: `agentops-embeddings`, `agentops-heavy-embeddings`, gotcha 54092 (sqlite-vec k ≤ 4096).
- `## Other stores`: accounts, teams, integrations, modules, heavy-api, docbrain (global + per-tenant files), manifest. One table: file, owning crate, env var. Source: `.env.example`, `AGENTOPS_DATA_DIR` decision note.
- `## Migrations`: `rusqlite_migration` append-only rule; tables without a migration framework (`repo_connections`). Source: gotchas 54076, 54216, 54088.
- `## Migrating between backends`: `agentops migrate-graph`. Source: `MigrateGraph` in `agentops-core/crates/agentops-cli/src/main.rs` *(verify)*.
- `## Deletion semantics`: org deletion cascade and what it leaves behind. Source: decision on org deletion; `findings.md` F10.
- Open: Q10.

## 04-indexing-pipeline.md

**Summary:** How a repository becomes a graph: walk, parse, link, embed and document. For anyone changing the scanner, rescans or reindex triggers.

- `## Pipeline overview`: Mermaid sequence diagram, scan → persist → link notes → embed → docgen.
- `## Walking the repo`: `ignore`-based walker, gitignore rules. Source: `agentops-scanner/src/walker.rs`; gotcha 53927 ("ignore::WalkBuilder only honors .gitignore inside an actual git repo").
- `## Parsing`: tree-sitter via `tree-sitter-language-pack`, build-time grammars, no runtime download. Source: `agentops-scanner/src/ast_extract.rs`, root `Cargo.toml` comment; gotcha 54110 (Windows CRT).
- `## Dependency edges`: `dep_extract.rs`, known gaps. Source: gotcha 53987 ("dependency-edge resolution gap on Rust crate:: and tsconfig aliases").
- `## Persisting and rescans`: incremental upsert, pruning stale nodes, dedupe. Source: `agentops-mcp/src/scan.rs` *(verify)*; gotchas 54078, 54057.
- `## Note linking`: auto re-matching after scans. Source: `agentops-notes/src/vault.rs` `match_symbols`; gotcha 54035.
- `## Embeddings`: `with_embeddings`, what gets embedded. Source: `agentops-embeddings`; gotcha 53966.
- `## rust-analyzer pass`: `--with-lsp`, `agentops-lsp-client`, retry on `ContentModified`. Source: `Install { with_lsp }` in `agentops-cli`, commit `8565155`, gotchas 53904, 54116.
- `## Documentation generation`: `agentops-docgen`, `repo-map.md`, `sectioned.rs`. Source: `agentops-docgen/src/*`; decision "Documentation Viewer backend: ... LLM-free docgen ...".
- `## Watch mode`: `agentops watch`, per-directory watching. Source: `agentops-mcp/src/watch.rs`; gotcha 53999.
- `## Hosted indexing and push-triggered reindex`: indexing jobs, GitHub `push` webhook. Source: `agentops-heavy-api/src/indexing.rs`, `github_app_routes.rs` (`"push"` arm, line ~483); gotchas 54077, 54215, 54007.

## 05-retrieval.md

**Summary:** How a search query is scored and expanded. For anyone tuning or extending search.

- `## Retrieval modes`: `dense`, `hybrid`, `gist_then_detail`. Source: `local_semantic_search` schema in `agentops-mcp/src/tools.rs`.
- `## Candidate sources`: `DenseCandidateSource`, `LexicalCandidateSource` (SQLite `bm25` / Postgres equivalent), `ExactCandidateSource`. Source: `agentops-core/crates/agentops-retrieval/src/lib.rs`.
- `## Retrieval fusion`: reciprocal rank fusion, `RRF_K = 60`. Source: same file, lines ~234–338.
- `## Graph expansion`: `personalized_pagerank` over Affects/References, `graph_expand`, path citations (`shortest_path`, `GraphPathHop`). Source: same file; commit `4a3cd12`.
- `## Gist-then-detail`: `search_gist_then_detail`. Source: same file.
- `## Prominence dampening`: reduced-prominence gotchas ranked ×0.1. Source: `agentops_graph::prominence_rank_multiplier`.
- `## Pattern completion`: `pattern_complete`, `related_context`. Source: same file; tool schema.
- `## Embedding projection`: `EmbeddingProjector` trait and why retrieval does not depend on training. Source: decision note.
- `## Semantic index path (Qdrant)`: BGE-M3 + Qdrant via `agentops-heavy-embeddings`, stdio only. Source: that crate, `agentops-mcp-server/src/main.rs`.
- `## Tuning knobs`: `top_k`, `kind`, `graph_expand`, damping/iterations, `max_chars`/budget caps. Source: function signatures, `agentops-mcp/src/budget.rs` *(verify)*.

## 06-notes-and-consolidation.md

**Summary:** How tribal knowledge (gotchas, decisions, notes) is recorded, matched to code and kept useful, and how local consolidation trains on it. For anyone working on notes, curation or training.

- `## Note types`: gotcha, decision, note; classification. Source: `agentops-notes/src/vault.rs` (`heuristic_score`, `HeuristicClassifier`).
- `## Writing notes`: `add_note`, `ingest_notes`, notes path resolution, vault folder. Source: `agentops-notes/src/config.rs`, `agentops-mcp/src/notes.rs`.
- `## Vault protocol`: summary + link. Source: `docs/vault-protocol.md`.
- `## Notes live in the graph`: hosted `add_note`; `.claude/hooks/block-local-notes.sh` blocks local note files when `.context/agentops-remote.json` exists *(verify script)*.
- `## Symbol matching`: `match_symbols`, `WordBoundaryMatcher`, `connect_many`. Source: `vault.rs`.
- `## Staleness and re-matching`: stale flagging, auto re-match, `updated_at` rule. Source: decision on weighted edges; gotcha 53965 *(verify where stale flags live)*.
- `## Curation`: keep vs reduce prominence; no delete. Source: decision "Gotchas are curated, never resolved/dismissed ...".
- `## Embedding consolidation`: `agentops-embeddings-train` (replay, projection head, `recall_at_k` eval). Source: that crate; gotcha 53969.
- `## Generative consolidation`: `agentops-heavy-consolidate` (hand-rolled LoRA, `evaluate`), `consolidate_model` tool, runtime cost. Source: that crate; gotchas 53970, 53983.
- `## No-regression gate`: what each eval compares and when an adapter is rejected. Blocked by Q7; source: both `eval.rs` files.
- `## Librarian`: `agentops librarian` dry run, label export. Source: `agentops-cli/src/librarian.rs`, `agentops-llm/src/librarian.rs`.
- Open: Q6 (`ingest_notes`/`persist_blueprint_doc` in Advisor), Q7.

## 07-mcp-and-rest-contracts.md

**Summary:** Every MCP tool and how each surface exposes it, plus the rules that keep REST and MCP in step. For anyone adding a tool or integrating a client.

- `## Surfaces`: stdio, hosted `/mcp`, REST `/tools`, `/docbrain/tools`; counts per access mode. Source: `survey/mcp-inventory.md`.
- `## Access modes`: Advisor/Full, hide + refuse, where each surface reads the mode. Source: inventory; `questions.md` A-5.
- `## Tool inventory`: the full table (name, inputs, output, R/W, access, surfaces). Source: inventory, with outputs added from each handler *(verify per tool)*.
- `## Adding a tool`: `ToolSpec` row; docbrain table; tenant-aware special case. Source: inventory "Where a new tool is added".
- `## Hosted path resolution`: `path` as connection id, `<connection_id>--<tenant>`. Source: `resolve_connection_path`; gotcha 54242.
- `## Output budgets`: capped output, `fetch_content` paging. Source: commit `31c1723`; `agentops-mcp/src/budget.rs` *(verify)*.
- `## Session correlation`: `session_id` across tools, `get_session`, `get_session_guide`. Source: decision "Module 6 session correlation is protocol-native via session_id ...".
- `## REST parity`: what REST mirrors and what it does not; link to `docs/openapi.yaml` (hand-written, update rule). Source: decision "README, OpenAPI spec, llms.txt, and docbrain self-seed shipped".
- Open: Q5b, Q6.

## 08-docbrain.md

**Summary:** How AgentOps ingests and searches third-party library documentation. For anyone changing scraping, versioning or doc search.

- `## Model`: libraries, versions, doc nodes, synapse edges, changelogs. Source: `docbrain-core/crates/docbrain-graph/src/*`.
- `## Discovery and registration`: `discover_library`, `register_library`. Source: `docbrain-ingest/src/discover.rs`.
- `## Scraping`: version-pinned, background jobs, `llms.txt` preference, optional Firecrawl fallback. Source: `docbrain-ingest/src/scrape.rs`; decision on `llms.txt`; commit `c2ef9a8`.
- `## Chunking and classification`: `chunk.rs`, `classify.rs` (code examples vs prose); topic length cap. Source: those files; gotcha 54239.
- `## Retrieval`: `search_docs`, `get_docs` token budgets, `get_code_examples`. Source: `docbrain-ingest/src/retrieve.rs`.
- `## Changelogs`: `sync_changelogs`, version-pair mismatch. Source: `changelog.rs`; gotchas 54002, 53990.
- `## Shared embeddings layer`: reuse of `agentops-embeddings`; heavy `search_docs_indexed` path. Source: deps in `survey/layers.md`.
- `## Multi-tenancy`: one SQLite file per tenant on hosted; global store elsewhere; self-seed regression. Source: decision note, `docbrain_db_path_for_org`; `findings.md` F3.

## 09-auth-teams-security.md

**Summary:** Who can do what, how they prove it, and how secrets are protected. For anyone touching auth, roles, keys or the MCP access mode.

- `## Trust boundaries`: diagram of browser, CLI, coding agent, server, GitHub. Source: `agentops-server` composition, `mcp_http.rs`.
- `## Accounts and sessions`: signup modes, session cookie, 2FA (TOTP, challenge token). Source: `agentops-accounts/src/lib.rs`; decisions on 2FA; gotcha 54081.
- `## Organizations and roles`: tenant = org, owner column, fixed + custom roles, permission matrix, lazy membership backfill. Source: `agentops-teams/src/lib.rs`, `capabilities.rs`; decisions S1–S3; gotcha 53997.
- `## Invites`: copyable links, signup `invite_token` rule. Source: decisions S5, S11.
- `## API keys`: generation, hashing, `AGENTOPS_API_KEY_HASH`. Source: `agentops-security/src/api_key.rs`, `ApiKey` CLI command.
- `## Device-flow login`: CLI `device_flow_login` → `/auth/cli/device/*` → `/cli-auth` approval page. Source: `agentops-cli/src/main.rs:1516`, `apps/web/src/app/cli-auth/page.tsx`, accounts routes *(verify)*.
- `## Secrets`: `AGENTOPS_SECRETS_MASTER_KEY`, integrations vault, personal-credential key separation, redaction. Source: `agentops-repo-access/src/secrets.rs`, `agentops-security`; decision S6.
- `## Repository access`: SSH deploy keys, GitHub App, webhook secret. Source: `agentops-repo-access`, `agentops-github-app`; gotcha 54093; RUSTSEC acceptance (S10).
- `## Audit log`: insert-only, Owner/Admin visibility. Source: decisions S4, S8.
- `## MCP access-mode enforcement`: link to 07; per-org vs per-process. Open: Q5b, Q6.
- `## Known gaps`: global docbrain router (gotcha 30341), org deletion leftovers (F10).

## 10-frontends.md

**Summary:** How the web app and the marketing site are built and how they talk to the backend. For anyone changing UI code.

- `## apps/web structure`: Next.js app routes (`(app)/docs`, `gotchas`, `graph`, `libraries`, `profile`, `repositories`, `search`, `settings`; `cli-auth`, `invite`, `login`, `setup`, `welcome`). Source: `apps/web/src/app`.
- `## Base path`: `/suite` via `BASE_PATH`, `withBasePath` for raw fetches. Source: `apps/web/next.config.ts`; gotcha 53913.
- `## Calling the API`: server-side proxy (`heavy-proxy.ts`, `heavy-api.ts`, `AGENTOPS_HEAVY_API_URL`), auth gate in `proxy.ts`. Source: `apps/web/src/lib/server/*`, `apps/web/src/proxy.ts`; gotchas 54056, 54031.
- `## Data fetching`: SWR, mandatory `error` handling. Source: gotchas 53942, 53945.
- `## UI conventions`: shadcn + `cn()`/tailwind-merge pitfalls, shared node-kind colors, graph view rules. Source: the 7 frontend decisions in `survey/adr-candidates.md`; gotchas 53937, 54038.
- `## Testing`: vitest + jsdom polyfills, Playwright notes. Source: `apps/web/vitest.config.mts`; gotchas 53978, 54052.
- `## apps/marketing`: Astro site, no login by design, env vars. Source: `apps/marketing/src`; decision "Marketing site has no login/register flow by design"; gotchas 53962, 54021.
- Link, don't repeat: `apps/web/CLAUDE.md`.

## 11-deployment-and-operations.md

**Summary:** How to run AgentOps in production and what to do when it breaks. For self-hosters and on-call maintainers.

- `## Configuration reference`: env vars grouped by concern, link to `.env.example` (29 vars). Source: `.env.example`; gotcha 54032 (no `.env` auto-load).
- `## Storage choices`: SQLite vs Postgres, data dir layout. Link to 03. Source: `AGENTOPS_DATA_DIR` decision; gotcha 53950.
- `## Deploy methods`: Docker (`Dockerfile`, `docker-compose.yml`, Debian trixie), Kubernetes (`deploy/k8s/*`), PM2 (`ecosystem.config.js`), `install.sh`, Caddy (`Caddyfile.example`). Source: those files; gotcha 53991.
- `## Upgrades`: migrations on boot, compose override rules. Source: gotchas 53952, 54246, 53992 *(verify upgrade path)*.
- `## Health and logging`: `/health`, logging via `println!` in heavy-api. Source: gotcha 53954.
- `## Runbook`: scan stuck, embeddings missing, webhook not firing, Postgres not ready on cold start, stale binary on a port. Source: gotchas 54077, 54006, 54215, 53908, 54094.
- Open: Q8.

## 12-development-workflow.md

**Summary:** How to build, test and change AgentOps safely. For contributors.

- `## Build and test`: `cargo build`, `cargo test`, `TSLP_LANGUAGES` (`.cargo/config.toml`), web `npm` scripts. Source: `CLAUDE.md`, `.cargo/config.toml` *(verify)*, `apps/web/package.json`.
- `## Test environments`: Postgres tests, git/ssh-clean environment. Source: gotchas 54247, 54248.
- `## Supply chain`: `deny.toml`, accepted advisories, Dependabot, SHA-pinned actions. Source: `deny.toml`, `.github/dependabot.yml`; decision S10.
- `## Release`: `.github/workflows/release.yml`, npm packaging (`npm/`). Source: those files *(verify)*.
- `## Continuous integration`: blocked by Q8.
- `## Claude Code setup`: hooks (`.claude/settings.json`, `block-local-notes.sh`), skills (`.claude/skills/*`, 16), review agents (`plan-council-member`, `plan-council-chair`, `ponytail-auditor`, `vault-archivist`), how `audit-plan`/`wrap` use them. Source: `.claude/`.
- `## Contribution rules`: link `CONTRIBUTING.md`, `CODEOWNERS`; no AI attribution trailer. Source: those files, root `CLAUDE.md`.

## 13-agent-integration.md

**Summary:** How AgentOps plugs into coding tools: registration, instructions, hooks and usage tracking. For anyone supporting a new editor or agent.

- `## Supported tools`: Claude Code, Cursor, Codex, Gemini CLI, Windsurf; why Cline, Zed and Copilot are excluded. Source: `connect` in `agentops-cli/src/main.rs`; decisions T1.
- `## connect`: local vs `--remote`, `--global`, `--device-login`, `--access-mode`, `.context/agentops-remote.json`. Source: `Connect` args in `main.rs`, `connect_global`.
- `## Ruler and instruction files`: `agentops-ruler-bridge`, prompt pack under `agentops-core/prompts/`, mirrors. Source: that crate; gotchas 54073, 54113, 54118, 54122.
- `## AGENTS.md generation`: `init_agents_md`, full overwrite, untracked. Source: `agentops-agents-md`, `agentops-mcp/src/init.rs`; gotcha 54112.
- `## Hooks`: `hook-capture` (PostToolUse), `compress-output` (PreToolUse, PIPESTATUS), schema pitfalls. Source: `agentops-cli/src/hook_capture.rs`, `compress_output.rs`; gotchas 53900, 53971, 53972, 54240.
- `## Usage tracking`: `report_usage`, SessionEnd sync, `agentops usage`, LLM spend ledger, pricing table. Source: `agentops-cli/src/usage*.rs`, `agentops-llm/src/pricing.rs`; gotchas 54234, 54236.
- `## connect.sh`: one-line installer served by the server. Source: `agentops-server/src/connect_sh.rs`.

## adr/NNNN-<slug>.md (Phase 5)

Template per record: `## Status` · `## Context` · `## Decision` · `## Alternatives considered` · `## Consequences` · `## Evidence` (commits, note titles, code). One ADR per candidate the author selects from `survey/adr-candidates.md` (41 listed). No ADR is written whose "why" is not in a note, code comment or `questions.md` answer.

## findings.md (Phase 5)

Already exists. Phase 5 adds what later phases find and re-checks each item's status.

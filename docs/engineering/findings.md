# Findings

Bugs, inaccuracies, dead code and stale knowledge found while writing the engineering docs. Nothing listed here is fixed by this task; each item says what proves it.

Status values: **confirmed** (proven from repo artifacts), **likely** (read from code or config, not run), **resolved** (true once, fixed in code since).

## Summary

| Group | Findings |
| --- | --- |
| **Security (hosted server)** | F31 `notes_path` unchecked on `/mcp`; F35 scraping fetches internal URLs; F37 REST auth fails open without the instance key; F38 roles skip MCP and curation; F39 no auth rate limits |
| Hosted data correctness | F27 stages 7–8 ignore Postgres; F28 web search finds no code; F30 hosted note files wiped on reindex; F36 `sync_docs` store; F3 self-seed |
| Cost | F24 paid LLM call on every scan; F29 LoRA adapter unused |
| Dead or unreachable code | F8, F25, F17 |
| Configuration and deployment | F15, F16, F41, F42, F43 |
| Docs and comments out of date | F1, F2, F4–F6, F11, F14, F18, F21, F22, F23, F33, F40 |
| Small defects | F7, F9, F12, F13, F19, F20, F26, F32, F34, F44 |
| Resolved | F10 |

Keep the security entries out of any public commit until they are fixed (`SECURITY.md`).

## Documentation inaccuracies

### F1. "About 40 MCP tools" matches no actual surface

- Status: confirmed.
- `README.md:11` and `llms.txt:5` say "about 40 tools".
- Measured surfaces: stdio 26/34 (31/39 with Qdrant), hosted `/mcp` 28/36, REST `/tools` 14/22 (Advisor/Full). 41 distinct names exist in total.
- Proof: `docs/engineering/survey/mcp-inventory.md`, `docs/engineering/scripts/mcp-tools.sh`.

### F2. README links a file that is gitignored

- Status: confirmed.
- `README.md:159` links `repo-map.md` as the "Architecture map"; `.gitignore:48` ignores `repo-map.md`. On GitHub the link is dead.

### F3. AgentOps's self-registration in docbrain is invisible to hosted orgs (regression)

- Status: confirmed.
- `llms.txt:5`: AgentOps's own docs "are registered as a library automatically on first boot".
- Intended design (decision note "README, OpenAPI spec, llms.txt, and docbrain self-seed shipped", commit `5918ede`, 2026-08-25): `agentops_server::run` upserts `agentops` as a docbrain library on every boot, **metadata only**. A user then runs `scrape_library` like for any dependency. The code still does this (`agentops-core/crates/agentops-server/src/lib.rs`).
- Regression: `eba0e98` (2026-09-04) routed hosted `/mcp` docbrain calls to one SQLite file per tenant (`docbrain_db_path_for_org`, `agentops-core/crates/agentops-heavy-api/src/lib.rs:431`). The boot-time upsert still writes only to the global store (`docbrain_mcp::default_db_path()`). Org users never see the entry: on 2026-10-12, `get_library` with slug `agentops` over hosted `/mcp` returned "no library 'agentops' registered".
- The `llms.txt` wording ("same scan-and-search treatment") also implies the docs are scraped, which the design never did automatically.
- Related gotcha: 30341, "docbrain_router mounted with one global, non-tenant store on the hosted multi-tenant server".
- Author decision A-1: AgentOps will not use this for its own docs, so fixing it is low priority.

### F4. BRIEF.md: `add_note` does not need Full mode

- Status: confirmed.
- `BRIEF.md` says to "switch to `full` mode" for the `add_note` step. `add_note` is `AccessMode::Advisor` on purpose (`agentops-core/crates/agentops-mcp/src/tools.rs:96-102`). So are `ingest_notes` and `persist_blueprint_doc`.

### F5. BRIEF.md: CI is not in `.github/`

- Status: confirmed.
- `BRIEF.md` lists "CI in `.github/`" for doc 12. `.github/workflows/` contains only `release.yml`. Where tests run is `questions.md` Q8.

### F6. BRIEF.md layer table is not a dependency layering

- Status: confirmed.
- The brief says the grouping "comes from each crate's internal dependencies". Only the Foundations row does. The heavy crates are not hosted-only. Details: `docs/engineering/survey/layers.md`.

## Contract inconsistencies

### F7. docbrain and heavy tools carry no MCP annotations

- Status: confirmed.
- `agentops-mcp` tools publish `readOnlyHint`/`destructiveHint`, as do `register_repo`/`unregister_repo`. `docbrain-mcp` and `agentops-heavy-mcp` `ToolDefinition` structs have no annotations field (`docbrain-core/crates/docbrain-mcp/src/protocol.rs`, `agentops-core/crates/agentops-heavy-mcp/src/protocol.rs`). Clients cannot tell `scrape_library` (write, network) from `get_docs` (read).

## Dead code

### F8. `agentops_heavy_api::run` has no callers

- Status: confirmed.
- `pub async fn run(addr, db_path)` in `agentops-core/crates/agentops-heavy-api/src/lib.rs` is the old standalone entry point. Nothing in the workspace calls it (`grep -rn 'heavy_api::run'`: zero hits). The crate has no `[[bin]]` since the open-core merge (`218c3fc`).
- The `include_tools = true` branch of `build_full_router` is then reached only from `run` and tests.

## Repo hygiene

### F9. Stray backups at the repo root

- Status: confirmed.
- `AGENTS.md.bak` and `CLAUDE.md.bak` exist at the root. They are likely left by `init_agents_md`, which fully overwrites both files (gotcha 54112, "AGENTS.md/CLAUDE.md have no hand-edit-preserved zone -- init_agents_md fully overwrites them every install, and they're not even git-tracked").

## Naming

### F11. "heavy" names are leftovers of the abandoned paid tier

- Status: confirmed (author answer A-2 in `questions.md`).
- "heavy" was the paid suite of a dropped SaaS plan; it means nothing today. The name survives in:
  - crates `agentops-heavy-api`, `agentops-heavy-embeddings`, `agentops-heavy-consolidate`, `agentops-heavy-mcp`;
  - env vars `AGENTOPS_HEAVY_API_DB` (`.env.example:33`) and `AGENTOPS_HEAVY_API_URL` (`Dockerfile`, `ecosystem.config.js`, `apps/web/src/lib/server/heavy-api.ts`);
  - the data file `heavy-api.sqlite` (`.env.example:19`).
- Effect: readers infer a tier boundary that does not exist (the brief itself did). The docs explain it once in the glossary; renaming is out of scope.

## Phase 3 additions

### F12. Permission matrix advertises "Resolve or dismiss gotchas"

- Status: confirmed.
- `CAP_GOTCHAS_RESOLVE` is labelled "Resolve or dismiss gotchas" (`agentops-core/crates/agentops-teams/src/lib.rs:199`). The decision "Gotchas are curated, never resolved/dismissed -- don't reintroduce a bug-tracker model" says no such action exists. The constant has no other reference, so nothing enforces it; it only shows in the matrix.

### F13. Billing capability and role from the abandoned paid tier

- Status: confirmed.
- `CAP_TEAM_BILLING` ("Manage billing & subscriptions", roles `admin`, `billing`) at `agentops-core/crates/agentops-teams/src/lib.rs:205`. No other reference. Leftover of the SaaS plan (`questions.md` A-2).

### F14. `agentops-linear` module doc says there is no webhook support

- Status: confirmed.
- The module doc says sync is "Poll-based, not webhook-based ... no inbound-webhook infrastructure exists anywhere in the codebase today". `agentops-core/crates/agentops-heavy-api/src/linear_webhook.rs` exists, and the crate exports `ensure_webhook_registered`.

### F15. Env vars read by the code but missing from `.env.example`

- Status: confirmed (2026-10-12, `grep 'env::var("..")'` over all crates).
- Missing operator-facing vars: `AGENTOPS_ANTHROPIC_MODEL`, `AGENTOPS_ANTHROPIC_MODEL_CHEAP`, `AGENTOPS_GITHUB_INSTALL_STATE_SECRET`, `AGENTOPS_PG_CONNECT_RETRY_SECS`, `AGENTOPS_PG_POOL_MAX_SIZE`, `AGENTOPS_INSTALL_DIR`.
- Test-only, fine to omit: `AGENTOPS_TEST_DATABASE_URL`, `AGENTOPS_TEST_QDRANT_URL`.

### F16. Model stores ignore `AGENTOPS_DATA_DIR`; data-dir logic is duplicated

- Status: confirmed.
- `ProjectionStore::open` (`agentops-core/crates/agentops-embeddings-train/src/store.rs`) and the adapter store (`agentops-core/crates/agentops-heavy-consolidate/src/store.rs`) hardcode `$HOME/.agentops/models/{repo}`. Everything else resolves `AGENTOPS_DATA_DIR` first (`agentops_manifest::agentops_data_dir`).
- `agentops-server` has its own private `agentops_data_dir` with the same logic instead of calling `agentops_manifest::agentops_data_dir`. The copying is deliberate: decision "AGENTOPS_DATA_DIR consolidates ~10 storage env vars" says to copy the small helper into each crate (ADR 0003). The defect is only that the two model stores never adopted it.
- Gotcha 53947 ("A crate-local duplicate of a Windows-fixed function can silently reintroduce the same bug") describes the risk of that copying pattern.

### F17. The CLI still ships the pre-merge split MCP servers

- Status: confirmed.
- `agentops serve` runs `agentops_mcp::run_stdio` (22 core tools only); `agentops docbrain-serve` runs `docbrain_mcp::run_stdio` (12 docbrain tools only). `agentops connect` registers the unified `agentops-mcp-server` instead. The two subcommands expose subsets of what clients normally get.

### F18. Module docs cite files outside the repo

- Status: confirmed.
- 7 references in 6 files point at `~/Vaults/...`, `~/.claude/plans/...` or `hexagonal-architecture-guide.md`, none of which exist in the repo (e.g. `agentops-core/crates/agentops-linear/src/lib.rs`, `docbrain-core/crates/docbrain-graph/src/lib.rs`, `agentops-core/crates/agentops-sqlite-support/src/lib.rs`). A reader cannot follow them.

### F19. Node kinds the search filter cannot select

- Status: confirmed.
- `NodeKind` has 8 variants including `DocSection` and `ToolOutput` (`agentops-core/crates/agentops-graph/src/lib.rs`). The `kind` enum of `local_semantic_search` and both `parse_node_kind`/`parse_kind` accept only the original 6.
- `EdgeRelation::from_db_str` maps any unknown string to `DependsOn` silently.

### F20. Repo identity is the directory basename

- Status: to verify (Phase 4).
- `repo_name` returns the canonical directory's file name (`agentops-core/crates/agentops-store-open/src/lib.rs`). Two local repos with the same folder name share one repo key when both point `AGENTOPS_DATABASE_URL` at the same Postgres. Hosted connections use `<connection_id>--<tenant>` instead (gotcha 54242).

### F21. Default LLM model id may not exist

- Status: to verify.
- `DEFAULT_MODEL = "claude-sonnet-5"` (`agentops-core/crates/agentops-llm/src/lib.rs:30`). Current Anthropic ids follow the `claude-sonnet-5-5` pattern; if `claude-sonnet-5` is not accepted by the API, `explain_symbol` fails unless `AGENTOPS_ANTHROPIC_MODEL` is set. Confirm with one live call or the model list.

### F22. Doc comments cite commands and binaries that no longer exist

- Status: confirmed.
- `agentops-core/crates/agentops-heavy-consolidate/src/download.rs` says the model download is triggered by "`consolidate_model`/`agentops consolidate --model`". The `Command` enum in `agentops-core/crates/agentops-cli/src/main.rs` has no `Consolidate` variant. LoRA consolidation runs only through the `consolidate_model` MCP tool (stdio, Qdrant configured) and `POST /consolidate/model`. Embedding consolidation runs inside `end_session`.
- Same pattern: the doc comment on `docbrain_mcp::default_db_path` (`docbrain-core/crates/docbrain-mcp/src/lib.rs`) names "this crate's own binary" and a `docbrain-serve-api` CLI command. `docbrain-mcp` has no `[[bin]]`, and `docbrain-serve-api` was folded into `serve-api` (CLI doc comment on `ServeApi`).

### F23. `migrate-graph` drops knowledge-reuse events, with a stale justification

- Status: confirmed (by reading `migrate_graph`, `agentops-core/crates/agentops-cli/src/main.rs:725`).
- Copied: nodes with embeddings, edges, scan history, `doc_page`, `session_usage`, `llm_usage`, `session_report`, tasks and task links. `repo_state` is recomputed.
- `node_versions` is deliberately not copied, with a sound reason: `snapshot_node_version` stamps `valid_from` as "now", so replaying would fabricate timestamps (doc comment on `MigrateGraph`). Not a defect.
- `session_events` is skipped as an "accepted gap". The comment justifies it with "no node_id column", but a later migration added `session_events.node_id` and `event_kind`. Those rows hold the usage dashboard's knowledge-reuse "hits", so more is lost than the comment says.

### F24. Every scan makes a paid LLM call when an API key is set

- Status: confirmed by reading the code; not measured live.
- `scan::persist` always calls `docgen::persist_doc_page` (`agentops-core/crates/agentops-mcp/src/scan.rs`, `src/docgen.rs`). That calls `agentops_llm::group_core_modules` on the default (not cheap) model whenever `AnthropicConfig::from_env` finds `AGENTOPS_ANTHROPIC_API_KEY`. The comment says the default tier was chosen on purpose after a council audit (2026-09-28).
- This contradicts the `agentops-llm` module doc, which says LLM work is "deliberately never called automatically during a scan". It also contradicts the "opt-in, real cost, never on a scan" convention cited in `agentops-heavy-consolidate/src/download.rs`.
- Cost effect: `agentops watch` re-runs `scan_and_persist` on every debounced batch of file changes (500 ms debounce, `src/watch.rs`). Hosted indexing and push-triggered reindex also go through `persist`. Each of these makes one module-labeling call per scan when a key is set.
- Calls are recorded in the spend ledger (`UsageRecorder` persisted to `llm_usage`), so the cost is visible on the usage dashboard.

### F25. Cross-file reference edges are built but unreachable

- Status: confirmed.
- `scan_and_persist_with_lsp` (`agentops-core/crates/agentops-mcp/src/scan.rs`) runs macro expansion **and** `find_cross_file_references`, which adds cross-file `References` edges through LSP `textDocument/references`.
- Its only callers are two tests in the same file. `agentops install --with-lsp` calls `enrich_with_macro_expansions` and then plain `persist` (`agentops-core/crates/agentops-cli/src/main.rs`, `install`), so it never adds cross-file references. No MCP tool calls it either.
- So every `References` edge in real graphs is same-file, as the `EdgeRelation::References` doc comment says. The cross-file feature (commit `5777224`, "Add LSP cross-file references ...") ships dead.
- The doc comment on `scan_and_persist_with_lsp` also cites an `agentops scan --with-lsp` command that does not exist.

### F26. Any push to any branch triggers a full re-clone and reindex

- Status: confirmed by reading the code.
- The `push` arm of the GitHub webhook handler (`agentops-core/crates/agentops-heavy-api/src/github_app_routes.rs`) never reads `ref`. A push to any branch of a GitHub-App-connected repo starts a `JobKind::Reindex` job.
- Every job deletes the existing checkout and clones again from scratch (`run_job`, stage `repository_cloned`, `agentops-core/crates/agentops-heavy-api/src/indexing.rs`). The job then checks out the connection's tracked branch, so feature-branch pushes reindex unchanged content.
- Combined with F24, each such push can also make a paid LLM call.

### F27. Hosted indexing stages 7-8 ignore Postgres (gotcha 54007 still current)

- Status: confirmed.
- Stage `embeddings_generated` and `generate_onboarding_doc` (stage `documentation_generated`) open `agentops_graph::SqliteGraphStore` on `<checkout>/.context/graph.db` directly (`agentops-core/crates/agentops-heavy-api/src/indexing.rs`). Stages 4-6 persist through `agentops_mcp::scan::persist`, which uses `open_store` and honors `AGENTOPS_DATABASE_URL`.
- On a Postgres deployment, stages 7-8 therefore read an empty or stale SQLite file. Qdrant receives no items, and `repo-map.md` describes an empty graph. The jobs still report success.
- Gotcha 54007, "indexing.rs's embeddings stage hardcodes SqliteGraphStore, ignoring AGENTOPS_DATABASE_URL", is still accurate.
- `generate_onboarding_doc` also re-scans the repo, the second full scan in one job.

### F28. Hosted web search cannot find code symbols after normal indexing

- Status: confirmed by reading the code; not verified live.
- The web Search page calls `/local-search` (`apps/web/src/lib/api/repos-api.ts`), proxied to `agentops-heavy-api`'s `dashboard::local_search_json`. That handler is dense-only: it calls `GraphStore::search_similar` (`agentops-core/crates/agentops-heavy-api/src/dashboard.rs`). No lexical or exact signal is used.
- Hosted indexing persists with `with_embeddings: false` (`agentops_mcp::scan::persist(.., false)` in `agentops-core/crates/agentops-heavy-api/src/indexing.rs`). Symbols and notes from that path get no core embeddings; only Qdrant receives vectors, when configured.
- Result: on a hosted repo indexed the normal way, Search returns only nodes embedded some other way, such as notes added with `with_embeddings: true` or a Full-mode `scan_repo` call with `with_embeddings: true`.
- MCP `local_semantic_search` with `mode: "hybrid"` is unaffected, because lexical and exact-name signals need no embeddings.

### F29. A promoted LoRA adapter is never used

- Status: confirmed by reading the code.
- `agentops_heavy_consolidate::consolidate_model` trains, evaluates and promotes a LoRA adapter (`agentops-core/crates/agentops-heavy-consolidate/src/lib.rs`). Nothing outside that crate loads an active adapter: `AdapterStore::load_active_into` is called only for the next run's baseline. No generation path uses it.
- Decision "CLS-inspired cleanup/optimization/local-consolidation plan (Initiatives 0-6) fully shipped" lists the consumer, a local-first `explain_symbol`, as deliberately deferred. Its stated blocker was the paid-tier boundary: open-core `agentops-llm` could not depend on the commercial crate. The open-core merge (`218c3fc`) removed that boundary.
- Effect: a `consolidate_model` run costs 10 to 45+ minutes of CPU (gotcha 53983) and produces a file only the next run reads.
- Not a code defect. It is recorded so the author can decide whether the deferral still stands.

### F30. Hosted note files are deleted on every reindex

- Status: confirmed by reading the code; not verified live.
- On hosted `/mcp`, `path` resolves to the tenant checkout, `checkout_path(..)` (`agentops-core/crates/agentops-heavy-api/src/tenant_repo.rs`). `add_note` writes its Markdown file under `<checkout>/.agentops/notes/` (`agentops-core/crates/agentops-mcp/src/notes.rs`).
- Indexing stage 2 runs `remove_dir_all` on that checkout before every clone (`agentops-core/crates/agentops-heavy-api/src/indexing.rs`). The note files are lost. Nodes and edges in the shared store survive.
- The every-scan re-match in `scan::persist` reads note files only. So a hosted note:
  - is matched to symbols once, at `add_note` time, and never again;
  - never attaches if it was added before the repo's first index, which repeats gotcha 54035 on the hosted path;
  - permanently loses an edge whose symbol is renamed, because the new symbol is never matched.
- `docs/vault-protocol.md` makes hosted `add_note` the only correct path for remote-connected repos, so this applies to every note this repo records.

### F31. Hosted `/mcp` passes `notes_path` through unchecked

- **Security. Status: confirmed by reading the code; not exploited or tested live.** `SECURITY.md` asks for private reporting; keep this entry out of any public commit until it is fixed.
- `handle_tools_call` (`agentops-core/crates/agentops-heavy-api/src/mcp_http.rs`) rewrites only the `path` argument. Its module doc states the rule: a remote caller's path "is never trusted as a literal filesystem path".
- `tool_add_note` and `tool_ingest_notes` also read `notes_path` (`agentops-core/crates/agentops-mcp/src/tools.rs`). `resolve_notes_path` returns an explicit path unchanged (`agentops-core/crates/agentops-notes/src/config.rs`). `add_note`'s schema does not even declare `notes_path`, but the handler reads it.
- Both tools are Advisor-mode, so any authenticated user of any org can call them in the default mode.
- Read risk: `ingest_notes` with `notes_path` set to the shared checkouts directory walks every `*.md` file below it, across every tenant's checkout, into the caller's own graph. The caller can then read them back. `add_note`'s reply prints the caller's own absolute checkout path, which reveals that directory.
- Write risk: `add_note` with `notes_path` creates the directory and writes `<slug>.md` anywhere the server process can write.
- Likely fix direction (not applied): drop or reject `notes_path` on hosted `/mcp`, or resolve it inside the tenant checkout only.

### F32. Changing a note's type leaves a duplicate node

- Status: confirmed by reading the code.
- `add_note` names the file from the title only, so a second `add_note` with the same title and a different `note_type` overwrites the same file.
- The node's natural key includes its kind (`upsert_node`), so the new type creates a second node. The old node keeps its edges. Nothing prunes `vault:` nodes, so it stays until removed by hand.
- Low impact; noted because notes are never deleted by design, so a wrong-kind duplicate is hard to clean up.

### F33. The REST note path does not work for hosted users

- Status: confirmed by reading the code.
- `docs/vault-protocol.md` (Step 1) offers "`POST /tools/add_note` over REST with an API key" as an alternative to MCP for remote-connected repos. `docs/openapi.yaml` describes `apiKeyAuth` on `/tools/*` as a key from `agentops api-key generate` or `POST /auth/api-keys`.
- `/tools/{name}` is `agentops-api`'s route. Its `require_api_key` checks only the instance-wide `AGENTOPS_API_KEY_HASH` (`check_bearer_api_key`, `agentops-core/crates/agentops-security/src/api_key.rs`). A personal key from `POST /auth/api-keys` is rejected.
- Even with the instance key, `path` is used as a literal server path. A connection id is not resolved, unlike on `/mcp`.
- So a hosted user's only working write path is `/mcp`. The docs should drop the REST alternative or describe it as operator-only.

### F34. JSON-RPC parse errors use the wrong code

- Status: confirmed by reading the code. Low impact.
- An unparseable request gets error code `-32603` (internal error) with the message "parse error". JSON-RPC 2.0 defines -32700 for parse errors.
- Same in `agentops-core/crates/agentops-mcp/src/server.rs`, `agentops-core/crates/agentops-mcp-server/src/lib.rs` and `agentops-core/crates/agentops-heavy-api/src/mcp_http.rs`.

### F35. Hosted scraping fetches any URL, including internal ones

- **Security. Status: confirmed by reading the code; not tested live.** Keep out of public commits until fixed.
- `register_library` stores any `docs_url` (`docbrain-core/crates/docbrain-mcp/src/tools.rs`). `scrape_library` then fetches it from the server with `ureq` (`scrape_docs`, `docbrain-core/crates/docbrain-ingest/src/scrape.rs`). There is no check against loopback, private, link-local or container-internal addresses.
- The response is chunked into the tenant's docbrain file, so the caller can read it back with `get_docs`. This is server-side request forgery with the response returned.
- Reachable by any authenticated user on hosted `/mcp` in Advisor mode, since docbrain tools are ungated.
- Related: `max_pages` has no upper bound, and every `background: true` call starts a new thread.

### F36. `sync_docs` ignores the configured docbrain store

- Status: confirmed by reading the code.
- `agentops_mcp::sync_docs(.., None)` opens a hardcoded `~/.agentops/docbrain.db` (`default_docbrain_db_path`, `agentops-core/crates/agentops-mcp/src/sync_docs.rs`). It ignores `AGENTOPS_DOCBRAIN_DB` and `AGENTOPS_DATA_DIR`, unlike `docbrain_mcp::default_db_path`.
- CLI `agentops sync-docs` without `--db` therefore writes discovery results to that file but runs its interactive step against `docbrain_mcp::default_db_path()`. With `AGENTOPS_DATA_DIR` set, those are two different files.
- The hosted Linear auto-kickoff calls it with `None` (`agentops-core/crates/agentops-heavy-api/src/linear_webhook.rs`), so discovered libraries land in a server-global file, not the tenant's.
- Same pattern as F16.

### F37. Hosted REST auth fails open when the instance key is unset

- **Security. Status: confirmed by reading the code. Whether the hosted deployment sets the key is unknown.**
- `check_bearer_api_key` returns OK when `AGENTOPS_API_KEY_HASH` is unset (`agentops-core/crates/agentops-security/src/api_key.rs`); `.env.example` documents this as "unauthenticated".
- On `agentops-server` with accounts enabled, `require_api_key_or_session` falls back to that check. An anonymous request then passes with no `User`. `resolve_tenant` trusts its `tenant` parameter, and `require_session_capability` skips all checks.
- Effect: anonymous read and write access to any tenant's REST routes, plus `/tools` and `/docbrain/*` with literal server paths.
- `agentops-server` neither requires the key when accounts are enabled nor warns at start.
- `deploy/k8s/secret.yaml.example` does not include `AGENTOPS_API_KEY_HASH`, so a Kubernetes deploy that follows the examples gets this behavior.

### F38. Roles do not apply to MCP or curation

- Status: confirmed by reading the code.
- `/mcp` authenticates the caller but checks no capability (`mcp_http.rs`). A `viewer` can call every tool the org's access mode allows, including `add_note`, and in Full mode `scan_repo`, `explain_symbol` and task writes.
- Curation (`POST /repos/{id}/nodes/{node_id}/curation`) checks only `repos.view`, so a viewer can demote or pin gotchas. F12 covers the unused `gotchas.resolve`.
- A personal API key acts in its owner's current tenant on REST routes but in the key's own tenant on `/mcp`. After `POST /me/switch-tenant`, the same key reaches different orgs on the two surfaces.

### F39. No rate limiting on authentication

- Status: confirmed by reading the code.
- No limiter exists on `POST /auth/login`, `POST /auth/login/2fa`, `POST /auth/cli/device/approve` or `POST /auth/signup` (searched `agentops-heavy-api` and `agentops-accounts` for limiter code). Passwords use Argon2, which slows but does not stop online guessing.
- The comment on `USER_CODE_CHARSET` quotes RFC 8628: enough entropy that, "combined with rate-limiting", brute force is infeasible. No rate limit exists. Approving a guessed code needs a logged-in account and would connect someone else's CLI to the guesser's account, so the impact is low.
- A wrong 2FA code leaves the 5-minute challenge token valid for retry (decision "2FA login flow uses separate challenge token, not real session"). Without a limit, a 6-digit code space can be guessed within that window by a fast enough client.

### F40. `.env.example` misdescribes Advisor mode

- Status: confirmed.
- `.env.example` says Full mode "adds write-capable tools like scan_repo/add_note". `add_note` is available in Advisor mode (`ToolSpec.access` in `agentops-core/crates/agentops-mcp/src/tools.rs`).

### F41. PM2 install does not pass `.env` to the web process

- Status: likely, from reading `ecosystem.config.js`; not run.
- `loadDotEnv` merges `.env` into `env`, which goes to `agentops-server`. The `agentops-web` app gets only `PORT`, `HOSTNAME` and `AGENTOPS_HEAVY_API_URL`.
- PM2 children normally inherit the parent's real environment, so variables set in the shell still arrive. Variables set only in `.env`, such as `AGENTOPS_PUBLIC_API_URL`, do not reach the web app under the PM2 method.

### F42. Kubernetes comments suggest scaling out with Postgres

- Status: confirmed.
- `deploy/k8s/deployment.yaml` and `deploy/k8s/pvc.yaml` say `replicas: 1` can be raised once `AGENTOPS_DATABASE_URL` points at Postgres.
- Postgres replaces only the code graph. Accounts, teams, integrations, connections, indexing jobs and docbrain stay in SQLite files on the pod's volume, and repo checkouts are local files ([03-data-model.md](03-data-model.md), "Platform stores"). Two replicas would each see different data.

### F43. `Caddyfile.example` routes only the exact root to the marketing site

- Status: likely, from Caddy's matcher rules; not run.
- `reverse_proxy / marketing:4321` uses the path matcher `/`, which in Caddy matches only the exact path `/`. `/pricing`, `/features/*` and built assets would not reach the marketing service. `/*` would.
- `deploy/k8s/ingress.yaml` uses its own path rules and is not affected.

### F44. Note search results carry no node id

- Status: confirmed live on 2026-10-13 against the hosted server.
- `local_semantic_search` returns each hit's kind, title, score and `vault:` path, but no node id. `fetch_content` needs a node id, so a decision found by search cannot be read in full over MCP.
- Effect: decisions recorded only on the hosted graph (for example "Small local models (≤4GB): recommended for the cheap LLM tier only, never bundled") could not be used as ADR sources.

## Stale recorded knowledge

These gotchas on the hosted graph no longer match the code. None is used as a source for the docs.

Gotchas are never resolved or deleted (decision "Gotchas are curated, never resolved/dismissed -- don't reintroduce a bug-tracker model"). The matching action is "Reduce prominence" with the reason from the table below. Done on 2026-10-13 after the author approved the docs, through `POST /repos/{id}/nodes/{node_id}/curation`; `list_gotchas` shows all seven as reduced.

| Gotcha | Title | Why stale |
| --- | --- | --- |
| 53957 | agentops-heavy is a separate Cargo workspace, invisible to the root deny.toml | No `agentops-heavy/` directory exists; heavy crates are root workspace members (`Cargo.toml`). |
| 53979 | install.sh's GitHub raw-content download 404s because Jesus-Glez60/agentops is a private repo | `gh repo view` reports visibility `PUBLIC` (2026-10-12). |
| 53951 | agentops-embeddings crate naming collision between agentops-core and agentops-heavy | Crates are now `agentops-embeddings` and `agentops-heavy-embeddings`. |
| 53956 | agentops-heavy-embeddings was renamed but never actually ported | Crate has 544 lines of source and is a dependency of 3 crates. **To verify**: what "ported" meant. |
| 53908 | agentops-server crash-loops and gives up if Postgres isn't ready yet on cold container start — needs a fix | Fixed in `cf408a4` (2026-10-07): `open_shared_postgres_store` retries with jittered backoff for `AGENTOPS_PG_CONNECT_RETRY_SECS` (default 60). |
| 54151 | delete_organization never wipes agentops-graph-pg data for the tenant's connections | `delete_organization` now calls `indexing::wipe_connection_graph` per connection (`agentops-core/crates/agentops-heavy-api/src/team.rs`). See F10. |
| 54023 | The /mcp HTTP endpoint never adopted the shared-Postgres-pool fix | `handle_tools_call` now wraps every call in `agentops_mcp::with_shared_postgres_store` (`agentops-core/crates/agentops-heavy-api/src/mcp_http.rs`). |

## Resolved during the docs work

### F10. Org deletion leaves Postgres code-graph data behind: no longer true

- Status: resolved in code before these docs were written; checked in Phase 5.
- `delete_organization` (`agentops-core/crates/agentops-heavy-api/src/team.rs`) calls `indexing::wipe_connection_graph` for each connection, which runs `PostgresGraphStore::wipe_repo`. Gotcha 54151 ("delete_organization never wipes agentops-graph-pg data for the tenant's connections") is therefore stale and is listed under "Stale recorded knowledge".
- Per-repo `.context/graph.db` files on developers' machines remain out of scope, as decision "Org deletion is a full cascade — audit_log excluded, codebrain graph out of scope" states.

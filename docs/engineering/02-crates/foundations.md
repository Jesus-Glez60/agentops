# Crates: foundations

The nine crates with no internal dependencies. Everything else builds on them; read an entry here before changing redaction, SQLite plumbing, embeddings, repo credentials or external clients.

Index and field definitions: [02-crate-reference.md](../02-crate-reference.md).

## agentops-security

- **Path**: `agentops-core/crates/agentops-security`
- **Purpose**: Secret redaction, secret-bearing filename exclusion, and injection-aware framing of repo content. Also API-key generation and verification for the REST servers.
- **Entry points**:
  - `redact(text) -> RedactionResult`: replaces credential-shaped strings with `[REDACTED:<rule>]` markers.
  - `is_secret_bearing_filename(path)`: files the scanner must skip.
  - `wrap_repo_content(source_label, content)`: frames untrusted repo text before it reaches a prompt or document.
  - `api_key` module: `generate_api_key`, `hash_api_key`, `verify_api_key`, `check_bearer_api_key`, `generate_master_key`.
- **Depends on**: none.
- **Used by**: `accounts`, `api`, `cli`, `heavy-api`, `manifest`, `scanner`, `teams`, `docbrain-api`.
- **Invariants**:
  - No networking dependency. It is linked into `agentops-scanner`, which must keep zero runtime network egress (module doc; `deny.toml`).
  - API keys are stored only as hashes; the raw key is shown once (`agentops api-key generate`).
- **Known history**: gotcha 54063, "agentops_security::redact's generic-credential-assignment rule required quotes, missing unquoted env-dump style secrets".
- **Tests**: 13 (`src/lib.rs` 8, `src/api_key.rs` 5).

## agentops-sqlite-support

- **Path**: `agentops-core/crates/agentops-sqlite-support`
- **Purpose**: Shared SQLite open/migrate helpers and a generic natural-key upsert, so `agentops-graph` and `docbrain-graph` stop re-copying this boilerplate.
- **Entry points**: `open(path, migrations)`, `open_in_memory(migrations)`, `upsert(conn, table, cols, key_cols, values) -> i64`.
- **Depends on**: none.
- **Used by**: `graph`, `docbrain-graph`.
- **Invariants**: `upsert` is the only natural-key write path. The module doc records why: `docbrain-graph` previously relied on a `UNIQUE` constraint plus a check-then-act race.
- **Tests**: 3 (`src/lib.rs`).

## agentops-embeddings

- **Path**: `agentops-core/crates/agentops-embeddings`
- **Purpose**: Local embedding generation through `fastembed`; no external service.
- **Entry points**: `Embedder` (port trait), `LocalEmbedder` (the only adapter), `EMBEDDING_DIM = 384`.
- **Depends on**: none.
- **Used by**: 12 crates, including `graph`, `graph-pg`, `retrieval`, `mcp`, `docbrain-graph`, `docbrain-ingest`.
- **Invariants**:
  - `EMBEDDING_DIM` is the single source of truth for vector width: `node_vectors` (sqlite-vec), the Postgres `vector(384)` column and docbrain's vector table all size against it.
  - Use cases take `&dyn Embedder`, never `LocalEmbedder` or `fastembed` directly.
  - The model is loaded once per process. Its Hugging Face download on first use is the one allowed network call, scoped in `deny.toml`.
- **Tests**: 2 (`src/lib.rs`).

## agentops-repo-access

- **Path**: `agentops-core/crates/agentops-repo-access`
- **Purpose**: Credentials and git operations for hosted repo access. It covers per-repo SSH deploy keys, HTTPS clones with a GitHub App token, the connection store and the indexing-job store.
- **Entry points**:
  - Keys and git: `generate_deploy_keypair_for_repo`, `UnlockedKey`, `clone_repo`, `clone_repo_https`, `fetch_repo`, `push_repo`, `list_remote_branches`, `checkout_branch`, `GITHUB_KNOWN_HOSTS`.
  - `store` module: `ConnectionStore`, `RepoConnection`, `ConnectionMethod`, `ConnectionStatus`, `is_local_only_url`, `new_local_repo_id`, `LOCAL_ONLY_URL_PREFIX = "local:"`.
  - `indexing_store` module: `IndexingStore`, `IndexingJob`, `JobStage`, `JobStatus`, `STAGE_ORDER` (9 stages), `GitHubAppInstallation`.
  - `secrets` module: `SecretsProvider` (port), `EnvSecretsProvider`.
- **Depends on**: none.
- **Used by**: `accounts`, `cli`, `heavy-api`, `integrations`, `manifest`.
- **Invariants** (module doc):
  - One dedicated keypair per repo, never shared, so a leaked key exposes exactly one repo.
  - A private key is held unencrypted only while one git subprocess runs. It sits in a `0600` temp file that `UnlockedKey`'s `Drop` overwrites and deletes.
  - `SecretsProvider` derives every per-tenant key from `AGENTOPS_SECRETS_MASTER_KEY`. The module doc says this is not yet KMS-backed.
- **Known history**: gotchas 54093 ("SSH deploy-key isolation leaked through the caller's own ~/.ssh/config"), 54216 (`repo_connections` has no migration framework), 54008 (indexing enums serialized as PascalCase).
- **Tests**: 60 (`src/lib.rs` 17, `src/store.rs` 27, `src/secrets.rs` 9, `src/indexing_store.rs` 7).

## agentops-agents-md

- **Path**: `agentops-core/crates/agentops-agents-md`
- **Purpose**: Generates a vendor-neutral `AGENTS.md` for a repo, including a `NOTES_PATH` marker that tells agents where new notes go.
- **Entry points**: `generate(root, &GenerateOptions) -> String`, `GenerateOptions`.
- **Depends on**: none.
- **Used by**: `mcp` (`init.rs`, behind `init_agents_md` and `agentops install`).
- **Invariants**:
  - Plain Markdown only, no `@include` directives, because agents other than Claude Code do not resolve them.
  - The caller resolves the notes path (`agentops_notes::resolve_notes_path`) and passes it in, so this crate needs no dependency on `agentops-notes`.
  - Generation fully overwrites `AGENTS.md`/`CLAUDE.md` (gotcha 54112).
- **Tests**: 10 (`src/lib.rs`).

## agentops-ruler-bridge

- **Path**: `agentops-core/crates/agentops-ruler-bridge`
- **Purpose**: Builds a `.ruler/` directory from the embedded prompt pack. It then runs a pinned `@intellectronica/ruler` to copy instructions, skills and subagents into each selected coding tool.
- **Entry points**: `RULER_VERSION = "0.3.44"`, `build_ruler_dir`, `apply(target_repo, agent_ids, dry_run, with_mcp)`, `preflight_check_npx`, `preflight_check_mcp_server_binary`.
- **Depends on**: none (shells out to `npx`).
- **Used by**: `cli`.
- **Invariants** (module doc, verified against Ruler 0.3.44):
  - The bridge resolves `@include` directives itself, because Ruler does not.
  - Skills must be laid out as `.ruler/skills/<name>/SKILL.md`; a flat `.md` is not recognized.
- **Known history**: gotchas 54073 (deprecated `.ruler/mcp.json` fallback), 54074 (unquoted colon in YAML description), 54118 (prompt-pack edits need a forced rebuild), 54122 (running `ruler apply` directly touches every agent).
- **Tests**: 2 (`src/lib.rs`).

## agentops-lsp-client

- **Path**: `agentops-core/crates/agentops-lsp-client`
- **Purpose**: A narrow, best-effort client for one `rust-analyzer` process. Its real value is `expand_macro`: the expanded source of a `macro_rules!` invocation, fed back through the normal symbol extractor.
- **Entry points**: `LspClient` (methods include document symbols, references and macro expansion), `LspSymbol`, `LspLocation`.
- **Depends on**: none.
- **Used by**: `mcp` (`enrich_with_macro_expansions`, behind `agentops install --with-lsp`).
- **Invariants** (module doc):
  - Every failure is a normal outcome: missing binary, a workspace that does not build, timeouts, non-expandable positions. None may fail or block a scan.
  - `#[derive(...)]` macros cannot be expanded this way (confirmed live); only `macro_rules!`-style invocations.
  - `rust-analyzer` can reject requests with `ContentModified`. Commit `8565155` retries those.
- **Known history**: gotchas 53904 (fixture projects need their own `[workspace]`), 54116 (`references` direction inverted relative to same-file references), 53903 (pilot's original motivation mostly redundant).
- **Tests**: 6 (`src/lib.rs`, 1 `#[ignore]`); fixtures in `tests/fixtures/`.

## agentops-github-app

- **Path**: `agentops-core/crates/agentops-github-app`
- **Purpose**: GitHub App authentication: RS256 app JWTs and installation access tokens. The recommended repo-access path, since it needs no per-repo private key custody.
- **Entry points**: `generate_app_jwt`, `get_installation_token`, `get_installation_details`, `list_installation_repos`, `list_repo_branches`, `install_url`, `installation_html_url`.
- **Depends on**: none.
- **Used by**: `heavy-api`.
- **Invariants**:
  - RS256 is mandated by GitHub, so the `rsa` advisory `RUSTSEC-2023-0071` is accepted in `deny.toml`. The exposure is narrow: the process signs its own JWTs and is not a signing oracle. Decision: "RUSTSEC-2023-0071 (RSA Marvin Attack timing side-channel) accepted, not fixed -- required for GitHub App RS256".
  - Tests verify the token exchange against `wiremock`, not GitHub's live API (module doc).
- **Tests**: 6 (`src/lib.rs`).

## agentops-integration-modules

- **Path**: `agentops-core/crates/agentops-integration-modules`
- **Purpose**: Per-tenant enrollment for integration modules: an on/off switch plus non-secret config. Linear auto-kickoff is the first consumer.
- **Entry points**: `ModuleStore`, `ModuleEnrollment`.
- **Depends on**: none.
- **Used by**: `heavy-api`.
- **Invariants** (module doc):
  - Holds non-secret operational config only. Secrets belong in `agentops-integrations`.
  - `config` is an opaque JSON blob, so a new module needs no schema change.
  - One shared SQLite file with a `tenant` column, unlike docbrain's one-file-per-tenant pattern.
- **Decision**: "Phase 6c: auto-kickoff moved from boot-time config to per-account opt-in via agentops-integration-modules".
- **Tests**: 7 (`src/lib.rs`).

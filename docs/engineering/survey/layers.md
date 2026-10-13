# Survey: crate layers (Phase 1)

Verifies the layer table in `BRIEF.md` against the measured internal dependency graph. Input for `01-architecture.md` and `02-crate-reference.md`; not a final document.

Source: `docs/engineering/scripts/crate-deps.sh` (reads `[dependencies]` of every workspace member in the root `Cargo.toml`). Measured 2026-10-12 on `main` @ `8565155`. No internal dependency is `optional`, so feature flags do not change the graph.

## Counts

- 36 workspace members: 32 under `agentops-core/crates/`, 4 under `docbrain-core/crates/`. Matches the brief.
- 3 binary targets: `agentops` (`agentops-core/crates/agentops-cli`), `agentops-server` (`agentops-core/crates/agentops-server`), `agentops-mcp-server` (`agentops-core/crates/agentops-mcp-server`). Matches the brief.
- No internal crate appears only under `[dev-dependencies]`.

## Measured dependency depth

Depth = longest path to a crate with no internal deps. "Used by" = number of workspace crates depending on it directly.

| Depth | Crate | Internal deps | Used by |
| --- | --- | --- | --- |
| 0 | `agentops-agents-md` | — | 1 |
| 0 | `agentops-embeddings` | — | 12 |
| 0 | `agentops-github-app` | — | 1 |
| 0 | `agentops-integration-modules` | — | 1 |
| 0 | `agentops-lsp-client` | — | 1 |
| 0 | `agentops-repo-access` | — | 5 |
| 0 | `agentops-ruler-bridge` | — | 1 |
| 0 | `agentops-security` | — | 8 |
| 0 | `agentops-sqlite-support` | — | 2 |
| 1 | `agentops-accounts` | `repo-access`, `security` | 1 |
| 1 | `agentops-graph` | `embeddings`, `sqlite-support` | 15 |
| 1 | `agentops-integrations` | `repo-access` | 1 |
| 1 | `agentops-manifest` | `repo-access`, `security` | 5 |
| 1 | `agentops-scanner` | `security` | 5 |
| 1 | `agentops-teams` | `security` | 1 |
| 1 | `docbrain-graph` | `agentops-embeddings`, `agentops-sqlite-support` | 10 |
| 2 | `agentops-docgen` | `graph` | 3 |
| 2 | `agentops-embeddings-train` | `embeddings`, `graph` | 1 |
| 2 | `agentops-graph-pg` | `embeddings`, `graph` | 3 |
| 2 | `agentops-heavy-embeddings` | `graph`, `docbrain-graph` | 3 |
| 2 | `agentops-linear` | `graph` | 2 |
| 2 | `agentops-notes` | `graph` | 3 |
| 2 | `agentops-retrieval` | `embeddings`, `graph` | 4 |
| 2 | `docbrain-ingest` | `agentops-embeddings`, `docbrain-graph` | 3 |
| 3 | `agentops-heavy-consolidate` | `embeddings`, `graph`, `retrieval` | 2 |
| 3 | `agentops-llm` | `embeddings`, `graph`, `notes`, `retrieval`, `scanner` | 3 |
| 3 | `agentops-store-open` | `graph`, `graph-pg` | 2 |
| 3 | `docbrain-mcp` | `agentops-manifest`, `docbrain-graph`, `docbrain-ingest` | 5 |
| 4 | `agentops-heavy-mcp` | `graph`, `heavy-consolidate`, `heavy-embeddings`, `store-open`, `docbrain-graph` | 2 |
| 4 | `agentops-mcp` | `agents-md`, `docgen`, `embeddings`, `embeddings-train`, `graph`, `graph-pg`, `llm`, `lsp-client`, `notes`, `retrieval`, `scanner`, `store-open`, `docbrain-graph`, `docbrain-ingest` | 5 |
| 4 | `docbrain-api` | `agentops-security`, `docbrain-graph`, `docbrain-mcp` | 1 |
| 5 | `agentops-api` | `docgen`, `embeddings`, `graph`, `manifest`, `mcp`, `scanner`, `security` | 2 |
| 5 | `agentops-mcp-server` | `heavy-embeddings`, `heavy-mcp`, `mcp`, `docbrain-graph`, `docbrain-mcp` | 0 |
| 6 | `agentops-heavy-api` | `accounts`, `api`, `docgen`, `embeddings`, `github-app`, `graph`, `heavy-consolidate`, `heavy-embeddings`, `heavy-mcp`, `integration-modules`, `integrations`, `linear`, `llm`, `manifest`, `mcp`, `repo-access`, `scanner`, `security`, `teams`, `docbrain-graph`, `docbrain-mcp` | 1 |
| 7 | `agentops-server` | `api`, `heavy-api`, `manifest`, `mcp`, `docbrain-api`, `docbrain-graph`, `docbrain-mcp` | 1 |
| 8 | `agentops-cli` | `embeddings`, `graph`, `graph-pg`, `linear`, `llm`, `manifest`, `mcp`, `notes`, `repo-access`, `retrieval`, `ruler-bridge`, `scanner`, `security`, `server`, `docbrain-graph`, `docbrain-ingest`, `docbrain-mcp` | 0 |

`agentops-` prefixes are dropped inside the deps column for `agentops-*` crates; `docbrain-*` names are kept whole.

## Brief's layer table: verdict per row

The brief says its grouping "comes from each crate's internal dependencies". Only the first row does. The other rows are functional groupings, which is fine, but they are not dependency layers.

| Brief row | Verdict | Evidence |
| --- | --- | --- |
| Foundations | **Correct.** The 9 crates listed are exactly the 9 depth-0 crates. | Table above |
| Graph and storage | Membership fine. `agentops-graph` is not storage-only: it depends on `agentops-embeddings`. `store-open` sits at depth 3, above `scanner`/`docgen`. | `agentops-core/crates/agentops-graph/Cargo.toml` |
| Indexing | `agentops-manifest` does not depend on the graph at all (`repo-access`, `security` only). It is a repo-registry crate, not an indexing stage. | `agentops-core/crates/agentops-manifest/Cargo.toml` |
| Knowledge and retrieval | `agentops-linear` depends only on `agentops-graph`. It is a task-sync integration, not retrieval. `agentops-llm` also pulls in `scanner`. | `agentops-core/crates/agentops-linear/Cargo.toml`, `agentops-core/crates/agentops-llm/Cargo.toml` |
| docbrain | Incomplete. Besides `embeddings` and `sqlite-support`, `docbrain-mcp` depends on `agentops-manifest` and `docbrain-api` on `agentops-security`. | `docbrain-core/crates/docbrain-mcp/Cargo.toml`, `docbrain-core/crates/docbrain-api/Cargo.toml` |
| Hosted / multi-tenant | **Wrong for the heavy crates.** The local stdio binary `agentops-mcp-server` depends on `agentops-heavy-embeddings` and `agentops-heavy-mcp`. "heavy" is therefore not hosted-only. `agentops-heavy-embeddings` also depends on `docbrain-graph`. | `agentops-core/crates/agentops-mcp-server/Cargo.toml` |
| Transports | `agentops-mcp` holds the use-case logic and the tool table, not only transport. `agentops-api` sits above it (depends on it). | `agentops-core/crates/agentops-api/Cargo.toml` |
| Binaries | `agentops-server` is also a library: `agentops-cli` depends on it. `agentops-server` links `agentops-heavy-api`, so every server build includes all heavy crates. | `agentops-core/crates/agentops-cli/Cargo.toml`, `agentops-core/crates/agentops-server/Cargo.toml` |

## Facts the later docs need

- `agentops_server::run` is documented as "the one entry point both this crate's own binary and `agentops-cli`'s `serve-api` subcommand call" (`agentops-core/crates/agentops-server/src/lib.rs`). This explains the `cli -> server` edge; see `questions.md` Q1.
- `agentops-graph` (15 dependents), `agentops-embeddings` (12) and `docbrain-graph` (10) are the most depended-on crates. A breaking change there touches most of the workspace.
- Decision note "agentops-retrieval must never depend on agentops-embeddings-train directly -- EmbeddingProjector is a plain trait for exactly this reason" holds today: `agentops-retrieval` deps are `embeddings`, `graph` only.

## Open items

See `questions.md`: Q1 (cli -> server), Q2 (meaning of "heavy"), Q4 (proposed layer names).

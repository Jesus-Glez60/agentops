# Crate reference

One entry per workspace crate: what it is for, its public entry points, what it depends on, its invariants and its tests. Read the entry for a crate before changing it; [01-architecture.md](01-architecture.md) explains how the crates fit together.

## How to read this reference

The 36 crates are split into nine files by functional group. Groups follow the approved layers in `01-architecture.md`, merged where a layer has one or two crates.

| File | Crates |
| --- | --- |
| [02-crates/foundations.md](02-crates/foundations.md) | `agentops-security`, `agentops-sqlite-support`, `agentops-embeddings`, `agentops-repo-access`, `agentops-agents-md`, `agentops-ruler-bridge`, `agentops-lsp-client`, `agentops-github-app`, `agentops-integration-modules` |
| [02-crates/graph-and-storage.md](02-crates/graph-and-storage.md) | `agentops-graph`, `agentops-graph-pg`, `agentops-store-open` |
| [02-crates/indexing.md](02-crates/indexing.md) | `agentops-scanner`, `agentops-docgen`, `agentops-manifest` |
| [02-crates/knowledge-and-retrieval.md](02-crates/knowledge-and-retrieval.md) | `agentops-retrieval`, `agentops-notes`, `agentops-llm`, `agentops-embeddings-train` |
| [02-crates/semantic-index-and-consolidation.md](02-crates/semantic-index-and-consolidation.md) | `agentops-heavy-embeddings`, `agentops-heavy-consolidate`, `agentops-heavy-mcp` |
| [02-crates/docbrain.md](02-crates/docbrain.md) | `docbrain-graph`, `docbrain-ingest`, `docbrain-mcp`, `docbrain-api` |
| [02-crates/accounts-teams-integrations.md](02-crates/accounts-teams-integrations.md) | `agentops-accounts`, `agentops-teams`, `agentops-integrations`, `agentops-linear` |
| [02-crates/use-cases-and-transports.md](02-crates/use-cases-and-transports.md) | `agentops-mcp`, `agentops-api`, `agentops-heavy-api` |
| [02-crates/binaries.md](02-crates/binaries.md) | `agentops-server`, `agentops-mcp-server`, `agentops-cli` |

`agentops-manifest` (the "Repo registry" layer) sits with indexing because it records what has been scanned.

## Entry template

Every crate entry has the same fields:

- **Path**: crate directory.
- **Purpose**: from the crate's own module doc (`//!` in `src/lib.rs`).
- **Entry points**: the main `pub` items. Not exhaustive; `cargo doc` is.
- **Depends on / Used by**: direct internal edges, measured by `scripts/crate-deps.sh`. `agentops-` prefixes are dropped in these lists.
- **Invariants**: rules a change must keep, with their source.
- **Tests**: test count per file (`#[test]` and `#[tokio::test]`), measured 2026-10-12 on `main` @ `8565155`. Tests live in inline `#[cfg(test)]` modules unless stated.

## Size at a glance

| Crate | Lines (src) | Tests |
| --- | --- | --- |
| `agentops-heavy-api` | 11252 | 177 |
| `agentops-cli` | 5514 | 65 |
| `agentops-mcp` | 5432 | 127 |
| `agentops-graph` | 3847 | 76 |
| `agentops-api` | 3207 | 79 |
| `docbrain-ingest` | 2391 | 61 |
| `agentops-scanner` | 2338 | 68 |
| `agentops-repo-access` | 2296 | 60 |
| `agentops-llm` | 2008 | 47 |
| `agentops-graph-pg` | 1897 | 27 |
| `agentops-accounts` | 1706 | 39 |
| `agentops-teams` | 1403 | 50 |
| All 36 crates | 56085 | 1135 |

The other 24 crates are under 1,100 lines each.

## Tests that need live services

Some tests skip silently or are `#[ignore]` unless a service is reachable. A green `cargo test` does not prove they ran.

| Requirement | Crates |
| --- | --- |
| `AGENTOPS_TEST_DATABASE_URL` (live Postgres) | `agentops-graph-pg`, `agentops-store-open`, `agentops-mcp` (`scan.rs`), `agentops-api` (`search.rs`), `agentops-heavy-api` (`lib.rs`, `team.rs`), `agentops-cli` |
| `AGENTOPS_TEST_QDRANT_URL` (live Qdrant) | `agentops-heavy-embeddings`, `agentops-heavy-mcp`, `agentops-heavy-api` |
| `#[ignore]` (model downloads, long runs, `rust-analyzer`) | `agentops-heavy-consolidate` (5), `agentops-heavy-api` (6), `agentops-mcp` (5), `agentops-api` (3), `agentops-store-open` (1), `agentops-lsp-client` (1) |

Gotcha 54247: "Postgres-backed tests skip silently unless AGENTOPS_TEST_DATABASE_URL reaches a live DB".

## Coverage check

Every workspace member must appear in this reference:

```sh
docs/engineering/scripts/crate-deps.sh --check docs/engineering/02-crates/*.md
```

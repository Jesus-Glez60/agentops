# 0012. Database-native multi-signal retrieval

Survey candidate R2. Rationale is taken only from the sources under "Evidence"; nothing here is inferred.

## Status

Accepted, 2026-08-11 (`67cedae`).

## Context

The original design assumed a separate tantivy index over a Qdrant dense index. Dense search had since moved into core (`search_similar`).

## Decision

Add `search_lexical` (SQLite FTS5 external-content table with triggers; Postgres generated `tsvector`) and `search_exact` to `GraphStore`. Fuse them with dense results by reciprocal rank fusion in a new `agentops-retrieval` crate, exposed as `mode: "hybrid"`.

## Alternatives considered

- A separate tantivy index. Rejected: database-native indexes stay in sync automatically, with no second index to maintain.

## Consequences

- Hybrid search finds un-embedded symbols by name, which dense search misses (live-verified with `reinforce_edge`).
- An FTS5 backfill with a self-referential guard silently corrupted the index; the guard was removed.
- Fusion weights and constants are described in `05-retrieval.md`.

## Evidence

- Decision "Phase 4 multi-signal retrieval redesigned against current architecture, live-verified".
- Module doc of `agentops-core/crates/agentops-retrieval/src/lib.rs`.

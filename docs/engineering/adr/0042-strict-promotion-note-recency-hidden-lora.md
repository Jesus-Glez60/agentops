# 0042. Strict promotion, note-only recency, and LoRA consolidation hidden

Amends ADR 0013. Rationale comes from the author's answers A-9, A-14 and A-15 in `questions.md`.

## Status

Accepted, 2026-10-13.

## Context

ADR 0013 left three questions open: whether a tie should promote a consolidated model, whether node recency should reuse the 30-day `Affects` half-life, and whether the LoRA consumer was still deferred. `consolidate_model` trained an adapter nothing used (`findings.md` F29).

## Decision

1. Both consolidation gates promote only on strict improvement (`candidate > baseline`, `should_promote`). A tie keeps the active model.
2. Recency decay in hybrid ranking applies to gotchas, decisions and notes only. Code is never ranked down for being stable.
3. `consolidate_model` and `POST /consolidate/model` are not exposed until their consumer, a local-first `explain_symbol`, exists. The `agentops-heavy-consolidate` crate stays.

## Alternatives considered

- A separate, longer half-life for code nodes. Not chosen.
- Deleting LoRA consolidation. Not chosen; it waits for its consumer.

## Consequences

- A first embedding-consolidation run promotes only if it beats the raw embedding space.
- Code ranking no longer differs between SQLite (which records `last_touched_at`) and Postgres (which does not).
- Re-exposing `consolidate_model` belongs with the work that uses its adapter.

## Evidence

- Author answers A-9, A-14, A-15, `questions.md`.
- `should_promote` (`agentops-core/crates/agentops-embeddings-train/src/lib.rs`, `agentops-core/crates/agentops-heavy-consolidate/src/lib.rs`); `recency_multiplier` (`agentops-core/crates/agentops-retrieval/src/lib.rs`); module doc of `agentops-core/crates/agentops-heavy-mcp/src/tools.rs`.

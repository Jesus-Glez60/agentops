# 0015. Weighted decaying edges and automatic note re-matching

Survey candidate R5. Rationale is taken only from the sources under "Evidence"; nothing here is inferred.

## Status

Accepted, 2026-08-10 (`7a7d798`).

## Context

Notes written before the first scan never attached to symbols (gotcha 54035). Every relationship counted the same however often it was confirmed.

## Decision

1. `Affects` edges carry a weight: 1.0, +0.5 per reinforcement, capped at 5.0. It decays at read time with a 30-day half-life, computed in Rust for both backends.
2. `persist` re-matches existing notes against current symbols on every scan.
3. A `repo_state` table keeps a top-5 gotchas and decisions snapshot.
4. The SQLite graph schema moved to `rusqlite_migration`.

## Alternatives considered

- Require a manual `ingest-notes` after each scan. Replaced by automatic re-matching.

## Consequences

- Automatic re-matching must not move `updated_at`, or staleness flags break (gotcha 53965).
- Every re-match still raises the weight.
- Hosted notes are not re-matched, because their files do not survive reindexing (`findings.md` F30).

## Evidence

- Decision "weighted decaying edges, auto note re-matching, and repo state added".
- Commit `7a7d798`.

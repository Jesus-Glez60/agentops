# 0051. Similarity edges between memories

Milestone 2 of the cue-driven memory work (MEMORY.md import), 2026-10-16. Approved with the milestone 2 plan; the parked "similarity edges" idea from the requirement, reviewed against kb-mcp-server on 2026-10-14.

## Status

Accepted, 2026-10-16.

## Context

Every AgentOps edge is structural or explicit: `DependsOn`, `Documents`, `Affects`, `References`, `Covers`. A memory that names no symbol has no edge, so recall that starts from a cue (a prompt keyword, a named symbol) can never reach it unless it shares a keyword. Imported MEMORY.md entries and past-session notes are mostly like that.

Hooks must stay free of embedding work: each hook is a fresh process with a tight latency budget (milestone 1: p95 under 300 ms local, under 1 s remote).

## Decision

Add a plastic `EdgeRelation::SimilarTo` between memories (gotcha, decision, note), stored in both directions.

- **Computed at write time only** (`agentops_graph::similarity::link_similar`), where the embedding already exists: `add_note` with embeddings, `ingest_notes`, the importer, the scan's note-embedding pass, and the `agentops link-similar` backfill.
- **Top `SIMILAR_K = 5` above `SIMILAR_MIN_SCORE = 0.75` cosine**, computed in Rust from stored vectors so SQLite (L2 KNN) and Postgres (cosine KNN) agree. At most `SIMILAR_MAX_DEGREE = 12` per memory; the weakest pair goes first.
- **Doc sections are excluded**: they're generated digests and already the main source of recall noise (F53).
- **Reinforced, not reset**, on an explicit re-write; only created, never reinforced, on a rescan, which re-reads every note. Decays with a 90-day half-life, like `References`.
- **Followed, never computed, in hooks**: `recall_seed` takes one hop from each memory that qualified on its own cues. The neighbour's reason is `similar to "<title>"`.

## Alternatives considered

- Semantic search inside the hook. Not chosen: it loads the ONNX embedding model in every hook process, which blows the latency budget.
- Adopt kb-mcp-server, which builds its whole graph from embedding similarity. Not chosen: it's generic, has no code awareness, and was last updated in May 2025. Only the idea was taken.
- Similarity edges for code symbols too. Not chosen: ADR 0017 keeps code structure explicit, and symbols are reached by name.

## Consequences

- The thresholds are starting values. The milestone 2 pilot reports the candidate score distribution (`agentops link-similar` prints it) and whether they hold.
- A hosted reindex fills in missing edges for every embedded memory, so it doubles as a backfill.
- Postgres needed `edges_relation_check` widened (gotcha 54053).

## Amendment, 2026-10-20 (milestone 3a item 3)

Neighbour scoring changed. A neighbour now inherits its source's cue count and scores `source score × cosine × 0.8`, instead of counting as one cue at half its source's score. Under the old rule, seeds full of 2-to-5-cue keyword hits always cut the neighbours (`m2-report.md` §5). The cosine comes from stored vectors, because edges keep no score. The hop is still one step, still read-only, and still free of any embedding model.

## Evidence

- `agentops-core/crates/agentops-graph/src/similarity.rs` and its tests (at most K both ways, never a doc section, degree cap, reinforce vs rescan).
- `recall_seed` test `a_similar_to_hop_reaches_a_memory_with_no_keyword_and_no_symbol_and_says_why`.
- Postgres test `similar_to_edges_round_trip_and_link_similar_works_on_postgres`.

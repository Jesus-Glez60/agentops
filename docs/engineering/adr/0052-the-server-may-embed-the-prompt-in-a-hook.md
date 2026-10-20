# 0052. The server may embed the prompt in a hook

Milestone 3a of the cue-driven memory work (recall quality), 2026-10-20. Decided in the milestone 3a spec; this record amends ADR 0051's "hooks must stay free of embedding work".

## Status

Accepted, 2026-10-20. Amends [0051](0051-similarity-edges-between-memories.md) (Context, and the rejected "semantic search inside the hook").

## Context

Milestone 2's recall check got 2 of 5. Two of its prompts share no word with their memory, so keyword recall can't find them. A `SimilarTo` hop could only bridge that gap from a memory that had already matched (`m2-report.md` §5). The rule against embedding inside a hook was about loading the ONNX model in a fresh local process on every call. Remote hooks never run that way: `agentops hook-capture` forwards them to `POST /repos/{id}/hooks/{event}`, which is served by the long-running server process, where `LocalEmbedder` keeps its model loaded.

## Decision

- **Server path only.** `hooks::handle_with(.., Some(embedder))` is called by the hosted hook route and the eval's `POST /repos/{id}/recall`. The local `agentops hook-capture` path calls `hooks::handle`, which passes no embedder, so a local hook process never loads the model. That is tested in `agentops-mcp/tests/hooks_never_load_the_model.rs` via `agentops_embeddings::model_loaded()`.
- **`recall_seed_with`** (`agentops-retrieval/src/recall.rs`) runs these steps:
  1. Embeds the prompt on a helper thread while the keyword queries run.
  2. Takes the `DENSE_TOP_K = 20` nearest memories with cosine ≥ `DENSE_MIN_SCORE = 0.55`.
  3. Fuses them with the keyword ranking by reciprocal rank (k = 60).
  4. Applies the `SimilarTo` hop from the fused order.

  With no embedder it is exactly `recall_seed`.
- **Latency guard.** If embedding plus the nearest-neighbour search exceeds `DENSE_TIMEOUT_MS = 250`, the dense half is dropped and the keyword result is returned. Each occurrence is logged as `dense recall timeout`. A stalled embed is abandoned at the deadline. A search that is already running is waited out and then discarded.
- **Warm start.** `agentops-server` calls `agentops_heavy_api::warm_embedder()` before binding, which embeds one dummy string on a background thread.

## Alternatives considered

- **Meaning-based recall in local mode too.** Not chosen. It would need a long-running local process, which is out of scope for milestone 3a.
- **A separate embedding service the hook calls.** Not chosen. The server is already long-running and already holds the model for search.

## Consequences

- Remote and local recall now differ: the remote seed can surface a memory that shares no word with the prompt, and the local one can't.
- Remote hook latency includes one embed. The milestone 3a eval records p50/p95 and the timeout count.
- The model loads once per server start (BGE-small, CPU).

## Evidence

- `recall_seed_with` tests: no embedder gives the same output as `recall_seed`; a prompt sharing no word recalls a note; a stalled embedder returns the keyword result within the deadline.
- `agentops-mcp/tests/hooks_never_load_the_model.rs`.
- `docs/engineering/handoffs/m3a-eval.md`, item 4's row.

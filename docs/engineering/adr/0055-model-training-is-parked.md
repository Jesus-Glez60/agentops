# 0055. Model training is parked

Milestone 4 (robustness, scope, consolidation), 2026-10-24.

## Status

Accepted, 2026-10-24. The author decided it in the milestone 4 spec.

## Context

The cue-driven memory requirement ends with "session ends → consolidation". Two consolidation passes exist, and neither does anything useful on this graph (`handoffs/m3c-report.md` §6, measured on the 2026-10-23 snapshot):
- **Embedding consolidation** (`agentops_embeddings_train::consolidate`, run by the `end_session` MCP tool). Took 0.12 s and trained nothing: "0 pairs with both embeddings and a hard negative (of 500 candidates), need 10". It trains on `Affects`/`References` edges, and their symbol ends carry no embeddings.
- **LoRA model consolidation** (`consolidate_model`). Hung 14+ minutes in the Hugging Face download (`hf_hub`), while curl fetched the same URL in 0.3 s. Its adapter has no consumer (F29, ADR 0042), and the tool is hidden.

## Decision

- **Both passes are parked.** Nothing new calls them, and the `hf_hub` hang isn't investigated.
- **`end_session` stays as it is.** It is documented as a no-op on this graph (`06-notes-and-consolidation.md`).
- **Consolidation is redefined at the level of memories** ([ADR 0056](0056-consolidation-proposals.md)): an LLM proposes merges and durable summaries, and a person accepts them.

## What would reopen it

- **Embedding consolidation:** at least 10 training pairs with hard negatives. Either the symbol ends of `Affects` edges get embeddings, or training moves to memory-to-memory pairs (accepted merges and `memory_followed` lookups are candidates).
- **LoRA:** a consumer for the adapter (a local-first `explain_symbol`, F29), and the `hf_hub` download hang understood first.

## Alternatives considered

- **Fix the training data now.** Out of milestone 4's scope. The spec parks it.
- **Delete the passes.** Rejected: their crates are the starting point when a consumer arrives.

## Consequences

- The requirement's "consolidation" outcome is met by memory-level proposals, not by model weights.
- The disk, CPU and download cost of training is avoided until something would use it.

## Evidence

`handoffs/m3c-report.md` §6; `findings.md` F29; ADR 0042.

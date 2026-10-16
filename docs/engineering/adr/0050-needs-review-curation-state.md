# 0050. "Needs review" is a curation state

Milestone 2 of the cue-driven memory work (MEMORY.md import), 2026-10-16. Approved with the milestone 2 plan.

## Status

Accepted, 2026-10-16.

## Context

The MEMORY.md importer meets entries it can't classify with confidence: no `type` and no file-name prefix, a classifier below `CLASSIFY_MIN_CONFIDENCE`, or text that says it's stale or superseded. The requirement says such an entry is "never guessed, and never dropped". It needs a place where a person sees it and decides, without agents treating it as current knowledge in the meantime.

Curation already has `Full`, `Reduced` and `Pinned`, and the rule that nothing is ever resolved or hidden (decision "Gotchas are curated, never resolved/dismissed").

## Decision

Add `NodeProminence::NeedsReview` (`"needs_review"`).

- It requires a `curation_reason`, like `Reduced` (`requires_reason()`).
- It is never recalled: no prompt seed, subagent seed, pin or session briefing (`recallable()`). Search and `list_gotchas` still list it, ranked ×0.1 and marked "⚠ needs review — <reason>", so it is never hidden.
- It leaves `curated` false: no person has looked at it yet.
- The Gotchas workspace has a "Needs review" tab listing every memory kind in the state, with "Set kind" (`POST /repos/{id}/nodes/{node_id}/kind`, which also rewrites the note file's `type:`), "Accept" (→ `Full`) and "Reduce" (→ `Reduced` with a reason).
- Every curation and kind change on the hosted server is written to the tenant's audit log (`node_curated`, `node_kind_set`). This covers the existing curation actions too, which weren't logged before.

## Alternatives considered

- Import unclassifiable entries as `Reduced`. Not chosen: `Reduced` is a person's judgement ("matters less"), and it still reaches recall at half score. An unclassified entry hasn't been judged at all.
- A separate review queue table. Not chosen: the curation columns and the Gotchas workspace already exist, and a state on the node keeps it searchable and auditable in one place.
- Store the kind as unknown. Not chosen: `NodeKind` has no such value and a CHECK constraint guards it in Postgres. The importer stores these as `Note`, and "Set kind" moves them.

## Consequences

- Every exhaustive match on `NodeProminence` gained an arm; string parsers in both curation handlers accept `needs_review`.
- `GET /gotchas` entries carry `kind`, and its `needs_review` bucket spans gotchas, decisions and notes. The unfiltered list is still gotchas only, so the web app merges the two (`getCurationQueue`).
- The local single-operator `agentops-api` has no audit store and no set-kind route.

## Evidence

- `NodeProminence`, `requires_reason`, `recallable`, `prominence_label` in `agentops-core/crates/agentops-graph/src/lib.rs`.
- `set_curation_json`, `set_node_kind_json`, `audit_curation` in `agentops-core/crates/agentops-heavy-api/src/dashboard.rs`; test `needs_review_is_listed_across_kinds_its_kind_can_be_set_and_every_curation_is_audited`.
- `recall_seed` (`agentops-retrieval/src/recall.rs`) and the hook tests `a_needs_review_memory_never_reaches_a_prompt_seed_a_subagent_seed_or_the_pins`.

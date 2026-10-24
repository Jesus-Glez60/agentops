# 0056. Consolidation proposals

Milestone 4 item 4, 2026-10-24.

## Status

Accepted, 2026-10-24.

## Context

Sessions leave near-duplicate memories (milestone 3a found same-fact pairs at cosine 0.85-0.92) and progress memories whose lesson is never written down as a rule. Milestone 3b showed that an LLM is useful only as a proposer: its classifications hid useful gotchas that the title words kept. So consolidation must never apply anything by itself.

## Decision

- **Trigger.** `SessionEnd` starts a detached `agentops consolidate --session <id>`, as it does `usage sync`.
  - For a remote repo it runs on the server (`POST /repos/{id}/memories/consolidate`), with the tenant's Anthropic key, else Groq.
  - Locally it uses `AGENTOPS_ANTHROPIC_API_KEY`.
  - With no key, nothing runs and nothing fails.
  - It also does nothing when the session wrote no memory (`memory_written`) and wrote or followed no progress memory.
- **Candidates** (`agentops_mcp::proposals`):
  - **merge:** pairs among the session's written memories and their 5 nearest neighbours, at cosine `MERGE_MIN_SCORE` (0.85) up to `DUPLICATE_MIN_SCORE` (0.92). Both memories must be recallable and not progress.
  - **distill:** the progress memories the session wrote or followed, up to 6, newest first.
  - Candidate ids are stable: `m<a>-<b>`, `d<id>-...`.
- **One call per candidate** (`agentops_llm::propose_memory`, structured JSON). The reply is `worth: false`, or a kind, title and body.
  - At most `MAX_PROPOSALS_PER_SESSION` (3) per session end.
  - A run stops before any call once it has spent `RUN_TOKEN_CAP` (20,000) input plus output tokens, and keeps what it wrote.
  - Every run is logged as a `consolidation_run` session event: proposals, tokens, estimated USD, provider, model.
- **A proposal** is a note written by `add_note`. It is:
  - `NeedsReview`, so recall never shows it, with the curation reason "proposed by consolidation from <ids>";
  - durable, and tagged `proposed-by:consolidation`;
  - recorded in `consolidation_proposals` with its sources and a SHA-256 digest of their ids, titles and bodies;
  - not applied: nothing about its sources changes.
- **Only a person decides**, in the Gotchas workspace (`POST .../memories/proposals/{id}/accept|reject`). The plain curation route refuses a waiting proposal (409).
  - **Accept:** the proposal becomes `Full` and durable, and its sources are superseded the way `add_note`'s `supersedes` does it (temporal, ending today), so recall drops them. The spec said the sources become progress. The author chose to reuse `supersedes` instead, because a merged-away gotcha isn't a progress record (2026-10-24).
  - **Reject:** the proposal's node and note file are deleted, and the rejection is kept with its digest. The same sources aren't proposed again until one of them changes.
- **Batch.** `agentops consolidate --all --dry-run` lists every unblocked merge pair in the repo, writing nothing. `--generate <id>...` proposes exactly the picked ones: the token cap applies, the per-session maximum doesn't.

## Alternatives considered

- **Apply merges automatically above a threshold.** Rejected: milestone 3b's LLM judgments hid useful memories.
- **Store the sources in the curation reason only.** It can't be queried, and it gives rejections no digest.
- **Reject by reducing.** Decision 54003 keeps knowledge curated, never dismissed. An unaccepted proposal isn't knowledge yet, though: it's a draft an LLM wrote. Deleting it on rejection keeps the graph free of drafts. Its sources, which are knowledge, are never touched.

## Consequences

- Recall, search and the importer are unchanged until a person accepts.
- A merge is close to its own sources by design, so proposals are written with `AddMode::Write`, which never checks for duplicates.
- Cost is bounded per run and visible in `consolidation_run` events.

## Evidence

`handoffs/m4-report.md` §5 (proposals generated, accepted and rejected); tests in `agentops-mcp/src/proposals.rs`.

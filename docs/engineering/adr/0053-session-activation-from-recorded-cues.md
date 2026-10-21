# 0053. Session activation from recorded cues

Milestone 3b of the cue-driven memory work (recall during work), 2026-10-20. Decided in the milestone 3b spec, and amended at its item 3 timing gate.

## Status

Accepted, 2026-10-20.

## Context

Recall during work fires on every file read or edit and every failed tool call. That is about 40 cues per main session at the median and about 210 at p95 (`handoffs/m3b-eval.md`, Sizing). Recall then has to decide what is relevant now. A memory several recent cues reached is a better bet than one a single old cue touched. The main session and each subagent have separate context windows. A compaction drops what was shown earlier.

The spec asks for activation to be **derived, not stored**: computed from `session_events` on each hook call, with no new table. Taken literally, that means resolving each of the last 20 cues again on every hook call. One cue takes about 85 ms on the server at p50 and about 240 ms at p95 on a hub file. Twenty would take about 1.7 s against a 300 ms remote budget.

## Decision

- **Store each cue's scores; derive activation from them.** A cue that reaches any memory records one `work_cue` session event. Its description holds the cue's top `WORK_CUE_KEEP` (10) candidates as `id:score`, best first. A file cue that reaches nothing records an empty one; an error cue that reaches nothing records nothing.
  - **Activation formula.** A memory's activation is the sum, over the context's last `ACTIVATION_WINDOW` (20) `work_cue` events plus the cue being handled, of its score × `ACTIVATION_DECAY` (0.85) raised to the cue's age (the newest cue is age 0). This is `agentops_retrieval::work_recall::activation`, a pure function, wrapped by `agentops_mcp::work_recall::context_activation`.
  - Still no new table. Approved by the author at the timing gate, 2026-10-20.
- **One activation per context.** `session_events.agent_id` (nullable; `NULL` means the main session) keeps the contexts apart (`work_recall::context_events`).
- **Compaction resets the main context.** `SessionStart` with `source: compact` records a `context_reset` event, and the main context counts only events after its latest one. A subagent's context never resets, because Claude Code sends no signal when a subagent compacts.
- **The prompt counts as a cue.** A plan-mode or default-mode prompt records a `work_cue` from its recall seed's confidences (`recall_scored_with`). Work-time recall then carries on from what the prompt was about.
- **Scores are comparable across cues** (`05-retrieval.md`, "Recall during work"): recall's confidence, `Affects` dampened by how many memories a file or symbol reaches, and path mentions at 0.9 or 0.7.

- **A re-read moves activation but never injects** (milestone 3b revision 1, 2026-10-20). A file cued again in the same context re-records its earlier `work_cue` description, so activation still counts it once at its newest position, and nothing is shown. Only a file's first cue in a context can inject.

## Alternatives considered

- **Recompute every hook call** (the spec as written). Rejected on the measured latency above.
- **A per-session activation table.** Not needed: the scores per cue are small and already sit in the session's own event log.

## Consequences

- At most one extra event per cue that reaches anything. That is at most about 40 per median main session, and fewer in practice: of the 5 replay sessions, see `m3b-eval.md` for how many cues reach a memory.
- Activation reflects scores as they were when each cue happened. A memory retitled or curated later keeps its old score until it ages out of the window.

## Evidence

- `work_recall::tests::a_memory_three_recent_cues_reached_outranks_one_a_single_cue_reached_ten_cues_ago` (agentops-retrieval).
- `work_recall::tests::each_context_has_its_own_activation_and_compaction_resets_only_the_main_one` and `hooks::tests::reads_errors_and_prompts_record_work_cues_per_context_and_compaction_resets_the_main_one` (agentops-mcp).
- The timing gate in `handoffs/m3b-eval.md`.

# 0054. Memory classes

Milestone 3b revision 2 (recall during work), 2026-10-21.

## Status

Accepted, 2026-10-21.

## Context

The revision 1 labelling sample (`handoffs/m3b-eval.md`, Labels) found that work-time recall's noise was mostly status-only memories: notes that announce that something shipped or was completed, or summarise a session or plan, with no cause, rule or warning. They go stale as later builds land. Activation can't separate them from useful memories: on errors, both span the same 0.6–1.9 range, so no `INJECT_MIN_SCORE` removes them without removing useful memories as fast.

What a memory *is* decides whether it should be recalled, independently of how strongly a cue reached it.

## Decision

- **Every node has a `memory_class`** (`agentops_graph::MemoryClass`): `durable` (the default), `progress`, `experimental` or `temporal`. A temporal memory has a `class_until` date (UTC `YYYY-MM-DD`). Both are TEXT columns. An unknown value reads as `durable`.
- **One rule, in `MemoryClass::recallable_at`:**
  - `durable` and `experimental`: always recalled; an experimental pointer says "experimental" after its title.
  - `progress`: never recalled, from any cue. It records where the work stands: something shipped, implemented or completed; a session, phase or milestone summary; a plan or requirements for upcoming work.
  - `temporal`: recalled while today < `class_until` (exclusive, so a memory superseded today is gone today).
- **Every recall path applies it:** prompt seeds, subagent seeds and error cues in `recall_ranked_with` (keyword, named-symbol, dense and hop entries), and file cues in `work_recall`'s `recallable`.
- **No class hides a memory.** Search, `list_gotchas` and the dashboard show every class. Nothing is resolved or dismissed (decision 54003).
- **Set like curation.** Only `GraphStore::set_memory_class` writes the class, never `upsert_node`, so a rescan keeps it.
- **Where a class comes from:**
  - `add_note` takes `memory_class`, `class_until` and `supersedes` (ids that end today).
  - Without one, a decision's or note's title that records progress is classed `progress` (`agentops_notes::heuristic_class`, whole title words and phrases; never a gotcha).
  - The `set_memory_class` MCP tool corrects a wrong class.
  - `agentops classify-memories` backfills existing memories: it proposes classes (an LLM judgment when the tenant has an Anthropic or, failing that, a Groq credential; `--per-minute` paces a free tier), the author reviews them, and `--apply` writes exactly what was reviewed.

## Amendment, 2026-10-22: `status` became `progress`

The class first shipped as `status`, still recalled through an `Affects` link from the opened file. On the held-out sample the author scored those 2 and 4 out of 10, and plans, requirements and session summaries proved to be the same noise. So the class was widened and renamed `progress`, and the `Affects` exception dropped. Stored `status` values were migrated, and `status` still reads as `progress`. The title words gained "finished", "piloted", "requirements for" and "Session N"; single words such as "session", "plan" and "milestone" were tested against all 390 titles and rejected, because they caught real decisions.

## Alternatives considered

- **A new node kind for progress.** Same recall effect as a class, but it needs widening Postgres's kind CHECK constraint (gotcha 54053) and every exhaustive kind match.
- **Tags.** `add_note`'s tags never reach the graph node (gotcha 62450), so recall can't read them.
- **Prominence.** `Reduced` is a person's curation act and only lowers rank; a status memory would still be recalled, and the two meanings would blur.
- **A higher `INJECT_MIN_SCORE`.** Measured and rejected: it removes useful memories as fast as noise.
- **Splitting long memories into smaller ones.** It adds pointers rather than removing them; left to milestone 4 as a precision question.

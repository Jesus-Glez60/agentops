# 0054. Memory classes

Milestone 3b revision 2 (recall during work), 2026-10-21.

## Status

Accepted, 2026-10-21.

## Context

The revision 1 labelling sample (`handoffs/m3b-eval.md`, Labels) found that work-time recall's noise was mostly status-only memories: notes that announce that something shipped or was completed, or summarise a session or plan, with no cause, rule or warning. They go stale as later builds land. Activation can't separate them from useful memories: on errors, both span the same 0.6–1.9 range, so no `INJECT_MIN_SCORE` removes them without removing useful memories as fast.

What a memory *is* decides whether it should be recalled, independently of how strongly a cue reached it.

## Decision

- **Every node has a `memory_class`** (`agentops_graph::MemoryClass`): `durable` (the default), `status`, `experimental` or `temporal`. A temporal memory has a `class_until` date (UTC `YYYY-MM-DD`). Both are TEXT columns. An unknown value reads as `durable`.
- **One rule, in `MemoryClass::recallable_at`:**
  - `durable` and `experimental`: always recalled; an experimental pointer says "experimental" after its title.
  - `status`: recalled only through an `Affects` link from the file being worked on. Never from a prompt, a subagent seed, an error or a path mention.
  - `temporal`: recalled while today < `class_until` (exclusive, so a memory superseded today is gone today).
- **Every recall path applies it:** prompt seeds, subagent seeds and error cues in `recall_ranked_with` (keyword, named-symbol, dense and hop entries), and file cues in `work_recall`'s `recallable`.
- **No class hides a memory.** Search, `list_gotchas` and the dashboard show every class. Nothing is resolved or dismissed (decision 54003).
- **Set like curation.** Only `GraphStore::set_memory_class` writes the class, never `upsert_node`, so a rescan keeps it.
- **Where a class comes from:**
  - `add_note` takes `memory_class`, `class_until` and `supersedes` (ids that end today).
  - Without one, a title that only announces a status is classed `status` (`agentops_notes::heuristic_class`, titles only).
  - The `set_memory_class` MCP tool corrects a wrong class.
  - `agentops classify-memories` backfills existing memories: it proposes classes (an LLM judgment when the tenant has an Anthropic credential), the author reviews them, and `--apply` writes exactly what was reviewed.

## Alternatives considered

- **Tags.** `add_note`'s tags never reach the graph node (gotcha 62450), so recall can't read them.
- **Prominence.** `Reduced` is a person's curation act and only lowers rank; a status memory would still be recalled, and the two meanings would blur.
- **A higher `INJECT_MIN_SCORE`.** Measured and rejected: it removes useful memories as fast as noise.
- **Splitting long memories into smaller ones.** It adds pointers rather than removing them; left to milestone 4 as a precision question.

# Milestone 2 handoff: MEMORY.md import

Written 2026-10-19 when milestone 2 of the cue-driven memory requirement finished its pilot on this repo. It feeds the milestone 3 spec (cued recall tuning). Facts only, each with a file path, commit or command output; anything not verified is marked **unverified**. **No memory contents:** this repo is public, so entries appear as counts and node ids only.

## 1. What shipped

All on `main`, deployed by the push-to-main autodeploy; the last deploy, `bf5f702`, passed the server's CI gate (Rust 1224, web 199, marketing 7) on 2026-10-17.

| Item | Commit | What |
| --- | --- | --- |
| Prerequisite | `7dca996`, `9709ea3`, `7303e87` | Milestone 1's four checks run and recorded in `m1-report.md`; fixes found on the way: `c51c5fd` (hand-backs), `2ecbf3c` (evidence gate), `30f2361` (path hint), `36a2733` (council seed), `3ebe5a1` (hook repo root, F59) |
| 0 | `1bcb180` | Spike, `docs/engineering/spikes/m2-memory.md` |
| 1 | `5d49b76` | `NodeProminence::NeedsReview`, `set_node_kind`, `POST /repos/{id}/nodes/{node_id}/kind`, Needs review tab, audit log for every curation; ADR 0050 |
| 2 | `e4e8fef` | `EdgeRelation::SimilarTo`, `agentops_graph::similarity`, one hop in `recall_seed`, `agentops link-similar`; ADR 0051 |
| 3 | `bf51ea3` | `agentops import-memory`; `add_note` `dry_run` / `skip_duplicates` / `node_id`; shared `/mcp` client |
| 4 | `a328c2e` | Procedural rules pinned as one-line rules; `PIN_BUDGET` on top of the hook budgets |
| Pilot fixes | `9e8a289`, `e054b45`, `bf5f702` | Every rule pinned, stale check narrowed, `PIN_BUDGET` 3,000 (F61); `PIN_LINE_MAX` (F60); `agentops-server link-similar` (F58) |
| 5 | (no code) | Dry runs, `--apply`, backfill, latency, switch-off, recall checks; this report |

Deviations from the spec, and who approved them:

| Deviation | Approved by |
| --- | --- |
| Every curation action is audit-logged, not only NeedsReview ones (the spec assumed existing ones were; they weren't) | The author, 2026-10-16 |
| Entries whose description or MEMORY.md line says stale/superseded go to NeedsReview (not in the spec); first checked whole bodies, narrowed after the first dry run | The author, 2026-10-16 and 2026-10-17 |
| `PIN_BUDGET` is added on top of the prompt and subagent budgets, and procedural pins drop the title; raised 2,000 → 3,000 after the dry run | The author, 2026-10-16 and 2026-10-17 |
| A rule that duplicates an existing memory pins that memory instead | The author, 2026-10-17 |
| Every pin line is cut to `PIN_LINE_MAX = 180` | **Not explicitly approved**; my call after F60 broke the "15 rules fit" check |
| The backfill runs as `agentops-server link-similar` inside the app container, not the CLI on the server | The author, 2026-10-17 |
| A rescan creates missing `SimilarTo` edges but never reinforces them | **Not explicitly approved**; my call, recorded in ADR 0051 |
| The local `agentops-api` accepts NeedsReview but has no audit log and no set-kind route | **Not explicitly approved**; my call, recorded in ADR 0050 |
| Unclassifiable entries are stored as `Note` nodes | Plan assumption the author approved, 2026-10-16 |

Spike findings (`spikes/m2-memory.md`): entries carry `type` nested as `metadata.type` (49 of 49), not top level as documented; `autoMemoryEnabled: false` in `.claude/settings.local.json` removes the whole memory section from the system prompt (7,502 → 5,402 characters); with it on, the agent has no reason to write memory files.

## 2. Interfaces milestone 3 will reuse

- **Similarity edges** (`agentops-core/crates/agentops-graph/src/similarity.rs`):
  - `link_similar(store: &dyn GraphStore, repo: &str, node_id: i64) -> Result<LinkSummary>`: top `SIMILAR_K` above `SIMILAR_MIN_SCORE`, both directions, reinforce existing pairs, hold `SIMILAR_MAX_DEGREE`.
  - `link_all_similar(store, repo, reinforce: bool) -> Result<LinkSummary>`; `similar_memories(store, repo, embedding, exclude: Option<i64>, k) -> Result<Vec<(Node, f32)>>`; `candidate_pair_scores(store, repo, per_node) -> Result<Vec<f32>>`; `cosine`.
  - `EdgeRelation::SimilarTo` (`"similar_to"`), `SIMILAR_TO_EDGE_HALF_LIFE_DAYS = 90`.
- **Traversal:** `recall_seed` (unchanged signature) follows one `SimilarTo` hop from each node that qualified on its own cues, never from a `DocSection`. Neighbour score = source × 0.5, one cue, reason `similar to "<title>"`; excluded, pinned and NeedsReview neighbours are dropped.
- **NeedsReview in ranking:** `NodeProminence::NeedsReview` (`"needs_review"`); `recallable()` false (recall seed, subagent seed, pins, briefing); `prominence_rank_multiplier` 0.1 and `prominence_label` "needs review" everywhere else; `requires_reason()` true; `set_curation` leaves `curated` false.
- **Importer:** `agentops import-memory --project <repo> [--memory-dir <dir>] [--apply] [--report <file>] [--llm-classify]`; `import_memory::import(memory_dir, sink: &dyn Sink, llm: Option<&dyn KindClassifier>, apply) -> Result<Vec<Record>>`. Report record: `{source_file, source_line, title, signal, memory_kind, node_kind, confidence, action, node_id, duplicate_of, pinned, similar_to, warnings, rule_line?}`.
- **`add_note`:** `add_note_with(.., mode: AddMode)` with `Write | WriteUnlessDuplicate | DryRun`; MCP `dry_run` / `skip_duplicates` add a `result: {json}` line.
- **Pins:** `session_guide::pinned_block(store, repo) -> (String, Vec<i64> cut)`, `render_pin`; `PIN_BUDGET = 3000`, `PIN_LINE_MAX = 180`, `HOOK_FIELD_MAX = 10_000`.

## 3. Measurements

**Similarity score distribution** (`agentops-server link-similar Jesus-Glez60--agentops--<tenant>`, 2026-10-17; 357 memories, 2,621 distinct candidate pairs, 10 nearest per memory):

| Cosine | Pairs |
| --- | --- |
| 0.90–0.95 | 17 |
| 0.85–0.90 | 114 |
| 0.80–0.85 | 624 |
| 0.75–0.80 | 1,258 |
| 0.70–0.75 | 539 |
| 0.65–0.70 | 69 |

The backfill embedded 51 memories that had no vector (F58), created 2,014 `SimilarTo` edges and dropped 115 over the degree cap.

**Final parameter values and whether they hold:**

| Name | Value | Holds? |
| --- | --- | --- |
| `SIMILAR_K` | 5 | Unverified; masked by the threshold |
| `SIMILAR_MIN_SCORE` | 0.75 | **No** (F56): 77% of candidate pairs clear it. Proposal: 0.82, roughly the top quarter (≥ 0.80 is 755 pairs, ≥ 0.85 is 131). Raising it doesn't remove edges already created at 0.75; that needs a prune step |
| `SIMILAR_MAX_DEGREE` | 12 | Did most of the selecting (115 drops); revisit after the threshold |
| `SimilarTo` half-life | 90 days | Unverified (no decay yet) |
| `DUPLICATE_MIN_SCORE` | 0.92 | Holds as far as checked: 17 pairs above 0.90; the 3 duplicates the importer found were all memories already recorded as notes |
| `CLASSIFY_MIN_CONFIDENCE` | 0.7 | Not exercised: all 49 entries had a frontmatter type |
| `RULE_LINE_MAX` | 160 | Holds: every rule reads as one line |
| `PIN_BUDGET` | 3,000 (was 2,000) | Holds: live block 2,482 characters, 15 rules, none cut |
| `PIN_LINE_MAX` | 180 | Added after F60 |

**Unlinked memories:** after the backfill, 0 of 357 memories have no edge of any kind. Before: **unverified** (not measured; the backfill only reports the after count).

**Pin block:** 2,482 characters, 15 rule lines, no cut line (live `UserPromptSubmit` output, 2026-10-17, after `e054b45`). Before `e054b45`: 2,760 characters, 14 rules, node 104420 named as cut.

**Hook latency** (remote, this machine to the hosted server, 20 runs each, 2026-10-17): prompt in default mode p50 100 ms / p95 120 ms; plan mode p50 107 ms / p95 148 ms; `SubagentStart` p50 97 ms / p95 115 ms. Target p95 under 1 s remote: met.

**Five-prompt recall check:** **2 of 5, failed** (target 4). See §5.

**MEMORY.md off:** a fresh session (`89f51712`, 2026-10-20) had no memory section in its system prompt (5,402 characters, 0 mentions of `MEMORY.md`), and the 15 pinned rules arrived at `SessionStart` and with its first prompt. Milestone 1's write redirect is still on.

## 4. The import

`agentops import-memory --project . --apply`, 2026-10-17, into the hosted connection `Jesus-Glez60--agentops`. Backup: `.context/memory-backup/2026-10-17/` (50 files; `diff -r` against the memory directory: identical).

| By action | | By memory kind | | By signal | | By node kind | |
| --- | --- | --- | --- | --- | --- | --- | --- |
| create | 44 | episodic | 29 | frontmatter | 49 | knowledge | 46 |
| duplicate | 3 | procedural | 15 | prefix | 0 | gotcha | 3 |
| needs_review | 2 | semantic | 5 | heuristic / llm / none | 0 | | |

- Entries in = records out: 49 = 49 (49 entry files; MEMORY.md indexes 48, the 49th is warned as unlinked).
- Created nodes 104407–104452. NeedsReview: 104422, 104444. Duplicates of existing notes: 54003, 54067, 54017; 54003 is pinned in place of the duplicate rule.
- Pinned: 15 of 15 rules. With a `SimilarTo` edge at import time: 47 of 49.
- Re-run: `--apply` again creates nothing (46 skip, 3 duplicate).
- Reviewed in the web workspace by the author: **unverified** at the time of writing.

Where classification went wrong:

- **Stale detection** (F61): checking whole bodies flagged 12 entries, mostly other senses of "stale". Narrowed to description and index line: 2 flagged, 1 of them correct, 1 a false positive (its index line says an *earlier* status was stale).
- **Titles** (F55): the frontmatter `name` is a slug, so every imported note is titled like its file. Hurts lexical recall (§5).
- **Classifier paths untested on real data:** no entry lacked a type, so the heuristic, LLM and "unclassified" paths ran only in the fixture test.

## 5. Recall quality now

Five plan-mode prompts, each about one imported memory, run through the real `UserPromptSubmit` hook against the hosted graph (2026-10-19). Seed limit 8 (`PLAN_SEED_MAX_ITEMS`).

| # | Prompt | Target | Result |
| --- | --- | --- | --- |
| 1 | "how do I get a change live on the production box now?" | 104450 | Hit, rank 1 |
| 2 | "add a sign-up button to the public marketing page" | 104430 | Miss (decision 54022, the same fact recorded earlier, was recalled) |
| 3 | "which server should a freshly connected repo talk to?" | 104449 | Miss |
| 4 | "pick up where we left off on the rainbow six stats project" | 104437 | Hit, rank 2 |
| 5 | "turn what we built here into something I can show employers" | 104452 | Miss |

- **Noise:** the three digest doc sections (33704, 33705, 58350) took 3 of 8 slots on all five prompts (F53).
- **Vocabulary misses:** prompts 3 and 5 share no word with their memory. Lexical recall can't bridge that; a `SimilarTo` hop could, but only from a memory that matched.
- **Hop neighbours lose:** a neighbour counts as one cue, so seeds already full of 2-to-5-cue keyword hits cut it.
- **Slug titles** (F55) give titles no matchable words beyond the file name's.

## 6. Second-pass readiness

- **Server connections** (`.context/agentops-remote.json` under `~/Repos`, depth 4): this repo, `r6-stream-hub` (0 memory files) and `job-hunter` (3 files). The other projects with memory directories have no connection; the importer would write to their local graph.
- **Memory directories:** 27 under `~/.claude/projects/`; 6 hold no entry files.
- **User-level directory** (`-Users-jesusglez`): 5 entry files, MEMORY.md 31 lines. It has no repo, so `--project` has no natural target; the second pass needs a decision on where user-level memories go.
- **Inline-only files:** `ai-fitness-app` has 0 entry files and a 106-line MEMORY.md; `ztrack-v2` has 2 files and 53 lines. The importer reads entry files only, so it would import 0 and 2 entries. It needs to parse inline MEMORY.md entries.
- **Client-specific:** 14 directories belong to client repos (DDS, impact-ignite, semantic, transactly, tyna). Their entries are client data; none has a server connection. Importing them to a hosted server needs the author's call per client.

## 7. Open issues

- New findings: F55–F58 and F62 open; F59–F61 resolved (`findings.md`).
- What milestone 3 must not break:
  - Milestone 1's interfaces (tool names `user_prompt` / `user_prompt:plan`, `recall_injected`, `tags_of` reading the last line, `memory-kind:*` tags) and `recall_seed`'s signature.
  - NeedsReview never recalled.
  - Pins always first, within `PIN_BUDGET`, never silently cut.
  - No embedding model inside a hook.
  - The importer staying idempotent on `add_note`'s natural key.
- Questions for the milestone 3 spec:
  1. Re-title imported notes from their `description` (F55)? It changes the natural key, so it needs a re-title operation, not a re-import.
  2. Raise `SIMILAR_MIN_SCORE` (proposal 0.82), and prune existing edges below it?
  3. Should a `SimilarTo` neighbour of a strong match outrank a weak keyword hit, for example by counting the source's cues?
  4. Leave digest doc sections out of the plan-mode seed entirely, instead of down-weighting them (F53)?
  5. Should hosted reindexing embed notes (F58), so the backfill isn't a manual step?

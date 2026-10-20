# Milestone 3a handoff: recall quality

Written 2026-10-20 when milestone 3a of the cue-driven memory requirement finished. It feeds the milestone 3b spec (recall during work). Facts only, each with a commit, path or command output; anything not verified is marked **unverified**. **No memory text and no evaluation prompts:** this repo is public, so cases appear as ids and nodes as ids.

**Result: 15/20 in plan mode, one short of the 16/20 target.** Every other measured target is met (§2). The last item, keyword rarity weighting (IDF), was added with the author's approval when the eval showed it was the remaining lever. It raised MRR but not the hit rate.

## 1. What shipped

All on `main`, deployed by the push-to-main autodeploy. Per-item scores come from `agentops recall-eval --remote --repeat 5` against the hosted graph (`m3a-eval.md`).

| Item | Commits | Eval after | What |
| --- | --- | --- | --- |
| 0 | `bc0d370`, `7b5d469` | 10/20 baseline | `agentops recall-eval`, `GET /repos/{id}/nodes/{node_id}/similarity`, baseline |
| 0b | `e5e165f` | 9/20 re-baseline | `POST /repos/{id}/recall` (deep ranking, records nothing); `deep_rank`, `miss_category` |
| 1 | `e976a03` | 11/20 | `GraphStore::rename_node`, `retitle_note`, `POST /repos/{id}/nodes/{node_id}/title` (audited `node_retitled`), `import-memory --retitle`; 46 retitles applied on the server |
| 2 | `96d82a2`, `6399325` | 13/20 | No `DocSection` in any seed (`RECALL_KINDS`) |
| 3 | `9db16a7`, `0fd3880`, `4699cdb`, `c68d349` | 9 → 11 → 11 → 13/20 | Hop scored `source × cosine × 0.8`, at most 2 neighbours per source, deterministic; "inherit strength" reverted |
| 4 | `1aab7ff` | 14/20 | `recall_seed_with`, `hooks::handle_with`, server-only dense recall with RRF and a 250 ms guard, warm start; ADR 0052 |
| 5 | `197d892` | 15/20 | `SIMILAR_MIN_SCORE` 0.82, floor link at 0.75, `prune_similar`, `link-similar --prune`; prune run on the server |
| 6 | `f17850c` | 15/20 | `embed_and_link_new` in the reindex job (F58 closed, verified live) |
| Extra: IDF | `f001b01` | 15/20 (MRR 0.475 → 0.506) | `GraphStore::count_lexical`, `count_nodes`; keywords weighted `ln(1 + N/df)` |

Deviations from the spec, and who approved them:

| Deviation | Approved by |
| --- | --- |
| Retitling keeps the vector instead of re-embedding: notes are embedded from their body, never their title (plan item D1) | The author (plan, 2026-10-19) |
| Deep-rank route and fields (`deep_rank`, `miss_category`) added to the eval contract | The author, 2026-10-20 |
| Item 3's "neighbour inherits its source's strength" reverted after two attempts regressed; kept cosine scoring, a cap of 2 per source and determinism | The author, 2026-10-20 (three decisions: cap, one cue fewer, revert) |
| IDF weighting, out of the spec's scope unless needed | The author, 2026-10-20 |
| Server prune run and the 46 production retitles | The author (retitle list reviewed), the plan (prune) |
| Deleting the 54.5 GB CI cache volume to end the outage (F66) | The author, 2026-10-20 |
| A write with nothing above 0.82 links its closest neighbour above 0.75 (floor on writes as well as in the prune) | **Not explicitly approved**; my call, documented in `03-data-model.md` |
| Two import entries sharing a description: the later keeps its `name` as title, so it is still detected as a duplicate | **Not explicitly approved**; my call |
| No unique index on the natural key, so a taken title is refused by a check before the write (the council asked to map a constraint error too; there is none to map) | **Not explicitly approved**; my call |
| "Missing memory" is not a miss category: unrecallable targets are refused before a run | **Not explicitly approved**; my call |
| Item 6's job wiring verified live on the server, not by extending the flaky end-to-end indexing test (F48) | **Not explicitly approved**; my call |
| `warm_embedder` lives in `agentops-heavy-api` and is called by `agentops-server` (which doesn't depend on `agentops-embeddings`) | **Not explicitly approved**; my call |
| Dense timeouts counted from the PM2 error log inside the container, since app output never reaches `docker logs` | **Not explicitly approved**; my call |

## 2. Evaluation

20 cases, plan mode, seed limit 8, 100 runs per evaluation. The cases are milestone 2's five plus 15 drafted from the graph and approved by the author. They live in `.context/recall-eval.jsonl`, git-ignored.

| | Baseline (`bc0d370`) | Final (`f001b01`) | Target |
| --- | --- | --- | --- |
| Hit rate | 10/20 | **15/20** | ≥ 16/20 — **not met** |
| Milestone 2's 5 prompts | 3/5 (2/5 in m2's manual check) | **4/5** | ≥ 4/5 — met |
| MRR | 0.239 | 0.506 | — |
| Latency p50 / p95 (remote) | 105 / 141 ms | 104 / 129 ms | p95 ≤ 298 ms — met |
| Dense timeouts | — | 0 of 100 runs | < 1% — met |
| Unlinked memories after prune | — | 0 of 365 (0.0%) | < 5% — met |

Change in hit rate per item: 0b re-baseline −1 (graph drift; m2-2 moved from rank 8 to 9), item 1 +2, item 2 +2, item 3 0 (after the revert; −4 and −2 before it), item 4 +1, item 5 +1, item 6 0, IDF 0.

**Cases still missing, by category** (`miss_category` and the analysis in `m3a-eval.md`):

- **Ranking: 4** (m2-3, c06, c07, c12; deep ranks 34, 13, 24, 17). Each matches two common words and loses to many other two-word matches. None is in the dense candidates, and `/local-search` hybrid search ranks them 29, 55, 69 or not at all, so BGE-small doesn't place them near their prompts either. IDF lifted two of them but none into the seed.
- **Vocabulary: 1** (c14). Not in recall's candidate pool by keyword or by meaning.
- **Noise: 0**, **missing memory: 0** (unrecallable targets are refused up front).

**Equivalence never fired.** The one pair the spec expected to settle milestone 2's prompt 2 scores 0.868, under `EQUIVALENT_MIN_SCORE`. That prompt hits exactly now anyway.

## 3. Final parameter values

| Name | Value | Did the evaluation support it? |
| --- | --- | --- |
| `EQUIVALENT_MIN_SCORE` | 0.88 | Untested in practice: no equivalent hit occurred; the one candidate pair scored 0.868 |
| `HOP_FACTOR` | 0.8 | Kept; with a neighbour counted as one cue it neither helped nor hurt measurably |
| `HOP_MAX_PER_SOURCE` (new) | 2 | Yes: without it one hub filled 5–7 of 8 slots (9/20) |
| `DENSE_MIN_SCORE` | 0.55 | Partly: item 4 gained a vocabulary case (m2-5); the remaining misses are outside the dense top 20, so lowering it is unlikely to help (**unverified**: not run) |
| `DENSE_TOP_K` | 20 | Same as above (**unverified**) |
| Fusion | RRF, k = 60 | Kept; no alternative measured |
| `DENSE_TIMEOUT_MS` | 250 | Yes: 0 timeouts in 100 runs, p95 129 ms |
| `SIMILAR_MIN_SCORE` | 0.82 | Yes: +1 case after the prune; 709 pairs pruned |
| `SIMILAR_MIN_EDGES` | 1 | Yes: 0 memories unlinked before and after |
| `SIMILAR_FLOOR_SCORE` | 0.75 | Yes: 94 floor pairs kept or added |
| `RETITLE_MAX` (new) | 90 | Yes: the author read the 46 retitled titles in the web workspace (2026-10-20) |
| Keyword weight (new) | `ln(1 + N/df)` | MRR +0.03, hit rate unchanged |

## 4. Interfaces milestone 3b will reuse

- **Recall entry point** (`agentops-core/crates/agentops-retrieval/src/recall.rs`):
  - `pub fn recall_seed(store: &dyn GraphStore, repo: &str, query: &str, limit: usize, exclude: &HashSet<i64>) -> Result<Vec<RecallPointer>>` (signature unchanged; keyword and graph only).
  - `pub fn recall_seed_with(store: &dyn GraphStore, repo: &str, query: &str, limit: usize, exclude: &HashSet<i64>, embedder: Option<&'static (dyn Embedder + Sync)>) -> Result<Vec<RecallPointer>>` (with `None` it is exactly `recall_seed`).
  - Constants exported from `agentops_retrieval`: `DENSE_MIN_SCORE`, `DENSE_TOP_K`, `DENSE_TIMEOUT_MS`, `HOP_FACTOR`, `RECALL_KINDS`.
- **Embedder plumbing** (`agentops-core/crates/agentops-mcp/src/hooks.rs`):
  - `pub fn handle(store, repo, event, payload) -> Result<Option<Value>>` is what the local `hook-capture` calls; it never loads the model.
  - `pub fn handle_with(store: &dyn GraphStore, repo: &str, event: &str, payload: &Value, embedder: Option<&'static (dyn agentops_embeddings::Embedder + Sync)>) -> Result<Option<Value>>`. The server's hook route passes `Some(&SERVER_EMBEDDER)` (`agentops-heavy-api/src/dashboard.rs`).
  - `agentops_embeddings::model_loaded() -> bool`; `agentops_heavy_api::warm_embedder()`.
- **Store additions** (`agentops-graph`): `rename_node`, `get_embeddings`, `count_lexical`, `count_nodes`; `similarity::{prune_similar, link_new, SIMILAR_MIN_EDGES, SIMILAR_FLOOR_SCORE}`.
- **Evaluation command:** `agentops recall-eval [--path <repo>] [--remote] [--cases <jsonl>] [--mode plan|default] [--repeat N]` (`agentops-cli/src/recall_eval.rs`). Its server routes are `POST /repos/{id}/recall` and `GET /repos/{id}/nodes/{node_id}/similarity?to=…`. Eval sessions use ids starting `recall-eval-`.

## 5. Inputs for recall during work

**Tool calls per session**, from `session_events` on the server, last 30 days, `recall-eval-*` excluded. 241 sessions, and only 14 have any tool event (F68), because PostToolUse records only edits, git ops, large outputs, errors and cache hazards.

| Recorded per session | All 241: median / p95 | The 14 with tool events: median / p95 |
| --- | --- | --- |
| `Read` (recorded only for large output, cache hazard or error) | 0 / 0 | 0 / 45 |
| `Edit` (`file_modified`) | 0 / 0 | 0 / 33 |
| `Write` (`file_modified`) | 0 / 0 | 0 / 9 |
| `Grep` | 0 / 0 | 0 / 0 |
| Failed tool calls (`error`) | 0 / 0 | 0 / 31 |

These are lower bounds, not tool-call counts: plain Reads and Greps are never recorded, and edits made by scripts never pass the hook. Sizing a work-time hook needs another source, such as the Claude Code session transcripts (`usage` already parses them).

**Memories reachable from a file path today** (351 recallable memories). The files are those `file_modified` events name: only 13 repo files appear, not 20, for the reason above. "Mentions" matches the file's basename in note text, which overcounts generic names such as `lib.rs`.

| File | Via `Affects` to its symbols | Via a path mention |
| --- | --- | --- |
| `agentops-mcp/src/hooks.rs` | 50 | 0 |
| `agentops-cli/src/recall_eval.rs` | 43 | 0 |
| `agentops-heavy-api/src/lib.rs` | 41 | 46 |
| `agentops-mcp/src/notes.rs` | 27 | 1 |
| `agentops-cli/src/import_memory.rs` | 25 | 0 |
| `agentops-graph/src/similarity.rs` | 23 | 0 |
| `agentops-heavy-api/src/dashboard.rs` | 6 | 6 |
| `docs/engineering/handoffs/m1-report.md` | 0 | 2 |
| `docs/engineering/handoffs/m2-report.md` | 0 | 1 |
| `docs/engineering/handoffs/m3a-eval.md`, `spikes/m2-memory.md`, `05-retrieval.md`, `agentops-mcp/tests/hooks_never_load_the_model.rs` | 0 | 0 |

Code files are well reached through `Affects`. Docs files are reached by neither route.

**PostToolUse hook latency** (an `Edit` payload, 20 runs each, 2026-10-20):
- Remote, from this machine: p50 22 ms, p95 49 ms.
- Local: p50 6 ms, p95 9 ms. That was measured with the debug build of `agentops hook-capture` on an empty repo graph.

**What the session guide already tracks** (`agentops-mcp/src/session_guide.rs::build`): per session, the last 10 `file_modified` paths, plus decisions, unresolved errors, git ops, external references and secret warnings. File events come from `hooks::classify_post_tool_use` (Edit, Write, MultiEdit, NotebookEdit). An activation state could reuse `file_modified`, but nothing records file *reads* today.

## 6. Open issues

**New findings** (F63–F69 in `findings.md`):
- F63: edges store no score.
- F64: the hop was nondeterministic (resolved).
- F65: the `LocalEmbedder` test.
- F66: the CI cache filled the disk; resolved, now capped at 20 GB with a 25 GB free-space floor.
- F67: stale CI builds after a gate.
- F68: PostToolUse records very little.
- F69: deep rank is bounded by the candidate pool.

Each is also a gotcha on the hosted graph.

**What milestone 3b must not break:**
- The local hook never loads the model: `handle` passes no embedder, and `hooks_never_load_the_model` must stay green.
- `recall_seed`'s signature, and that `recall_seed_with(.., None)` equals it.
- Milestone 1 and 2 interfaces: the tool names `user_prompt` / `user_prompt:plan`, the `recall_injected` format, `tags_of`, `memory-kind:*`.
- Needs-review memories are never recalled.
- Pins come first, within `PIN_BUDGET`.
- The importer stays idempotent, now also after `--retitle`.
- At most 2 hop neighbours per source.

**Questions for the milestone 3b spec:**
1. **The last case to 16/20.** All four ranking misses tie on common words, and neither IDF nor meaning lifts them into the seed. Options: weight the keyword list above the dense list in fusion; give the work-time hook its own cue (the file being edited) instead of improving the prompt seed further; or accept 15/20 for prompt-time recall.
2. **Sizing work-time recall.** `session_events` can't size it (F68). Measure from session transcripts first, or record every Read/Grep for a pilot week?
3. **Docs files.** Docs files have no reachable memories (§5). Should recall during work on a doc fall back to the doc's own section or its links?
4. **CI cold builds.** The 20 GB cap on the CI cache (F66) means a cold build every few gates. If that gets slow, a larger disk (about 75 GB is unallocated in `ubuntu-vg`) would allow a higher cap.

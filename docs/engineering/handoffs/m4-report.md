# Milestone 4 handoff: robustness, scope, consolidation

**Final, 2026-10-25.** Covers the milestone 4 spec. Facts only, each tied to a commit, a script or a command's output; anything not verified is marked **unverified**. **No memory text, prompts or paths from other projects:** this repo is public, so cases appear as ids and memories as node ids.

**Result: recall no longer swings on graph data; one new memory costs a target at most 1 rank. Memories can be scoped, follows are recorded, and session-end consolidation proposes memories that only a person accepts. 7 of the 10 proposals generated were accepted, and neither eval dropped.**

| Done when | Result | |
| --- | --- | --- |
| Original prompt cases within 1 of each other on the 10-20 and 10-21 backups, ≥ 14/20 on both | 15/20 and 15/20 (§2) | Met |
| Adding any one of the 26 memories moves no seed target by more than 3 ranks | Worst move: 1 rank (c15, memory 235645) | Met |
| Same graph, identical ranks in 5 separate processes | Identical on both backups | Met |
| Parallel reads of one file inject each memory once (F71: 0% duplicates) | Live: 5 injections over 5 trials, 0 duplicates | Met |
| Redrawn sets ≥ 13/20 prompt and 6/20 work; held-out noise ≤ 30% | 15/20 and 6/20; noise 0 of 30 by per-memory proxy (§2) | Met, noise by proxy |
| A live session follow records `memory_followed` with the right source | Recorded: `file_cue`, durable (§3) | Met |
| `recall-stats` reports follow rates per source and class | Route and CLI live (§3) | Met |
| Payloads carry `client_host` and nothing else new | Tested on the forwarded body (`18d2699`) | Met |
| Error cue on another host skips a scoped memory; a file cue labels it | Tests (`18d2699`) | Met |
| The known situational memories scoped through a reviewed backfill; their error-cue rows gone in a replay as another host | 14 scoped after review; 2 rows → 0 (§4) | Met; see the note on "4 memories" |
| A session that wrote memories produces ≤ 3 proposals at its end, as needs-review nodes, sources unapplied | Tests; the batch path verified live | Live session end **unverified** |
| Accept applies `supersedes`; reject is recorded and the sources aren't re-proposed | Verified live through recall (§5) and in tests | Met |
| No LLM key: nothing runs, nothing fails | Tests | Met (not live: the tenant has a key) |
| Batch dry run lists candidates; ≥ 10 proposals generated and reviewed, at least half worth accepting | 106 candidates; 10 proposals; 7 accepted | Met |
| After accepting, neither evaluation drops | Prompt 15/20 (MRR 0.450 → 0.456), work 6/20 | Met |
| Docs: each item, the ADR parking model training, the consolidation ADR | ADR 0055, ADR 0056; docs listed in §1 | Met |

## 1. What shipped

All on `main`, deployed by the push-to-main autodeploy. The last deploy is `91da126` (00:46, 2026-10-25).

| Item | Commits | What |
| --- | --- | --- |
| 1a | `e181aa9`, `3b07eeb` | **Keyword ties.** `search_lexical` orders tied ranks by id on both stores. Recall's keyword pools use the new `search_lexical_ties`, which keeps every hit tied with the `PER_QUERY`-th, up to 3 × `PER_QUERY`, ordered by rank then id. Postgres does this with `FETCH FIRST n ROWS WITH TIES`. |
| 1b | `1a6e5d3` | **Dense taper.** `DENSE_WINDOW` = 50, with share `dense_weight(j) = rrf(j) · (1 − j/50)`. `similar_memories` fetches vectors in one `get_embeddings` call. |
| 1c | `f036aab` | **F71.** `recall_shown` table plus `GraphStore::claim_shown` (insert-if-absent, returns what was won). `on_cue` shows only what it claimed. Context key: subagent id, or `main#<n>` for the main session after n compactions. |
| 1d | `414b413`, `0e80d31` | **Stability tooling.** `recall-eval`/`work-recall-eval --embed`: the server's hook call (`handle_with` with the embedding model), run against a local or restored database. |
| 2 | `ee05ef5` | **Follows.** A `memory_followed` event when `fetch_content`, `related_context` or `get_symbol` (exact title) names a memory this context was shown. `recall-stats` and `GET /repos/{id}/recall/stats`. The revisit threshold is in `m3b-eval.md`. |
| 3 | `18d2699` | **Scope.** `client_host` in payloads; `nodes.applies_to`; the `set_memory_scope` tool and `add_note applies_to`; error cues skip memories scoped to other hosts. `classify-memories --scope`, `POST memories/scope`, `work-recall-eval --replay --client-host`. |
| 4 | `8900e5a`, `ccd7040`, `383ba6f`, `6aea617`, `ff2e01a`, `e369bab` | **Consolidation proposals** (ADR 0056): routes, CLI, the Gotchas workspace panel, and the SessionEnd spawn. Five follow-up fixes, found live, are listed under deviations. |
| 5 | `b8e04a5` | **Training parked.** ADR 0055; `end_session` documented as a no-op on this graph. |
| — | `91da126` | **CI fix.** `PostgresGraphStore::connect` applies the schema under an advisory lock (§7). |

**Docs:**
- `05-retrieval.md`: the dense taper, tie blocks, scope.
- `13-agent-integration.md`: never twice, follows, memory scope, SessionEnd.
- `03-data-model.md`: `recall_shown`, `applies_to`, `consolidation_proposals`.
- `07-mcp-and-rest-contracts.md`, `06-notes-and-consolidation.md`, `findings.md` (F71 closed), `m3b-eval.md`, ADR index.

**Deviations, each with its approval:**

- **1a: whole tie blocks instead of an id tiebreak alone.** The id tiebreak that was specced (`e181aa9`) left the work set at 5/20, below its 6/20 baseline, and made c09 a stable miss (gotcha 349916). The author approved keeping the tied block, 2026-10-24.
- **1b: no pre-measurement of alternative curves.** The council chair rejected it: choosing a curve by eval score would spoil the gate. The fallback was declared beforehand and wasn't needed.
- **1d: no scratch server.** The author chose `--embed` over the milestone 3c harness, so no account files were copied (2026-10-24).
- **Item 2: three tools, not four.** `explain_symbol` was dropped (the council's Simplifier). `get_symbol` counts only on an exact title match, because it takes a name, not an id.
- **Item 2: subagent follows are forwarded.** Subagents' agentops calls weren't sent to the server before. The three follow tools now are, which required the CLI reinstall.
- **Item 3: hosts are where Claude Code runs.** `applies_to` lists client hostnames (the author, 2026-10-24). In this repo every session runs on the laptop, so a scope naming the server or an SSH alias means "never on an error cue" (gotcha 354625).
- **Item 3: 14 scoped, not all proposals.** The backfill proposed 45 scopes; the author applied a curated 14 (§4).
- **Item 4: accepting reuses `supersedes`.** The sources become temporal, ending today, rather than progress as the spec said (the author, 2026-10-24).
- **Item 4: fixes found live:**
  - `ccd7040`: a failed LLM call stops the run and is still logged.
  - `383ba6f`: Groq requires every schema key to be listed (gotcha 364020).
  - `6aea617`: 4,096 output tokens per call, because gpt-oss reasons inside `groq_config`'s 512.
  - `ff2e01a`: a proposal overwrote one of its own sources (§5).
  - `e369bab`: the bulk Keep all button failed on proposals.
- **Pushes after the first.** The author approved the first push and the CLI reinstall, the `ccd7040` push with generation, and the `ff2e01a` push with the 54087 repair. `383ba6f`, `6aea617`, `e369bab` and `91da126` were pushed **without a separate ask**, as fixes blocking already-approved work.

## 2. Robustness

**Method.** On thedamnserver, the 10-20, 10-21 and 10-24 nightly dumps were restored into a scratch pgvector container (the production image digest). Every evaluation was a fresh `CREATE DATABASE ... TEMPLATE` copy and a fresh CLI process run with `--embed` and `--repeat 1`. The new code and `b6d57f1` (plus the `--embed` patch) were built the same way.
- **The harness reproduces milestone 3c:** old code scores 15/20 (MRR 0.506) on 10-20 and 12/20 (0.421) on 10-21, the same as `m3c-report.md` §2.
- **Additions:** the 26 memories are the gotcha/decision/note ids in 10-21 that aren't in 10-20 (they include 235645 and 160517). For each, its row and the edges whose other end exists were copied from 10-21 into a fresh 10-20 copy, ids preserved.
- **No dense timeouts:** none in 230+ runs, once the model was warmed (`0e80d31`).
- The scratch container and files were removed afterwards.

**The curve:** `dense_weight(j) = rrf(j) · (1 − j / 50)`.
- Decreasing, and 0 at the window edge.
- j=0: 0.0164 (unchanged); j=19: 0.0078; j=20: 0.0074.
- One new close neighbour at j≈20 now costs 0.0003 instead of 0.0125.
- A fixture of c20's shape moves the target 0 → 19 under the old step and stays within 2 places now (mutation-checked).

**Original cases, 5 fresh runs each:**

| Code | 10-20 | 10-21 |
| --- | --- | --- |
| `b6d57f1` | 15/20, MRR 0.506 | 12/20, MRR 0.421 |
| Taper + id tiebreak | 14/20, 0.459 | 14/20, 0.462 |
| Taper + no tiebreak (ablation) | 15/20 | 14/20 |
| **Shipped: taper + tie blocks** | **15/20, 0.454** | **15/20, 0.455** |

Ranks were identical in all 5 runs of every row. On the shipped code c20 is at rank 3 on both backups (old: 1, then 32) and c15 is at 5 (old: 4, then a miss). On the 10-20 copy, the cumulative trajectory of all 26 additions stays at 15/20 at every step.

**The 26 additions, one at a time, on 10-20:**

| Code | Worst drop of a seed target |
| --- | --- |
| `b6d57f1` | 32 ranks (c20, on 235645); 4 (c15, on 235645) |
| Shipped | **1 rank** (c15, on 235645) |

**Redrawn sets on the 10-24 backup (5 runs each), and live on production:**

| | Prompt (`recall-eval`) | Work (`work-recall-eval`) |
| --- | --- | --- |
| 3c baselines | 13/20 | 6/20 |
| `b6d57f1`, 10-24 | 13/20, MRR 0.449 | 6/20, 0.300 |
| Taper + id tiebreak, 10-24 | 14/20 | **5/20** (error cues 2 → 1) |
| Shipped, 10-24 | 15/20, MRR 0.454 | 6/20, 0.300 |
| Production, before accepting proposals | 15/20, MRR 0.450 | 6/20, 0.300 |
| Production, after accepting 7 | 15/20, MRR 0.456 | 6/20, 0.300 |

- **Latency** (production, after the deploy settled): p95 150 ms prompt and 127 ms work, at load ≈ 4. A p95 of 1.27–1.42 s was measured while a CI gate ran on the same host and is discarded.
- **F71**, live on production (`.context/m4/f71.py`, 5 trials of 8 simultaneous same-file reads, session ids `recall-eval-f71-*`):
  - `docs/engineering/findings.md`: 5 injections and 0 duplicates (milestone 3c: 20 and 15).
  - `agentops-graph/src/sqlite.rs`: 0 injections.
- **Held-out noise** (proxy). Today's replay of the 5 held-out sessions as the laptop host gave 458 pointers, 127 of them to memories the author labelled in milestone 3b.
  - A random 30 of those, scored by their memory's label: 11 useful, 19 neutral, **0 noise**. Over all 127: 1 noise (0.8%).
  - **This is the author's labels per memory, not a fresh scoring of these rows.**

## 3. Follow data so far

`agentops recall-stats --remote`, 2026-10-25 00:50:

```
| Source | Class | Shown | Followed | Rate |
| --- | --- | --- | --- | --- |
| file_cue | durable | 1 | 1 | 100.0% |
| prompt_seed | durable | 10 | 0 | 0.0% |

1 follows; 1 sessions shown memories, 1 followed one.
```

- **0 ordinary sessions.** The only session since the deploy is this milestone's own (meta); its one follow was the live check (memory 53999, file cue).
- Counting started at the deploy (`b8e04a5`, 20:01, 2026-10-24); earlier sessions have no `recall_shown` rows.
- The revisit threshold is 200 follows from at least 20 ordinary sessions (`m3b-eval.md`).

## 4. Scope

- **Backfill** (`agentops classify-memories --scope`). Candidate names: this hostname and 6 `~/.ssh/config` aliases.
  - 45 of 413 memories matched: 43 on `thedamnserver`, 2 on `github-personal`. The full set is in `.context/classify-memories.scope.full.json` (local).
  - About two thirds only mention the server in passing.
  - The author applied a curated 14: 12 about the server's deploy mechanics, plus the 2 SSH-alias ones. Reply: "Scoped 14 of 14 reviewed memories".
- **The spec's "4 known situational memories" are 4 rows of 3 memories:** 53975, 54101, and 104450 twice (from `to-label.jsonl`, scores 1–2). All 3 are among the 14.
- **Replay comparison** (5 held-out sessions, today's graph):

| | Pointers | Error-cue pointers | Situational memories on errors | Any scoped memory on errors | Scoped memories on file cues |
| --- | --- | --- | --- | --- | --- |
| No host | 459 | 106 | 2 | 7 | 1 |
| As the laptop host | 458 | 106 | **0** | **0** | 1 |

Error-cue totals don't fall, because each cue's slot goes to its next candidate. On today's graph only 2 of milestone 3c's 4 situational rows still appear without a host.

## 5. Consolidation

- **Batch dry run:** 106 merge candidates (cosine 0.85–0.92), written to `.context/consolidate.dry-run.md`. The author approved 16 of them.
- **Runs:** 13 logged as `consolidation_run` events, 19,715 input and 11,098 output tokens in all. The first 7 failed before writing anything useful: 4 on Groq's schema rule (0 tokens) and 3 on the 512-token cap.
- **Cost:** Groq, model openai/gpt-oss-120b. Logged USD is `null`, because `agentops_llm::pricing` has no rate for that model; the tenant's key is on Groq's free tier. **Per-run cost in dollars: unverified.**
- **Outcomes:** 10 valid proposals came out of 11 writes, and the model judged 10 candidates not worth a memory.
  - **The 11th write overwrote its own source.** The merge of 54087 and 54088 reused 54087's exact title, and `add_note` upserted onto it: the body was replaced and the node was set needs-review, with a proposal row naming itself.
  - **Fixed in `ff2e01a`:** proposal titles are made unique, and a self-sourced row can't be accepted, only closed.
  - **Repair** (approved; four separate changes): the row was closed; the body was restored via `add_note` from the 10-24 backup, and it matches the backup byte for byte (1,084 characters); curation was set back to Full. Gotcha 368727.
  - The pair was then regenerated as a new proposal, 373426.

| Proposal | Sources | Decision |
| --- | --- | --- |
| 364021 | 53981, 54019 | Rejected |
| 368718 | 54029, 104416 | Accepted |
| 368719 | 47152, 54101 | Accepted |
| 368720 | 54072, 54100 | Accepted |
| 368721 | 54031, 54056 | Accepted |
| 368722 | 53992, 54246 | Rejected |
| 368723 | 54262, 58354 | Accepted |
| 368724 | 79106, 83277 | Accepted |
| 368725 | 53963, 54037 | Rejected |
| 373426 | 54087, 54088 | Accepted |

- **Accepted: 7 of 10** (the spec asked for at least half).
- **Rejections, categorised by me from the sources' kinds and titles:** the author gave no reasons, so these are **unverified**.
  - 364021 and 368725: two related but distinct facts. A merge would blur which decision says what.
  - 368722: a general rule paired with one specific case of it.
- **Verified live through `POST /recall`,** prompted with a source's own title:
  - For accepted 368719 and 368721, neither source is recalled any more and the proposal is.
  - For rejected 364021, both sources are still recalled and the proposal is gone.
  - The proposals list is empty: nothing is waiting.
- **Evals after accepting:** prompt 15/20 (MRR 0.450 → 0.456), work 6/20, unchanged (§2).
- **The bulk button incident.** The author's first try was "Keep all" on the Needs review tab. The server refused all 10 (409) as designed, so nothing changed, but the page reported a vague failure. Fixed in `e369bab`: bulk actions skip proposals and say why.

## 6. The state of the requirement

| Outcome (cue-driven memory requirement) | State | Evidence |
| --- | --- | --- |
| A relevant memory reaches the context without a tool call, in normal and plan mode and in subagents | **Met** | `m1-report.md` checks 1–4; milestone 3b work-time recall in subagents; this session's own injections after deploy (e.g. 53999 on a Read) |
| A memory surfaces on its cue and not otherwise; injection stays inside a fixed budget | **Partly met** | Noise 0.8% by proxy (§2); one pointer per cue, 800 characters (`WORK_RECALL_BUDGET`). The per-session volume target from milestone 3b (median ≤ 3,000 characters) was not met there (17,672) and not re-measured here |
| Rules that must always hold are enforced or always present | **Met** | `m2-report.md`: 15 of 15 imported rules pinned, re-injected every prompt |
| MEMORY.md imported once, every entry routed, nothing silently dropped | **Met** for this repo | `m2-report.md`: 49 entries in = 49 records out; 2 to the review queue. Other projects not imported (out of scope here) |
| Session learnings become memories without asking: the agent writes during the session, and consolidation runs at session end | **Partly met** | Mid-session writes since milestone 1; consolidation proposals ship (§5), but a person accepts each one by design, and a live session-end run is **unverified** (the hook is installed with the new CLI; this session's own end will be its first) |
| A memory recalled and wrong gets flagged for review; one recalled and used gets stronger | **Open** | "Followed" is recorded (§3) but changes no weight; 1 follow so far. Nothing flags a memory as wrong |
| Measured: on a replay of the evidence sessions, the missed memory now appears before the step that went wrong | **Open** | The evidence transcripts were never attached (`m1-report.md`, check 5) |

**What comes next.** Reinforcement can't start on data: it needs 200 follows, and there is 1. So the choice is between the rules milestone and the second import pass. Two of the open or partly met outcomes depend on things no milestone has supplied:
- the evidence transcripts, for "measured";
- an explicit "wrong" signal, for review flags. The second import pass doesn't touch either.

## 7. Open issues, and what must not break

**Open:**
- **A superseded pin stays pinned.** Accepting 368718 superseded 104416, which is pinned. Pins ignore the memory class (`nodes_pinned` selects on prominence only), so 104416 is still injected on every prompt next to its merged replacement. Unpin it, or make supersedes unpin; neither was done.
- **The 80-row cutoff.** SQLite's `search_lexical` over-fetches 80 rows across all repos and kinds before filtering, which is still a cliff on a multi-repo local DB. Its tiebreak test can't fail without the fix, because FTS5 already returns rowid order (gotcha 349870).
- **Scope can't yet tell machines apart.** With one client machine, it only suppresses error cues (gotcha 354625).
- **Unpriced model.** `agentops_llm::pricing` has no rate for Groq's gpt-oss-120b, so consolidation runs log `usd: null`.
- **The live session-end consolidation run** is unverified.
- **CI deadlocks.** Parallel Postgres test stores could deadlock applying the schema (2 of 259 CI logs, including the gate for `e369bab`); fixed by an advisory lock in `91da126` (gotcha 382829). The flaky indexing test (F48) is unchanged.
- **No deploy gate for the laptop CLI.** The CLI was reinstalled by hand for this milestone.

**What must not break:**
- Milestone 3c's list: the `memory_written` exclusion, progress never recalled, the first cue per file per context, 1 pointer per cue, hooks never loading the model, and `add_note`'s `result:` line.
- From this milestone:
  - claims gate work-time injection, and compaction re-arms through `main#<n>`;
  - keyword pools keep whole tie blocks;
  - a waiting proposal is never recalled and can't be curated, only accepted or rejected;
  - accepting supersedes its sources;
  - a proposal never writes over an existing memory;
  - hook payloads carry nothing from the environment but `client_host`.

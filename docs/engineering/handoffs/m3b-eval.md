# Milestone 3b work-time recall evaluation

Numbers from `agentops work-recall-eval` (`agentops-cli/src/work_recall_eval.rs`). The cases live in `.context/work-recall-eval.jsonl`, and the replay labels live under `.context/work-recall-eval/`. Both are git-ignored, so no paths, error text or memory text from them appear here.

## Sizing

`agentops work-recall-eval --sizing`, 2026-10-20: Claude Code transcripts for this repo on the author's machine, modified in the last 30 days. Main sessions and subagent transcripts are counted separately. The counts cover this one machine. The server saw 241 sessions in the same window (F68), so other machines' sessions are missing here.

| Per transcript | Main (30): median / p95 / max | Subagent (408): median / p95 / max |
| --- | --- | --- |
| `Read` | 12 / 84 / 198 | 6 / 18 / 39 |
| Single-file Bash read (`cat`/`head`/`tail`/`sed -n`) | 4 / 28 / 31 | 0 / 3 / 14 |
| `Edit` | 26 / 130 / 267 | 0 / 0 / 0 |
| `Write` | 5 / 44 / 58 | 0 / 0 / 12 |
| `Grep` / `Glob` | 0 / 0 (1 call in total) | 0 / 0 |
| Failed tool calls | 11 / 28 / 29 | 0 / 3 / 11 |
| **Error cues** (failures that reach `PostToolUseFailure`) | 5 / 21 / 21 | 0 / 3 / 11 |
| **File cues** (in the repo, off the skip list) | 33 / 187 / 434 | 5 / 16 / 39 |
| Distinct files touched | 13 / 68 / 121 | 4 / 13 / 39 |
| All tool calls | 253 / 1,050 / 1,276 | 19 / 47 / 130 |

What this sets:

- **How often the hook fires.** About 40 cues per main session at the median and about 210 at p95. Subagents add about 5 each.
- **What a session may add.** At `WORK_RECALL_MAX_ITEMS` 3 and `WORK_RECALL_BUDGET` 800 characters per cue, injecting on every cue would add about 32,000 characters in a median main session. The 3,000-character median target means at most about 1 cue in 10 may inject. `INJECT_MIN_SCORE`, repeat suppression and `MAX_INJECTED_PER_CONTEXT` 30 have to produce that silence.
- **Searches go through Bash.** 1 `Grep` call and 0 `Glob` calls, against about 6,000 Bash `grep`/`rg`/`find` commands (from a one-off count). The spec's `search` event, scoped to Grep/Glob, records almost nothing here.
- **Reads go through Bash too.** In recent milestone sessions, `Read` calls are 4–12 per session. Single-file Bash reads count as file cues (the author's decision, 2026-10-20).
- **Failures that never reach the hook.** About 1 in 3 failures in main sessions: validation errors, permission or classifier denials, and the user declining a call (hooks reference: `PostToolUseFailure` doesn't fire for them).

## Replay sessions

Five past sessions, chosen 2026-10-20 and approved by the author. The ids are the first 8 characters of the session id.

| Session | Work | Tool calls (all contexts) | Contexts | File cues | Error cues |
| --- | --- | --- | --- | --- | --- |
| `9994add3` | Milestone 2 | 838 | 21 | 112 | 36 |
| `b222dd72` | Milestone 3a | 704 | 23 | 93 | 21 |
| `d58de736` | Engineering docs (docs-heavy) | 2,218 | 41 | 378 | 42 |
| `5431fe7b` | Agents and plan pipeline | 1,133 | 30 | 251 | 23 |
| `46354763` | Web app design (frontend contrast) | 705 | 21 | 185 | 23 |

## Cases

20 cases: 10 file cues and 10 error cues. They were drafted from the graph and from real failed tool calls on 2026-10-20, and approved by the author.

- **File cases.** Four are config files, two are docs or scripts, and four are code or frontend files. No target is `Affects`-linked to its file: the harness checks that and refuses such a case.
  - The check refused 4 drafted file cases (f06–f09), which were replaced with unlinked ones.
- **Error cases.** Each target is a gotcha that explains the error.
  - e07 was replaced: the drafted one was an auto-mode classifier denial, which never reaches `PostToolUseFailure`.
- **A hit** is a target within the first `WORK_RECALL_MAX_ITEMS` (3) candidates. **Latency** is the round trip from the author's machine to the server, 3 runs per case.

## Scores

| Point | Commit | Hit rate | File | Error | MRR | p50 / p95 ms |
| --- | --- | --- | --- | --- | --- | --- |
| Baseline: `Affects` lookup only (what `get_symbol` gives) | `f892bc3` | 0/20 | 0/10 | 0/10 | 0.000 | 22 / 45 |
| Baseline: cue text through prompt-time recall (`/recall`) | `f892bc3` | 11/20 | 7/10 | 4/10 | 0.442 | 97 / 140 |
| Baseline: same, error cases with real newlines (re-run) | `9679731` | 10/20 | 7/10 | 3/10 | 0.442 | 87 / 146 |
| 2. Cue resolution (error cues cleaned to empty, see below) | `87aae38` | 7/20 | 7/10 | 0/10 | 0.233 | 90 / 169 |
| 2. + empty-query guard, dense confidence rescaled | `9679731` | 10/20 | 6/10 | 4/10 | 0.375 | 106 / 262 |
| 2. + Affects dampened, gotcha boost capped, ties on rank (approved fix) | `b783df4` | 10/20 | 6/10 | 4/10 | 0.375 | 99 / 169 |

Target: at least 14/20, with at least 7/10 file cases and at least 7/10 error cases.

**The `Affects` baseline is 0/20 by construction.** The spec requires file cases whose target isn't `Affects`-linked, and an error names no file. So that baseline only confirms the cases are valid. The second baseline is the informative one: m3a's prompt recall fed nothing but the path or the error text.

**The file half of the case set is easier than intended.** Seven of the ten file targets carry the file's name in their title, so a keyword match on the path alone already ranks them in the top 3. Work-time recall has to beat 7/10 on file cases to show anything the path keyword doesn't already give. The error cases are where the prompt baseline is weak (4/10).

**Item 2's misses**, diagnosed on the server. The targets are in the candidate lists, but scores saturate in three ways:

- **Ties at 1.0.** A one-keyword file query (`docker-compose.yml`) scores every memory containing that word 1.0, and ties fall back to node id (f01, rank 4).
- **`Affects` floods.** A hub file reaches up to 50 `Affects`-linked memories, all at 1.0 (f08, f10). So does an error that names a symbol (e03).
- **The gotcha boost saturates.** A fully matched gotcha scores 1.5 and outranks a partly matched target (e01).

**First item 2 run.** The case texts had their newlines flattened to `|`. An error whose single line started with `Exit code N` cleaned to an empty query, and that empty query still reached generic memories by meaning at confidence about 0.85. Fixed in `9679731`, and the cases now keep the real newlines.

## Timing gate before item 3

Resolving one cue on the server takes about 85 ms at p50: the 106 ms round trip, less the about 22 ms the trivial `Affects`-only probe takes. On a hub file, the p95 is about 240 ms. Recomputing activation from the last `ACTIVATION_WINDOW` (20) cues on every hook call would cost about 20 × 85 ms ≈ 1.7 s at p50, against a 300 ms remote p95 budget. So each cue's candidate scores are stored once, as a `work_cue` event, and activation sums those.

**The approved scoring fix (`b783df4`) left the hit rate unchanged**, though it is live: a hub file's `Affects` memories now score 0.43 rather than 1.0. The ten misses fall into three groups:

| Why it misses | Cases |
| --- | --- |
| Not reached by any source | f10, e02, e04 |
| Tied with other memories that mention the same file or words, several of them arguably relevant too | f01, f08 |
| Reached but ranked low | f09, e01, e03, e05, e07 |

More tuning against 20 single-target cases would mostly fit these cases. The replay labels measure precision directly.

**Prompt-time `recall-eval` after `b783df4`:** 15/20, MRR 0.472, p95 117 ms, misses 3 ranking and 2 vocabulary. This holds the 3a contract of at least 15/20. MRR is below m3a's 0.506 (`f001b01`); the graph has gained notes since. `recall_seed_with`'s order is unchanged by design, and its tests check that.

## Replay (before labels)

`agentops work-recall-eval --remote --replay <5 sessions> --min-score T`. All contexts of each session: the main session and every subagent.

| Run | Commit | Threshold | Characters added per session | Pointers (main / all) | Most work-time pointers in one context |
| --- | --- | --- | --- | --- | --- |
| First | `cc9f272` | 0.4 | 48,576 – 198,111 | – / 1,757 | 106 (ceiling reset by compaction) |
| Re-reads counted once, ceiling spans compactions | `823bbf4` | 0.6 | 43,926 – 180,631 | 199 / 1,505 | 62 (error cues past the ceiling) |
| Same | `823bbf4` | 1.0 | 25,004 – 115,026 | 56 / 813 | 39 |

- **Subagents carry the volume.** 87% of pointers at 0.6 went to subagent contexts. There are 102 subagent contexts with at least one injection, at a median of 11 pointers each. Each subagent starts with an empty shown set.
- **Above 1.0 means convergence.** A single cue scores at most 1.0, so a memory needs two or more converging cues to pass a threshold above 1.0.
- **What 1.5 would give**, estimated from the 0.6 run by filtering on activation (the shown sets would differ, so this is approximate): about 340–3,600 characters per session, near the 3,000 median target. Nearly every error cue would then be silent, since an error cue alone tops out at 1.0.

The author labels a stratified sample before `INJECT_MIN_SCORE` is set: 79 pointers at 1.5 or above, 94 from error cues, and 100 random ones from 1.0–1.5.

## Revision 1: rerun and ablation (2026-10-20, deployed `52650fc`)

Rules 1–3 of revision 1: only a file's first cue per context injects (`00f4186`); a subagent stops at 8 pointers, errors included (`30ee906`); an error cue keeps only memories a keyword or a named symbol matched (`b4f2c4a`). Ablation switches: `52650fc`. All rows below come from the same deploy, so the "All" row is the post-rule-3 measurement.

### Cases

`agentops work-recall-eval --remote --repeat 3 --sources S`

| Sources | Hits | File | Error | MRR | p95 ms |
| --- | --- | --- | --- | --- | --- |
| Before revision 1 (`b783df4`) | 10/20 | 6 | 4 | 0.375 | 169 |
| All (rules 1–3) | **9/20** | 6 | 3 | 0.367 | 190 |
| Without `Affects` | 9/20 | 6 | 3 | 0.367 | 166 |
| Without path mentions | 9/20 | 6 | 3 | 0.375 | 219 |
| Without file-as-query | 9/20 | 6 | 3 | 0.367 | 116 |
| Without the resolver's `SimilarTo` hop | 9/20 | 6 | 3 | 0.367 | 213 |

- **Rule 3 cost one error hit.** e05 (rank 5 before) and e07 (rank 7) now reach nothing: their targets came by meaning only. No hit was lost elsewhere.
- **No source adds a hit on its own.** Without mentions, f01 gains (4 → 1) and f05 loses (1 → 5). Every other case keeps its rank under every switch. The file hits f02–f07 are reached by more than one source, so switching one off at a time doesn't show a single source's share. **Unverified:** what switching off mentions and file-as-query together would do. The `Affects`-only baseline is 0/20.

### Replay at 0.6

`agentops work-recall-eval --remote --replay <5 sessions> --min-score 0.6 --sources S`

| Sources | Characters per session (median, range) | Pointers (all / main / from errors) | Most in a subagent / in the main session |
| --- | --- | --- | --- |
| Before revision 1 (`823bbf4`) | – (43,926 – 180,631) | 1,505 / 199 / – | – / 62 |
| All (rules 1–3) | **38,659** (26,793 – 84,089) | 798 / 176 / 156 | 8 / 50 |
| Without `Affects` | 39,516 | 767 / 167 / 151 | 8 / 48 |
| Without path mentions | 39,723 | 734 / 164 / 158 | 8 / 50 |
| Without file-as-query | 31,125 | 737 / 167 / 148 | 8 / 44 |
| Without the hop | 37,517 | 792 / 178 / 158 | 8 / 51 |

### What each source adds (All minus Without)

| Source | Case hits | Characters (5 sessions) | Pointers |
| --- | --- | --- | --- |
| `Affects` (dampened) | 0 | −8,613 (others fill its slots) | +31 |
| Path mentions | 0 (f01 −, f05 +) | +2,548 | +64 |
| File as query | 0 | **+55,324** | +61 |
| Resolver's `SimilarTo` hop | 0 | +3,940 | +6 |

- **Rules 1–3 halved the volume** (1,505 → 798 pointers). The median is still 38,659 characters against the 3,000 target. The threshold has to do the rest.
- **The main session reaches 50** because errors still inject past its 30 ceiling. No subagent passed 8.
- **Prompt-time `recall-eval` is at 13/20** (MRR 0.446, two runs), below the 15/20 contract. c15 (rank 4 → 10) and c20 (2 → past 8) dropped. [Certain] No code reached by prompt recall changed apart from `b4f2c4a`'s refactor, which only adds a field. [Likely] Notes added to the graph since the 15/20 run changed the keyword rarity weights. Neither case's new top 10 contains a note newer than milestone 3a. **Open.**

### After removing the resolver's hop (`c27179c`, author's call), and the combined check

| Sources | Hits | File | Error | Characters, 5 sessions (median) | Pointers (all / main / from errors) | Most in a subagent |
| --- | --- | --- | --- | --- | --- | --- |
| All, no hop | 10/20 | 6 | 4 | 235,512 (39,017) | 796 / 178 / 170 | 8 |
| No hop, no file-as-query | 10/20 | 6 | 4 | 172,910 (30,545) | 720 / 158 / 158 | 8 |

- **File-as-query adds no hit even with the hop gone.** The only rank that moves is f09 (13 → unreached), already a miss. Removing it saves 62,602 characters (27%) over the five sessions.
- **The cases move by ±1 between runs with no code change on their path.** e05 went from unreached (`52650fc`) to rank 3 (`c27179c`) with the error path unchanged; one note (240297) was added in between. Its target now matches two keywords. [Guessing] It sits at the edge of the per-keyword candidate cut (`PER_QUERY`), so graph growth or ties in text ranking move it. 9/20 and 10/20 are the same result.

### Final source set: `Affects` and path mentions (`7774597`), and the labelling sample

| | Cases | File | Error | MRR | p95 ms | Characters, 5 sessions (median) | Pointers (all / main / from errors) | Most in a subagent |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `Affects` + mentions, rules 1–3 | 9/20 | 6 | 3 | 0.367 | 112 | 172,061 (30,648) | 715 / 158 / 140 | 8 |

- The replay ran twice and gave the same numbers both times.
- **Labelling sample:** 100 of the 715 pointers, stratified by the author's choice: 40 from error cues, 40 main-session file cues, 20 subagent file cues. Each group is drawn in a fixed pseudo-random order (`work-recall-eval --replay … --sample 100`, written to `.context/work-recall-eval/to-label.{jsonl,md}`). Errors and main-session pointers alone came to 256, so "all of them" and "about 100" couldn't both hold. Reweight each group's noise share by its size (140 / 116 / 459) to estimate it for all 715.
- **Prompt-time `recall-eval` is down to 12/20** (MRR 0.421). c09 dropped from rank 2 to past 8. No retrieval or graph code changed between this run and the 13/20 one (`git log 6c992d1..7774597` over those paths is empty). Only one note was added to the graph in between. [Likely] Graph growth wears prompt-time recall down at about one case per few notes. **Open.**

## Labels (revision 1 sample, 2026-10-21)

The author labelled all 100 rows, five at a time. Weighted by group size (140 / 116 / 459):

| Group (pointers) | Useful | Neutral | Noise |
| --- | --- | --- | --- |
| Errors (140) | 35% | 17.5% | 47.5% |
| Main-session file cues (116) | 75% | 12.5% | 12.5% |
| Subagent file cues (459) | 45% | 40% | 15% |
| **All 715** | **48%** | **31%** | **21%** |

**Activation doesn't separate noise from useful.** On errors, useful rows score 0.61–1.31 and noise rows 0.62–1.91. The threshold sweep, weighted:

| `INJECT_MIN_SCORE` | Pointers kept | Noise among kept | Useful kept |
| --- | --- | --- | --- |
| 0.6 (current) | 100% | 21% | 100% |
| 0.7 | 94% | 20% | 93% |
| 0.75 | 63% | 25% | 51% |
| 0.9 | 45% | 17% | 39% |
| 1.0 | 20% | 24% | 17% |

So the threshold stays at 0.6.

**What the noise is.** Nearly every noise row is a status-only memory, one that announces that something shipped, was implemented or was completed, or summarises a session or plan, with no cause, rule or warning. Such memories go stale as later builds land. The same memory was useful from a file it concerns and noise elsewhere in three cases. A title pattern fitted on these rows would drop 23 noise, 4 neutral and 1 useful row; it is an in-sample fit, so it isn't a measurement. Recorded as gotcha 253842. Revision 2 (ADR 0054) acts on it with memory classes.

## Revision 2, step 1: short work-time lines (`b3ce492`)

Work-time pointers lost their match reasons, and the header no longer names the file or the error. Same five sessions, 0.6, `Affects` + mentions:

| | Characters, 5 sessions (median) | Pointers |
| --- | --- | --- |
| Before (`7774597`) | 172,061 (30,648) | 715 |
| Short lines | 106,903 (18,235) | 734 |

−38% in total and −40% at the median. Shorter lines let a few more pointers fit `WORK_RECALL_BUDGET`, so the pointer count rose slightly.

## Revision 2: classes applied, and the held-out sample (2026-10-22)

After the backfill (30 memories classed `status`, now `progress`; title words only, since the tenant had no LLM credential), same five sessions at 0.6:

| | Cases | Characters, 5 sessions (median) | Pointers | Prompt-time `recall-eval` |
| --- | --- | --- | --- | --- |
| Short lines (`b3ce492`) | 9/20 | 106,903 (18,235) | 734 | 12/20 |
| + 30 classed memories (`8b6f2e2`) | 10/20 | 105,322 (17,672) | 706 | 13/20 |

Cases and `recall-eval` moved by one, within the drift recorded in revision 1. **The class barely moved volume:** each cue shows up to `WORK_RECALL_MAX_ITEMS`, so the next candidate fills a dropped memory's slot. The class lowers noise; only a cap lowers volume.

**Held-out sample.** 100 pointers, drawn only from memories never labelled before (`work-recall-eval --sample`, which now keeps the labelled file and excludes its memories): 418 of 706 pointers reached an already-labelled memory, so the sample covers the other 288. The author scored each 1–10 (7–10 useful, 4–6 neutral, 1–3 noise):

| Group | Rows | Useful | Neutral | Noise | Mean score |
| --- | --- | --- | --- | --- | --- |
| Errors | 40 | 17 | 18 | 5 | 6.0 |
| Main-session file cues | 32 | 13 | 16 | 3 | 5.9 |
| Subagent file cues | 28 | 13 | 12 | 3 | 6.0 |
| **All** | **100** | **43** | **46** | **11** | |

- **Progress memories never helped.** All 8 rows whose memory is (or now would be) `progress` scored 2–4, including two shown through an `Affects` link from the opened file. That is why the class became `progress` with no exception (ADR 0054 amendment).
- **Without them, noise is 6 of 92 (6.5%).** Four of the six are situational gotchas, tied to one machine or a rare setup (rsync to the server, the deploy setup, an SSH host alias), reached through one shared keyword on an unrelated error. A class doesn't describe them; a stronger match for situational memories might. Open.
- **Spec targets on this sample:** noise ≤ 30% and at least a third useful both hold. The median volume (17,672 against 3,000) does not.

## Final state (`684e36a`, 2026-10-22): 40 progress memories, no `Affects` exception

| Cases | File | Error | MRR | p95 ms | Characters, 5 sessions (median) | Pointers | Prompt-time `recall-eval` |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 8/20 | 5 | 3 | 0.317 | 127 | 106,452 (17,717) | 703 | 12/20, MRR 0.395 |

- **f05 is lost by design.** Its target (190997) is classed `progress`.
- **e04 is drift**: rank 3 → unranked, with its target unclassed.
- In `recall-eval`, c09 dropped from rank 3, which is drift (it flipped before). c17's target (104436) is `progress`, but c17 was already a miss.

**Who should class a memory.** The same 241 memories, the same prompt, both samples' labels:

| Judge | Useful hidden | Neutral hidden | Noise caught | Noise missed | Gotchas → progress |
| --- | --- | --- | --- | --- | --- |
| Title words (never a gotcha) | 0 | 7 | 25 | 5 | 0 |
| Groq `openai/gpt-oss-120b` + bodies | 3 | 10 | 26 | 4 | 14 |
| Haiku, titles only | 1 | 6 | 21 | 9 | 5 |
| Haiku + bodies | 5 | 14 | 27 | 3 | 19 |

The Haiku runs were subagents. The first reported success after reading titles only (20 tool calls for 241 memories), and one batch of the second ignored the bodies it fetched. Both were caught by checking tool-call counts, and recorded body lengths and opening text against the real memories. The batch was rerun. So the title words stay the only automatic classifier, and `classify-memories --llm` only proposes.

## Volume ceiling (`785d282`, 2026-10-22): 1 pointer per cue

The author replaced the 3,000-character median with the noise and useful targets plus `WORK_RECALL_MAX_ITEMS` 1.

| Cases | File | Error | MRR | p95 ms | Characters, 5 sessions (median) | Pointers |
| --- | --- | --- | --- | --- | --- | --- |
| 5/20 | 3 | 2 | 0.250 | 113 | 82,395 (12,479) | 453 |

- f02, f04 and e06 had their target at rank 2 or 3 in the `684e36a` run; recomputing that run at a cut-off of 1 gave the same 5/20 before deploying.
- Headers now make up about a third of the volume: one per injection, 453 injections.
- Label mix by memory (a proxy, joining each pointer with its memory's existing label): 56% useful, 37% neutral, 7% noise, against 58/35/7 at 3 per cue. 53 of the 453 pointers reached unlabelled memories.


## Milestone 3c: labels lead, cases guard (2026-10-23, `6ea8172`)

From here on, **labels are the primary measure of recall during work.** The noise and useful shares come from a labelled sample of replayed pointers. The 20 cases in `.context/work-recall-eval.jsonl` and `.context/recall-eval.jsonl` are a regression guard: a drop means something changed, but a score alone doesn't say whether recall got better.

**Redrawn cases.** Each case was checked against three rules for a recallable target: not `progress`, not `temporal` past its date, and not `needs_review`. Two failed, both because their target is classed `progress`. Each was replaced by a new case with the same cue type and a durable target. Every other case keeps its id.

| Set | Old case | Target | New case | Cue | Targets |
| --- | --- | --- | --- | --- | --- |
| Prompt | c17 | 104436 (`progress`) | c21 | plan-mode question on the Postgres store's Tokio runtime panic | 54054, 54028 (same bug, either counts) |
| Work | f05 | 190997 (`progress`) | f11 | file `docs/engineering/spikes/m1-hooks.md` | 62451 (mentions the path; no `Affects` link) |

**Baselines.** Both sets ran at `6ea8172` against a snapshot of the production graph taken 2026-10-23. Each set ran 5 times, every run on a fresh restore and a fresh server process, with `--repeat 5` inside each run.

| Set | Hits min / median / max | MRR min / median / max | p95 ms (range) |
| --- | --- | --- | --- |
| Prompt (`recall-eval`) | 13 / 13 / 13 | 0.445 / 0.445 / 0.445 | 81–99 |
| Work (`work-recall-eval`) | 6 / 6 / 6 (file 4, error 2) | 0.300 / 0.300 / 0.300 | 83–107 |

- **No run-to-run drift this time.** Every case's rank was identical across all 5 runs of each set. The ±1 seen before (gotcha 244815) didn't show up on this snapshot.
- **The +1 in each set is the swap.** c21 and f11 both hit at rank 1. On the same snapshot, the old c17 and f05 both still miss. Without the swaps the sets would score 12/20 and 5/20, matching the `684e36a` and `785d282` numbers above.

## Milestone 4: when to revisit reinforcement by use (2026-10-24)

Weights don't change from use yet. The live signal is now recorded as `memory_followed` (`agentops recall-stats`).
- **Revisit at:** at least 200 follows, from at least 20 ordinary sessions. Ordinary means not the labelling or analysis sessions, and not `recall-eval-*`.
- **Why that much:** in milestone 3c, follows fired on 2.8% of 575 live pointers (16 follows). At that rate, 200 follows is about 7,000 shown pointers, enough to compare follow rates per source and class with a few dozen follows in each.

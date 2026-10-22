# Milestone 3b handoff: recall during work

**Final, 2026-10-22.** It covers the milestone 3b spec, revision 1 (2026-10-20) and revision 2 (memory classes, 2026-10-21/22). Facts only, each tied to a commit, path or command output; anything not verified is marked **unverified**. **No memory text, prompts or paths from other projects:** this repo is public, so cases appear as ids and memories as node ids.

**Result: work-time recall is live and its noise is low. It misses the hit-rate and volume targets, and the volume target can't be met without a decision only the author can make.**

| Target | Result | |
| --- | --- | --- |
| Cue cases ≥ 14/20, ≥ 7/10 per half | **8/20** (file 5, error 3) | **Not met** |
| Replay median ≤ 3,000 characters per session | **17,717** (106,452 over 5 sessions, 703 pointers) | **Not met**: needs the decision in §6 |
| Noise ≤ 30% of what's injected | **11%** on the held-out sample; 6.5% without progress memories | Met |
| At least a third useful | **43%** | Met |
| Remote p95 < 300 ms | **127 ms** (cases, `--repeat 3`) | Met |
| Prompt-time `recall-eval` | **12/20**, MRR 0.395 (m3a ended at 15/20) | Below m3a; drift, open (§6) |

Two of the 10 → 8 lost cases are explained: f05's target is now a progress memory, which is never recalled by design (§2), and e04 is run-to-run drift (gotcha 244815).

## 1. What shipped

All on `main`, deployed by the push-to-main autodeploy. The last code commit is `684e36a`. Its gate passed: rust 1307, web 199, marketing 7.

### The spec's items (2026-10-20)

| Item | Commits | What |
| --- | --- | --- |
| 0 | `f892bc3`, `61ee618` | `agentops work-recall-eval` (`--sizing`, `--cases`, `--baseline`, `--replay`); 20 cases, 5 replay sessions |
| 1 | `1bbe7e3` | `file_read` and `search` events; nullable `session_events.agent_id` |
| 2 | `87aae38` … `309c8fe` | `resolve_cue`; `recall_scored_with`; `POST /repos/{id}/work-recall` |
| 3 | `9f3b431` | `work_cue` events; activation over the last 20 cues per context, decay 0.85 (ADR 0053) |
| 4 | `cc9f272`, `823bbf4`, `5fc0430` | `PostToolUse`/`PostToolUseFailure` injection; `POST /repos/{id}/work-recall/replay` |
| 5 | `859f029` | `AGENTOPS_WORK_RECALL=off` and `agentops connect --no-work-recall` |

### Revision 1 (2026-10-20/21)

| Rule | Commit | What |
| --- | --- | --- |
| 1. Only a file's first cue per context injects | `00f4186` | A re-read or later edit re-records activation and never resolves again. Compaction re-arms the main context. Ends the re-read drain (gotcha 235636) |
| 2. Subagent ceiling | `30ee906` | `SUBAGENT_MAX_INJECTED = 8` over a subagent's whole life, error cues included (author's reading) |
| 3. An error needs a keyword match | `b4f2c4a` | An error cue keeps only memories a keyword or an exactly named symbol reached. Meaning and the hop re-rank but never add (gotcha 235643) |
| Ablation | `52650fc`, `6c992d1` | `--sources no_affects|no_mentions|…`, then the table in `m3b-eval.md` |
| Sources removed (author's call) | `c27179c`, `7774597` | The resolver's `SimilarTo` hop and file-as-query. Neither added a case hit; file-as-query added 62k characters |
| Labelling sample | `ea3aefb`, `63bb2fb` | `--sample 100`, stratified 40/40/20 (errors / main files / subagent files) |

### Revision 2: shorter pointers and memory classes (2026-10-21/22, ADR 0054)

| Change | Commits | What |
| --- | --- | --- |
| Short work-time lines | `b3ce492` | `- [kind] Title (id N)` under one fixed header, with no match reasons. −40% at the median |
| Memory class on nodes | `8985cf3`, `638afa4` | `memory_class` (durable, progress, experimental, temporal) and `class_until`, set only by `set_memory_class`, kept across rescans. A migration renames stored `status` to `progress` |
| Recall applies the class | `b65bcbe`, `638afa4` | Every path (prompt seeds, subagent seeds, errors, files) drops `progress` always and `temporal` after its date. `experimental` is labelled |
| Classed at write time | `1b68d0c`, `8b6f2e2` | `add_note` takes `memory_class`, `class_until`, `supersedes`; otherwise title words class it (never a gotcha). The `set_memory_class` MCP tool fixes a wrong class |
| Backfill | `4c52e89`, `5463a73`, `684e36a` | `agentops classify-memories` proposes, the author reviews, `--apply` writes exactly what was reviewed. `--llm` (opt-in) asks Anthropic, else Groq, as proposals only |
| Held-out sampling | `3d2b2e9` | A new sample keeps the labelled file and never redraws a labelled memory |

**Backfill applied:** 40 memories are classed `progress` (30 by title words, then 10 of 39 LLM proposals the author approved). 3 the words flagged were held back by the author.

### Deviations from the spec, and who approved them

| Deviation | Approved by |
| --- | --- |
| Activation sums stored `work_cue` events instead of recomputing from reads (about 1.7 s per hook call otherwise) | The author, 2026-10-20 (timing gate) |
| A nullable `agent_id` column, not a new table; the off switch is an env var forwarded in the payload | The author (plan) |
| A re-read counted once in activation, and the ceiling spanning compactions (`823bbf4`). Not approved at the time | **Superseded by revision 1, rule 1**, which the author approved; the ceiling spanning compactions stays |
| Rule 2 read as a hard cap of 8 for the subagent's whole life, errors included; rule 3 read as "a keyword or an exact symbol name" | The author, 2026-10-20 (planning) |
| **Two cue sources removed**: the resolver's `SimilarTo` hop and file-as-query. This supersedes the spec's decision that a doc with no `Affects` link uses the file itself as the query; such a doc now reaches memories only through mentions of its path | The author, 2026-10-21 (after the ablation) |
| **`INJECT_MIN_SCORE` left at 0.6.** The spec expected the labels to set it; the sweep showed no threshold separates noise from useful (§2) | The author, 2026-10-21 |
| **Memory classes added** (ADR 0054), with `progress` never recalled, the `Affects` exception dropped after the held-out labels, and the LLM judge opt-in | The author, 2026-10-21/22 |
| **The volume target is not met.** Classes cut noise, not volume: the next candidate fills a dropped memory's slot | **Open: the author's decision** (§6) |

## 2. Evaluation

### Cases

20 cases (10 file, 10 error), 3 runs each. A hit is the target in the first 3 candidates. The cases are in `.context/work-recall-eval.jsonl` (git-ignored).

| Stage | Commit | Hits | File | Error | MRR | p95 ms |
| --- | --- | --- | --- | --- | --- | --- |
| Baseline: prompt recall on the cue text | `9679731` | 10/20 | 7 | 3 | 0.442 | 146 |
| Spec items done | `b783df4` | 10/20 | 6 | 4 | 0.375 | 169 |
| Revision 1 rules | `52650fc` | 9/20 | – | – | – | – |
| Revision 1 final sources | `7774597` | 9/20 | 6 | 3 | 0.367 | 112 |
| Revision 2, 30 memories classed | `8b6f2e2` | 10/20 | 6 | 4 | 0.383 | 112 |
| **Final** (40 progress, no `Affects` exception) | `684e36a` | **8/20** | 5 | 3 | 0.317 | 127 |

- **f05 lost by design.** Its target (190997) is a milestone summary, now `progress`. The case set was drawn before the class existed, so it treats a progress memory as the answer.
- **e04 lost to drift** (rank 3 → unranked; its target isn't classed). Runs swing ±1 with no code change (gotcha 244815).
- **Not tuned against the cases**, as the revision asks.

### Replay

The same 5 sessions, every context, `INJECT_MIN_SCORE` 0.6.

| Stage | Characters (median) | Pointers |
| --- | --- | --- |
| Spec items done (`823bbf4`) | 43,926 – 180,631 per session | 1,505 |
| Revision 1 rules | – | 798 |
| Revision 1 final sources (`7774597`) | 172,061 (30,648) | 715 |
| Short lines (`b3ce492`) | 106,903 (18,235) | 734 |
| **Final** (`684e36a`) | **106,452 (17,717)** | **703** |

- Short lines account for nearly all the cut from 30,648 to 17,717.
- Classes removed about 30 pointers, but `WORK_RECALL_MAX_ITEMS` refills each cue's slots from the next candidates. So **classes lower noise, not volume.**
- 3,000 characters at the current line length is about 25 pointers per session, against 140 today.

### Labels

Two samples, both labelled by the author (`m3b-eval.md`, "Labels" and "Revision 2"):

| Sample | Rows | Useful | Neutral | Noise |
| --- | --- | --- | --- | --- |
| Revision 1 sample (`63bb2fb`), weighted by group size | 100 | 48% | 31% | 21% |
| Held-out sample (`3d2b2e9`), scored 1–10, memories never labelled before | 100 | 43 | 46 | 11 |
| Held-out without progress memories | 92 | 43 | 43 | 6 |

- **Activation doesn't separate noise from useful.** Both span 0.6–1.9 on errors. A sweep from 0.6 to 1.0 removes useful memories as fast as noise.
- **Progress memories never helped.** All 8 in the held-out sample scored 2–4, including two shown through an `Affects` link.
- **The remaining noise is mostly situational gotchas**: 4 of 6, tied to one machine or a rare setup and reached through one shared keyword (§6).

### Choosing who classes a memory

On the 241 memories both LLMs judged, with the same prompt, against both samples' labels. "Hidden" means classed `progress`.

| Judge | Useful hidden | Neutral hidden | Noise caught | Noise missed | Gotchas → progress |
| --- | --- | --- | --- | --- | --- |
| **Title words** (never a gotcha) | **0** | 7 | 25 | 5 | **0** |
| Groq `openai/gpt-oss-120b`, with bodies | 3 | 10 | 26 | 4 | 14 |
| Haiku, with bodies (verified by body text) | 5 | 14 | 27 | 3 | 19 |

- **Reading the body pushes a model toward `progress`**: a gotcha's body narrates the fix. So title words are the only automatic classifier, and an LLM only proposes (`--llm`).
- The title words were chosen from the first sample, so their 0 is partly in-sample.

### Manual session (2026-10-20, before the revisions)

These results stand as found, but revision 1 changed what most of them describe:
- The re-read drain (gotcha 235636) is fixed by rule 1.
- The error-by-meaning noise (gotcha 235643) is fixed by rule 3.
- The bulky "similar to" trailers are gone with the short lines.

Still true from that session:
- **Remote `PostToolUse` p95 was 298 ms over 40 calls through the installed binary.** That predates the cuts; the cases now measure 127 ms. The installed-binary latency was **not remeasured**.
- **Local `PostToolUse` latency: unmeasured.**
- **Parallel-call duplicates (F71): unmeasured.**
- **Recall of a memory just written** (235646): still possible (§6).

## 3. Final parameter values

| Constant | Value | Supported by the evaluation? |
| --- | --- | --- |
| `INJECT_MIN_SCORE` | 0.6 | Yes, as the floor: no higher value improves the noise/useful trade (sweep, §2) |
| `WORK_RECALL_MAX_ITEMS` | 3 | The case metric's cut-off. It decides volume, since freed slots refill (§6) |
| `WORK_RECALL_BUDGET` | 800 characters | Not binding now that lines are short |
| `MAX_INJECTED_PER_CONTEXT` | 30 | Main session; errors may exceed it |
| `SUBAGENT_MAX_INJECTED` | 8 | Revision 1, rule 2; errors included. No subagent passed it in the replay |
| `ACTIVATION_WINDOW` / `ACTIVATION_DECAY` | 20 / 0.85 | Not tuned (ADR 0053) |
| `MENTION_PATH_SCORE` / `MENTION_NAME_SCORE` | 0.9 / 0.7 | Chosen by hand |
| `ERROR_GOTCHA_BOOST` | ×1.5, capped at 1.0 | The cap is supported (`b783df4`) |
| `AFFECTS_HUB` | 20 | Supported |
| `PROGRESS_WORDS` | title phrases plus "Session N" | Single words like "session", "plan" and "milestone" were rejected after testing against all 390 titles |

## 4. Interfaces milestone 4 will reuse

`agentops-graph`:

```rust
pub enum MemoryClass { Durable, Progress, Experimental, Temporal }   // "status" reads as Progress
impl MemoryClass { pub fn recallable_at(&self, today: &str, until: Option<&str>) -> bool; pub fn label(&self) -> Option<&'static str>; }
pub fn utc_today() -> String;                                         // "YYYY-MM-DD"
// Node gains memory_class, class_until.
fn set_memory_class(&self, repo: &str, node_id: i64, class: MemoryClass, until: Option<&str>) -> Result<()>;  // GraphStore
```

`agentops-retrieval`:

```rust
pub enum WorkCue { File { path: String }, Error { text: String } }   // serde tag "kind"; the heading is gone
pub enum CueSources { All, AffectsOnly, NoAffects, NoMentions }
pub struct WorkCandidate { pub pointer: RecallPointer, pub score: f64 }
pub struct RecallPointer { pub node_id: i64, pub kind: NodeKind, pub title: String, pub reason: String, pub memory_class: MemoryClass }
impl RecallPointer { pub fn render(&self) -> String; pub fn render_short(&self) -> String; }
pub fn resolve_cue(store, repo, cue: &WorkCue, sources: CueSources, embedder) -> Result<Vec<WorkCandidate>>;
pub fn recall_scored_with(..) -> Result<Vec<(RecallPointer, f64)>>;   // signature unchanged
```

`agentops-mcp`:
- `on_cue`, `on_prompt`, `replay`, `context_events` and `shown_ids` are as in the draft.
- Also `WORK_RECALL_HEADER`, `SUBAGENT_MAX_INJECTED`, and `apply_memory_class`.
- MCP tools: `add_note` (`memory_class`, `class_until`, `supersedes`) and `set_memory_class`.

`agentops-notes`: `heuristic_class(kind, title) -> (MemoryClass, bool)`. `agentops-llm`: `classify_memory_class`, `groq_config`.

Routes:
- `POST /repos/{id}/work-recall`
- `POST /repos/{id}/work-recall/replay` (`min_score`, `sources`)
- `POST /repos/{id}/memories/classify` (`offset`, `limit`, `llm`; writes nothing)
- `POST /repos/{id}/memories/class` (`assign`; refuses the whole batch if any entry is bad)

CLI:
- `agentops work-recall-eval --remote [--repeat N] [--sources …] [--replay S… [--min-score T] [--sample N]]`
- `agentops classify-memories [--llm] [--per-minute N] [--retry-failed] [--apply]`

## 5. Inputs for reinforcement and consolidation

**Was an injected memory used?** From the draft, over the 0.6 replay: 9 of 1,505 injections were followed by a call naming the id, and the 5 sessions made 36 `fetch_content` calls in total. Following a pointer is too rare to learn from.

**Milestone 4 must measure two more signals**, counted **after the injection only**, over the revision 2 replay (`.context/work-recall-eval/replay-1792699875.json` and its labels file):
1. **An edit to a symbol the injected memory `Affects`**, in the same context, after the injection. Needs the memory's `Affects` edges joined with the context's edit calls.
2. **The memory's title, or a distinctive phrase from it, in the agent's own text** after the injection, in the same context. Needs the titles of the injected memories (in the labels file) and the transcript's assistant text.

Neither is measured yet.

**Was it contradicted?** No revert-like git commands in the 5 sessions. A later note on the same symbols, and user corrections, are **unmeasured**.

**Consolidation:**
- `end_session`'s embedding consolidation is still manual and repo-wide.
- `work_cue` and `recall_injected` now give per-session structured ids to narrow it.
- The LoRA adapter is still unused (F29).

## 6. Open issues

**The volume target needs your decision.** At 703 pointers and a median of 17,717 characters, the 3,000 target means about 25 pointers per session, about 1/6 of today. Labels put 43% of pointers at useful, so any cut that size drops useful memories. Options:
1. **Cap pointers per session** (main plus subagents), e.g. 25. That meets the target and drops useful memories in order of activation.
2. **Lower `WORK_RECALL_MAX_ITEMS`** from 3 to 1 per cue. It roughly thirds the volume and keeps each cue's best match. It still won't reach 3,000 alone.
3. **Replace the volume target** with the noise and useful targets (both met) plus a looser ceiling you set.

**New since the draft:**
- **Situational gotchas are the remaining noise.** 4 of the 6 noisy held-out rows are true but tied to one machine or a rare setup, reached through one shared keyword. A class can't describe them; a stronger match for such memories might.
- **The case and recall-eval sets contain progress targets**: f05 (190997) and c17 (104436). They now measure something recall deliberately won't show. Re-draw or re-target those cases.
- **151 memories have no LLM proposal.** Groq's free tier hit its token caps (gotcha on the 8k TPM limit). That's fine by the decision above, since an LLM only proposes.
- **Prompt-time `recall-eval` is 12/20**, down from m3a's 15/20, with no retrieval change between most runs. [Likely] Graph growth shifts rarity weights. Open since revision 1.
- **Splitting long memories** into smaller, targetable ones. It adds pointers rather than removing them, so it's a precision question for milestone 4, measured against the noise labels, not the volume target.
- **docbrain-ingest's live next.js test can fail the deploy gate** (gotcha recorded); a re-push retries it.

**Still open from the draft:**
- F70 (no unique index on a note's natural key) and F71 (parallel-call duplicates).
- A recall error skips the large-output capture in `post_tool_use`.
- `hooks_never_load_the_model` doesn't cover `posttooluse`/`posttoolusefailure`.
- A memory written earlier in a session is recalled back into it (235646).

**Milestone 4 must not break:**
- `recall_seed_with`'s order and the `recall_injected` format `ids 1,2,3`.
- `None` `agent_id` meaning the main context; `context_reset` cutting the main context only.
- Injection off with recording on when `agentops_work_recall: false`.
- The class rules in `MemoryClass::recallable_at`, and gotchas never classed `progress` automatically.

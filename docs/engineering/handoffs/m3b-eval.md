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

Target: at least 14/20, with at least 7/10 file cases and at least 7/10 error cases.

**The `Affects` baseline is 0/20 by construction.** The spec requires file cases whose target isn't `Affects`-linked, and an error names no file. So that baseline only confirms the cases are valid. The second baseline is the informative one: m3a's prompt recall fed nothing but the path or the error text.

**The file half of the case set is easier than intended.** Seven of the ten file targets carry the file's name in their title, so a keyword match on the path alone already ranks them in the top 3. Work-time recall has to beat 7/10 on file cases to show anything the path keyword doesn't already give. The error cases are where the prompt baseline is weak (4/10).

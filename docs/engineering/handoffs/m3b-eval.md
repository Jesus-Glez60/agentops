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

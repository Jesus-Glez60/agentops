# Milestone 5 follow-up handoff: the open issues, one pass

**2026-11-05.** Covers the one-pass plan made from `m5-report.md` §7 (the author chose all four groups: recall robustness, an action cue, small fixes, milestone 4 carryovers). Facts only, each tied to a commit or a command's output; anything not verified is marked **unverified**. Client projects by alias only.

**Result: four of the six groups shipped and hold live. The two that were meant to close milestone 5's failed gates did not:**
- **c09:** both versions of the recall fix traded one eval for another, so the change was reverted;
- **E2:** the command cue missed E2's memory and injected only noise, so it is off by default.

| Done when | Result | |
| --- | --- | --- |
| Prompt eval ≥ 15/20, ranks identical in 5 processes | 14/20, MRR 0.439, identical (back to the pre-batch value after the revert) | **Not met** |
| Work eval ≥ 6/20 in 3 runs | 6/20, 6/20, 6/20 | Met |
| Adding note 411710 moves c09 ≤ 3 ranks (Postgres) | c09 at 20; the two fixes reached 6 but were reverted | **Not met** |
| Prompt-eval noise no higher than before | Unchanged from the baseline (136 non-target pointers in 150) | Met |
| E2 surfaces 104449 before the corrected step via a command cue | Rank 16 for E2's command query; a cue shows 1 | **Not met** |
| Each item shipped with tests, verified live where possible | §1 | Met, except the DNS alert (waiting on the author) |
| Prompt-hook p95 < 300 ms remote | 202 ms (p50 158 ms, n = 200, `ac5f535`); PreToolUse with a classed command: p95 123 ms (n = 50) | Met |

## 1. What shipped

| Step | Commit | What | Live check |
| --- | --- | --- | --- |
| 0 | — | Probe: `PreToolUse` `additionalContext` reaches the model | A headless `claude -p` run quoted the marker word back (note 421339) |
| 1 | `5fadd79` | **CLI protocol gate.** Remote hooks send `cli_protocol`. At `sessionstart` the server adds a reinstall `systemMessage` when it's behind (missing counts as 0). `CLI_PROTOCOL` is 2. | Protocol 0 and 1 warn, 2 doesn't |
| 2 | `e05505a` | **SQLite keyword search** filters repo and kind in the query: a small repo's hits are no longer lost behind 80 rows from other repos | Test fails on the old code (100 vs 3 rows) |
| 3 | `345590b`, `35badae`, reverted in `196980c` | **True keyword matches** in recall ranking (see §2) | Reverted |
| 4 | `65a1f8e` | **migrate-graph** copies memory class, scope, recall claims, consolidation proposals and not-worth verdicts. Every id is mapped (inside candidate ids too); a row whose node wasn't migrated is skipped, and `shown_at` resets. Its Postgres tests fail instead of skipping when `AGENTOPS_TEST_DATABASE_URL` is set. | PG test ran on CI |
| 5 | `06af3df`, `ac5f535` | **Command cues** (local server, deploy, remote) on `PreToolUse`; **off unless `AGENTOPS_COMMAND_CUES=on`** (§3) | p95 123 ms over 50 calls; off in production: a classed command posted to `/hooks/pretooluse` returns `{"output":null}` (`ac5f535`) |
| 6 | `df0b85e` | **Personal seeds** keep keyword matches only; the **memory-only registration message** says what works | "Proceed" now draws 0 personal pointers (it drew 2 before) |

`a060f12` from milestone 5 shipped with the first push of this batch.

**Pushes:** the batch was pushed with the author's approval (2026-11-04). `35badae`, `196980c` and `ac5f535` were pushed as fixes to it: `35badae` without a separate ask, the other two after the author chose the revert and the switch.

## 2. Recall robustness (c09): what was tried

Cause, from gotcha 416490: with 3 or more keywords a memory must match 2, and a match counted only if the memory survived that keyword's pool cut. One new note using "every" (411710) pushed c09's target out of the "every" pool.

| Version | Prompt eval | Work eval | c09 deep rank |
| --- | --- | --- | --- |
| Before the batch | 14/20, MRR 0.439 | 6/20 | 20 |
| v1: credit each pool member with every query keyword it contains | **15/20**, MRR 0.418 | 4/20 (e08, e09 lost: common words in long error text reordered the top) | 6 |
| v2: credit only to rescue a memory short of the minimum | 14/20, MRR 0.407 (c20 from 3 to 9: rescued memories took full strength) | 5/20 | 6 |
| After the revert (live) | 14/20, MRR 0.439 | 6/20 | 20 |

Each measured in 5 processes (identical ranks) and 3 work runs. **Lesson** (gotcha 430970): granting keyword *strength* from a word check moves both eval sets. And deploy-then-measure on production costs a deploy per try and invites fitting 20 cases. **Next:** milestone 4's restored-database harness, both evals offline, before any ranking change is deployed. The c09-shape and stability tests are kept as `#[ignore]`d tests that name the open issue.

## 3. Command cues (E2)

- **Classes:** local server (`serve*` subcommands, `cargo run`, `npm run dev`, requests or binds to localhost), deploy (`git push`, `rsync`/`scp` to `host:`, `kubectl apply`…), remote (`ssh host command`). The author chose all three.
- **Every `&&`/`;`/`|` segment counts.** `echo`, `cd` and `grep` never classify.
- **Mechanics:** one keyword-only pointer, the first per class per context, never blocking.
- **E2's real steps** were `agentops serve-api --addr 127.0.0.1:…` and `curl` calls to `127.0.0.1` (transcript read locally).
- **For that command's query,** "agentops serve-api" plus `local localhost server run serve`, 104449 ranks **16**. Its title shares "local" and "server" with the vocabulary, which wasn't enough.
- **Noise:** across the four evidence replays, command cues injected 5 pointers, and one more came up in a live session. All 6 are [Likely] noise by title (unlabelled): `/mcp` auth, the backend merge, device-authorization login, a ConnectionStore guard, rsync excludes.
- **Decision** (the author, 2026-11-05): off by default, behind a switch. The CLI still forwards classed calls (p50 86 ms), so turning cues on needs no reinstall. Replays still evaluate them.

## 4. Evidence replays, re-run (2026-11-05, today's graph)

| Case | Result | Path |
| --- | --- | --- |
| E1 | Recalled before the step | pin (104413) |
| E2 | **Not recalled** | none: command cues injected 4 other memories in the replay |
| E3 | Recalled before the step | prompt seed, rank 5 |
| E4 (weak) | Recalled before the step | pin (104410) |

Unchanged from milestone 5 (`m5-report.md` §4).

## 5. Open issues

- **c09 and the stability limit:** open, with a method chosen (§2).
- **Command cues:** off. A redesign must improve the query (for example, weighting the class vocabulary over the program name) and be measured on the replays before it's switched on.
- **DNS alert:** waiting on the author to create a Kuma push monitor and supply its URL. The script change is staged as in milestone 5 (gotcha 411710).
- **Personal pins in AgentOps** don't all fit the 3,000-character budget (406925 is cut and named, by design).
- **From milestone 5, still the author's:** the needs-review queue (2), the 11 LLM-pinned rules, the old deSEC token, client ports, and the "wrong" signal milestone.

**What must not break:** milestone 5's list, plus:
- a CLI behind the server is warned at session start;
- SQLite keyword search never filters after a global limit;
- migrate-graph never carries an unmapped id;
- command cues stay off unless the switch is set;
- personal seeds are keyword-only.

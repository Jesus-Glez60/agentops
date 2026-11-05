# Milestone 5 follow-up handoff: the open issues, one pass

**2026-11-05.** Covers the one-pass plan made from `m5-report.md` §7 (the author chose all four groups: recall robustness, an action cue, small fixes, milestone 4 carryovers). Facts only, each tied to a commit or a command's output; anything not verified is marked **unverified**. Client projects by alias only.

**Result: of the four groups the author chose, two shipped as intended (small fixes, milestone 4 carryovers); by step, four of the six hold live. The two groups meant to close milestone 5's failed gates did not:**
- **c09:** both versions of the recall fix traded one eval for another, so the change was reverted;
- **E2:** the command cue missed E2's memory and injected only noise, so it is off by default.

| Done when | Result | |
| --- | --- | --- |
| Prompt eval ≥ 15/20, ranks identical in 5 processes | 14/20, MRR 0.439, identical (back to the pre-batch value after the revert) | **Not met** |
| Work eval ≥ 6/20 in 3 runs | 6/20, 6/20, 6/20 | Met |
| Adding note 411710 moves c09 ≤ 3 ranks (Postgres) | c09 at 20; the two fixes reached 6 but were reverted | **Not met** |
| Prompt-eval noise no higher than before | Unchanged from the baseline (136 non-target pointers in 150) | Met |
| E2 surfaces 104449 before the corrected step via a command cue | Rank 16 for E2's command query; a cue shows 1 | **Not met** |
| Each item shipped with tests, verified live where possible | §1 | Met; the DNS alert's down path is unverified (§5) |
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
- **DNS alert:** live since 2026-11-05 (note 445396). Each updater run pushes to the Kuma monitor `desec-dyndns`: up on `good`/`nochg`, down on anything else, and down after 20 minutes without a push (two missed runs). Kuma recorded `up | good` at the install run. **Unverified:** the down path and notification delivery (test: stop `desec-dyndns.timer` for 25 minutes).
- **Personal pins in AgentOps** don't all fit the 3,000-character budget (406925 is cut and named, by design).
- **From milestone 5, still the author's:** the needs-review queue (2), the 11 LLM-pinned rules, the old deSEC token, client ports, and the "wrong" signal milestone.

**What must not break:** milestone 5's list, plus:
- a CLI behind the server is warned at session start;
- SQLite keyword search never filters after a global limit;
- migrate-graph never carries an unmapped id;
- command cues stay off unless the switch is set;
- personal seeds are keyword-only.

## 6. Wrap review (2026-11-05)

The whole milestone (`b27428a..fa2b399`, 34 files, already deployed) was reviewed after the fact by the five-persona council, a chair and a reuse audit. **Verdict: clear with changes; no rollback.** The review ran after deploy, which is the order gotcha 53914 warns against. Summary in note 450305.

**Fixed** (one commit, `agentops-test-remote` OVERALL PASS, 1,382 Rust tests):

| Finding | Fix | Test |
| --- | --- | --- |
| Personal seeds could lose a keyword match, **live**: `recall_lexical_with` filtered after its limit, so a memory reached by meaning alone took one of the 2 places and was then dropped (gotcha 450303) | Filter, then take the limit; command cues use it too | `a_lexical_limit_counts_keyword_matches_only_even_when_meaning_ranks_others_first`, which fails on the old line |
| migrate-graph copied sources digests that hash the old node ids, so after a migration rejected proposals reopened and not-worth pairs were judged again (gotcha 450304) | A digest that still matches its sources is recomputed on the target; a stale one is copied as it is. Candidate ids come from `Candidate::merge`/`distill` | The migrate test runs `unblocked` after migrating (Postgres; it ran, no SKIPPING) |
| import-memory treated any bullet with a `](x.md)` link as an index line, dropping it and its sub-bullets, and the count check used the same rule | One predicate, `index_target`: only a bullet that opens with the link | `a_bullet_that_only_mentions_a_file_is_an_entry_not_an_index_line`. **No real import was affected:** none of the 22 MEMORY.md files or 18 backups has such a bullet |
| On SQLite, personal recall would read and write the work repo's `graph.db` (latent: production is Postgres) | Personal memories join only on the shared Postgres store | none (no SQLite hook-route test exists) |
| Text | The reinstall message works outside an AgentOps checkout; `--no-work-recall` names `settings.local.json` for guest connects; `local_semantic_search`; "Affects links" explained; misplaced doc comments; the PreToolUse module doc lists its denial; `AGENTOPS_COMMAND_CUES` documented as a server-side switch; dead `contains_keyword` removed | — |
| The Postgres wipe test didn't check that another repo's not-worth rows survive (gotcha 54241) | Asserted | `wipe_repo_removes_only_its_own_llm_and_session_usage` |

**Reports corrected** in `m5-report.md`:
- requirement rows "reaches the context" and "rules always present" downgraded;
- the eval-session deviation disclosed;
- E4's timing explained;
- the unpin commit cited correctly;
- this report's summary line.

**Not fixed, deliberately:**
- **Error cues** filter after their limit, the same defect as the personal seed. Moving it changes the work eval, so it waits for the offline harness.

**Deferred:**
- dedupe the command-cue redaction (`hooks.rs`/`work_recall_eval.rs`) and `push_personal_seed`/`push_seed`;
- tests for `personal.rs`, `import-memory --personal` and the command arm of `resolve_cue`;
- reserve `personal` as a connection id;
- a size bound on `POST /memories/classify-kind`;
- the "no git remote" wording in `indexing.rs`;
- a `--min-hits` exit code on the evals.

**The author's to decide:**
- personal memories reach client repos with no per-repo opt-out;
- employment, personal-memory and home-network details in these public reports;
- inline import entries are keyed by line number, so a re-import after edits duplicates them;
- `memory-off --user` has no undo.

**Unexplained:** a reviewer saw a "create a Claude Docs document" instruction inside a graph search hit. [Guessing] It is a session's connector instructions captured in a stored tool output; not yet checked.

# Milestone 5 handoff: second import pass and evidence

**Final, 2026-11-04.** Covers the milestone 5 spec. Facts only, each tied to a commit, a script or a command's output; anything not verified is marked **unverified**. **No client names, memory text, prompts or paths from other projects:** projects appear as aliases (p01–p21, `user-level`), whose real names live only in the git-ignored `.context/m5/projects.local.md`. AgentOps' own memories appear as node ids.

**Result: every personal project and the user-level memories are in AgentOps, personal memories recall in other repos, and the evidence replays measured the requirement: 3 of 4 corrected memories now reach the context before the step that went wrong. The "no regressions" gate is not met: the prompt eval is 14/20 (target 15/20), because one new memory pushed a fragile hit out of a keyword pool. A DNS outage took production offline from outside for 7 days 20 hours mid-milestone.**

| Done when | Result | |
| --- | --- | --- |
| Accepting a proposal unpins its sources; 104416 no longer injected on production; a progress or expired temporal pin is never injected (tests) | Live probe: 104416 out of the pin block; tests in `9723db5` | Met |
| An unchanged pair judged "not worth" isn't sent to the LLM again (test, one live session end) | Session `3875ecaf` ended twice: 2 judged, then no call (§1) | Met |
| Consolidation runs log a dollar cost | `usd: 0.0007014` on the live run (§1) | Met |
| Every approved project: entries in = records out, backup identical, auto-memory off, `git status` unchanged | 7 of 7 imports (§2) | Met |
| Inline MEMORY.md files import entry by entry | p09: 85 inline entries → 85 records | Met |
| Heuristic, LLM-proposal and needs-review paths each ran on real entries, counts reported; author reviewed the needs-review queue | Heuristic: dry run (3 entries); LLM: 95 entries; needs-review: 2 (§2) | **Partly met:** the author hasn't reviewed the queue yet |
| Re-running the import creates nothing | 7 of 7 re-runs created nothing | Met |
| In a repo other than AgentOps, a prompt seed shows a personal memory labelled "personal", and personal pins appear (live) | job-hunter (p12) and p08 probes (§3) | Met |
| A personal memory never appears from a file or error cue (test) | `a_seeded_subagent_gets_personal_memories_and_work_cues_never_do` (`e92a159`) | Met |
| User-level auto-memory off, after the check above | `agentops memory-off --user`, 2026-10-27 | Met |
| Prompt-hook p95 under 300 ms remote | 166 ms (2026-10-27), 215 ms (2026-11-04), n = 200 each (§3) | Met |
| 3 confirmed sessions replayed: recalled before the failing step or not, path, rank | 4 replayed, the author's choice (§4) | Met |
| Both evals still 15/20 (prompt) and 6/20 (work); worst stability drop ≤ 3 ranks | Prompt **14/20**, work 6/20; c09 dropped 17 ranks (§7) | **Not met** |

## 1. What shipped

All on `main`, deployed by the push-to-main autodeploy. The last deploy is `d9f0980` (2026-11-04).

| Item | Commits | What |
| --- | --- | --- |
| 1 | `9723db5` | **Pins honour the memory class.** `session_guide::active_pins` filters `nodes_pinned` by `MemoryClass::recallable_at`; `record_follows` credits only shown pins. |
| 1 | `c7ad41d`, `a060f12` | **"Not worth" is recorded.** Table `consolidation_not_worth` (SQLite migration, Postgres schema) keyed on the candidate with the sources' digest; `unblocked` skips an unchanged one. `parse_proposal` is three-way: `Worth`, `NotWorth` (only an explicit `"worth": false`), `Unreadable` (counted in `RunLog.unreadable`, never recorded). |
| 1 | `80957b4` | **Groq price** for `openai/gpt-oss-120b`: $0.15 in, $0.075 cached, $0.60 out per million tokens (Groq's model page, 2026-10-26). New rows only. |
| 2 | `0a031ff` | **Guest connect** (`guest_connect.rs`): in every repo but AgentOps, `connect --remote` changes no tracked file. Hooks go to `.claude/settings.local.json`; a path that isn't ignored goes into `info/exclude` (`git rev-parse --git-path`); a tracked path aborts before anything is written; an unmatched remote is registered memory-only; a stale marker is re-resolved (gotcha 54243). `update_settings_json` refuses to overwrite a malformed settings file. `agentops memory-off [--user]`. `--full` keeps the old behaviour. |
| 3 | `29bfe51` | **Importer:** inline MEMORY.md bullets are entries; titles and slugs shared with an earlier entry get a line suffix (gotchas 195360, 387595); records are checked against an independent count; scope proposals (`classify_scope::mentioned`) are listed, never applied; `--backup-dir`. |
| 3 | `82398b8` | **Server-side kind classification:** `--llm-classify` on a connected repo calls `POST /memories/classify-kind`, which judges with the org's own Anthropic or Groq key (`agentops_llm::classify_memory_kind`, 2,048 output tokens), paced at 8 a minute (gotcha 299440). |
| 7 | `75e520d` | **`recall-stats`** splits sessions into meta (touched `recall_stats::META_PATHS`) and ordinary, with ordinary follows per source. |
| 4 | `d599b3a` | **Personal connection:** one per user, in its own tenant `u<user_id>` with connection id `personal`, created by the first write, reached only from the caller's user id (`path: "personal"`, `add_note personal: true`). `TenantCaller` carries `user_id`. |
| 4 | `e92a159`, `d9f0980` | **Personal recall:** `hooks::handle_with_personal`. Personal pins after the repo's in one `PIN_BUDGET`; up to `PERSONAL_SEED_MAX = 2` `[personal]` pointers after the repo's seed in prompt and seeded-subagent seeds; claims and follows recorded in the personal repo; never on work cues. A personal pin cut from the block is named with `path "personal"`. `import-memory --personal`. Curation and retitle routes accept `personal`. |

**Docs:** `13-agent-integration.md` (guest connect, personal memories, `memory-off`, recall-stats split), `06-notes-and-consolidation.md` (inline entries, counts and scope, server classification, `--backup-dir`).

**Deviations, each with its approval:**

- **Writes to other repos** (the author, 2026-10-26): spec item 2 wins over the privacy rule. Guest connect writes `.context/agentops-remote.json` and `.claude/settings.local.json` and may append to `info/exclude`. The list per repo is in §2.
- **Clients memory-only; dev-knowledge-base memory-only** (the author, 2026-10-26): no deploy keys.
- **The personal connection lives in a per-user tenant**, not the org tenant (the author, 2026-10-26). The plan council found that a connection inside the org tenant could be read and written by a teammate who guessed its id.
- **104416 stays pinned; 368718 pinned** (the author, 2026-10-26). The class filter alone keeps 104416 out of the block.
- **Kind classification on the server** instead of a laptop Anthropic key (the author, 2026-10-27).
- **Only one client project imported** (p08), the others left untouched because the author has left those companies (2026-10-27). **All personal projects and user-level imported** (2026-10-27).
- **Four evidence sessions, not three** (the author picked all four, 2026-11-04), one marked weak.
- **p10's `archive/` (2 entries) not imported**, as archived by the author; kept in the backup.
- **Pushes:** each approved by the author (`80957b4` batch, the Phase B batch, `82398b8`, `d9f0980`).

## 2. Import

**Dry run** (2026-10-27): all 22 memory directories, heuristic-only, each into an empty scratch graph on the laptop, so nothing reached the server before approval. 256 entries in, 256 records out; 16 projects flagged client (remote owner not the author, or under a client folder). The GitHub App reaches 3 repos; job-hunter is the only imported project it indexes.

**Imported:**

| Alias | Connection | Entries → records | Kinds (episodic / procedural / semantic) | Signal | Created / duplicate / needs review | Pins | Scope proposals | Re-run |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| p08 | memory-only | 6 → 6 | 2 / 2 / 2 | frontmatter 6 | 6 / 0 / 0 | 2 | 1 | 6 skipped |
| p09 | memory-only | 85 → 85 | 9 / 9 / 67 | llm 85 | 84 / 0 / 1 | 9 | 0 | nothing new |
| p10 | memory-only | 56 → 56 | 43 / 5 / 8 | frontmatter 56 | 55 / 1 / 1 | 5 | 1 | nothing new |
| p12 | indexed (GitHub App) | 3 → 3 | 1 / 1 / 1 | frontmatter 3 | 3 / 0 / 0 | 1 | 0 | 3 skipped |
| p13 | memory-only | 1 → 1 | 1 / 0 / 0 | frontmatter 1 | 1 / 0 / 0 | 0 | 0 | 1 skipped |
| p21 | memory-only | 1 → 1 | 1 / 0 / 0 | frontmatter 1 | 1 / 0 / 0 | 0 | 0 | 1 skipped |
| user-level | personal | 15 → 15 | 4 / 2 / 9 | frontmatter 5, llm 10 | 15 / 0 / 0 | 2 | 3 | 15 skipped |

- **The three paths:** the heuristic ran in the dry run (p09 2, user-level 1); the LLM ran on the 95 untyped entries of the import. With `--llm-classify` the importer asks the LLM instead of the heuristic (milestone 2 design), so no imported record says "heuristic". Needs-review: 2, both entries that call themselves stale. **11 rules were pinned on the LLM's judgment** (p09 9, user-level 2, confidence 0.8–0.95); the author should look at them.
- **Live fixes during the import:** a missing log directory (no write happened), a laptop CLI older than `82398b8` (failed before any write; reinstalled), and p10's nested `archive/`, which made the backup step fail loudly instead of copying partially.
- **Paths written per repo:**
  - Every imported repo: `.context/agentops-remote.json`, `.claude/settings.local.json` (hooks, `autoMemoryEnabled: false`, the repo's own keys kept).
  - `.context/` added to `info/exclude`: p09, p10, p13, p21. p08 and p12 already ignored both paths.
  - `git status` was compared before and after on every repo: unchanged on all 6. p09, p10, p13 and p21 already had uncommitted work of their own, so "unchanged" is not "clean".
- **Paths written outside repos:** `~/.claude.json` (the user-level MCP entry, rewritten with the same values), `~/.claude/settings.json` (`autoMemoryEnabled: false`), `~/.agentops/backups/<alias>/2026-10-27*` (every backup identical to its source).
- **Two throwaway connections** were registered while testing: `6ad8fa3a…` (guest-connect probe) and job-hunter's stale marker fixed to `Jesus-Glez60--job-hunter`. The probe connection is left in place (the author, 2026-10-27): no errors in the server logs, nothing indexes it.

## 3. Personal connection

- **Live check** (2026-10-27): a prompt in job-hunter (p12) and one in p08 showed the personal pins labelled `(personal, id …)` and, in job-hunter, a `[personal]` seed pointer to 401981 after the repo's own pointers. job-hunter's hooks had been posting to a merged stub's id; the guest connect re-resolved its marker (gotcha 401982).
- **Pin block with personal pins:** in AgentOps, 2,905 of 3,000 characters: 15 repo pins and 2 of 3 personal pins. The third (406925) is cut and named as `fetch_content path "personal" ids 406925` (`d9f0980`). In p08: 689 characters, 2 repo and 3 personal pins.
- **Latency** (`recall-eval --remote --repeat 10`, 200 prompt hooks each):
  - before personal recall (2026-10-26): p50 133 ms, p95 175 ms;
  - after, with a personal connection (2026-10-27): p50 128 ms, p95 166 ms;
  - final (2026-11-04): p50 163 ms, p95 215 ms. Under 300 ms; the rise since 2026-10-27 is **unverified** as to cause (the graph grew by ~180 memories across repos).
- **Noise seen:** a one-word prompt ("Proceed") drew two personal memories about the editor theme and the terminal at 0.57–0.60 meaning. Personal recall has no stricter threshold than repo recall.

## 4. Evidence

Candidates: transcripts before 2026-10-14 of the projects whose memories are now imported (AgentOps 21 sessions, job-hunter 1). A first word-overlap pass found nothing usable: half its matches named an entry written after the session. The second pass used recall itself (read-only `POST /repos/{id}/recall` on each correction), then a manual reading. The author confirmed 4.

Each replay ran on today's graph, on copies of the transcript and its subagents cut just before the correction:
- the prompt seed was checked with `POST /recall` for every earlier prompt, at the seed's own limit (5 default, 8 plan);
- work cues came from `work-recall-eval --remote --replay` on the cut copies (32–93 injections per session; none was the target);
- the pin came from the target's state today.

| Case | Session | Memory (MEMORY.md entry written) | Recalled before the correction? | Path | Rank |
| --- | --- | --- | --- | --- | --- |
| E1 | `5431fe7b` (2026-10-05) | 104413, the no-Co-Authored-By rule (2026-08-17) | Yes, all 15 earlier prompts | pin | – |
| E2 | `2001c928` (2026-10-06) | 104449, the hosted server is the live one (2026-09-04) | **No** | none: not in the top 30 for any of the 14 prompts; no cue | – |
| E3 | `84aa616d` (2026-09-25) | 104449 (same) | Yes, from prompt 6 of 24 | prompt seed | 5 |
| E4 (weak) | `f8e90857` (2026-09-28) | 104410, notes reach the DB through MCP (2026-09-29, after the session) | Yes, all 3 prompts | pin | – |

**What it says.** Of the three valid cases, two are recalled before the step that went wrong (E1 by pin, E3 by prompt seed at the last slot a default seed shows) and one is not (E2). E2's prompts never mention servers or running locally. The step that went wrong was an action: running the app locally. A Bash command is a cue only when it reads a file or fails, so no path in today's design would surface that memory then. E1 and E4 hold only because the memories are pinned: a pin is present on every prompt, which says nothing about cue recall. **Leakage:** none. Every target is an imported MEMORY.md entry, not something the replayed session wrote.

## 5. Ordinary sessions so far

`agentops recall-stats --remote` (2026-11-04):
- AgentOps: 4 sessions with shown memories, **1 ordinary and 3 meta**; 2 follows in all (a file cue and a pin), **0 in the ordinary session**.
- p08, p09, p10, p12: 0 sessions.

The counting runs, but there is almost nothing to count. **Production was unreachable from outside from 2026-10-28 01:28 UTC (the last hook request in the proxy log) to 2026-11-04 21:07**, which began the morning after the imports. Cause: the deSEC dynDNS token started getting `401` on 2026-10-28 02:25, the home IP changed, and the updater's unit still reported success (gotcha 411710). Fixed with a new token and a script that exits with curl's status. [Likely] every hook in every repo timed out silently in that window. The follow threshold (200 follows, 20 ordinary sessions) is far off.

## 6. The requirement, updated

| Outcome | State | Evidence |
| --- | --- | --- |
| A relevant memory reaches the context without a tool call, in normal and plan mode and in subagents | **Met** | Milestones 1, 3b; personal memories now too (§3) |
| A memory surfaces on its cue and not otherwise; injection stays inside a fixed budget | **Met** against the 3b targets (decided: noise and useful share, 1 pointer per cue) | Spec decision; work cues stay at 1 pointer, 800 characters |
| Rules that must always hold are enforced or always present | **Met** | Pins, including personal pins; 11 new rules pinned (§2) |
| MEMORY.md imported once, every entry routed, nothing silently dropped | **Met** for AgentOps and the 6 personal and 1 client projects imported; 15 client projects deliberately not imported | §2; entries = records everywhere; unparsed lines and archives counted |
| Session learnings become memories without asking, consolidated at session end | **Met**, with a person accepting | Milestone 4; "not worth" no longer re-judged (§1) |
| A memory recalled and wrong gets flagged; one recalled and used gets stronger | **Open** | No "wrong" signal; 2 follows, 0 in ordinary sessions |
| Measured on a replay of the evidence sessions | **Met as a measurement:** 2 of 3 valid cases recalled before the failing step (E4 also, but weak); the miss (E2) is an action without a cue | §4 |

## 7. Open issues, and what must not break

**Open:**
- **The prompt eval fell to 14/20; the stability limit is breached.** No code changed: c09's target matched "every" and "anthropic", the two-keyword minimum (`min_keyword_matches`) needs both, and a new note using "every" (411710) took its place in the "every" pool. It fell from rank 3 to 20 (gotcha 416490). "every" isn't a stopword while "each", "all" and "any" are, but adding it would not bring c09 back: the hit was carried by that word. **Proposed next:** a common word's pool cutoff shouldn't decide whether a memory reaches the two-keyword minimum (the author chose to fix this next, not here, 2026-11-04).
- **Actions aren't cues** (E2). Candidates: a cue on commands that run or deploy the app, or pinning such rules.
- **The needs-review queue** (2 stale entries) and the **11 LLM-pinned rules** await the author.
- **No alert when the DNS updater fails:** the unit now fails visibly, but nothing pushes to Kuma (gotcha 411710).
- **The memory-only registration message** says "it has no git remote" even when the repo has one (server text in `register_repo`).
- **Personal recall noise** on short prompts (§3); no stricter threshold yet.
- **15 client projects** keep their MEMORY.md; the author may port them later or delete the repos.
- From milestone 4, still open: `migrate-graph` drops data; the 80-row SQLite cutoff; no deploy gate for the laptop CLI (it needed two reinstalls here).

**What must not break:**
- Milestone 4's list, plus:
  - guest connect never changes a tracked file and never writes a marker before the exclude;
  - a settings file that isn't JSON is left alone;
  - the personal connection is reachable only from the caller's own user id, never by a path id, and never from a work cue;
  - repo pins win the pin budget; a cut personal pin names its path;
  - "not worth" is recorded only for an explicit verdict;
  - records out equal an independent count of entries in.

**My read of what comes next:** the recall robustness fix above first, since it is what broke the gate. Then either the rules milestone or an explicit "wrong" signal; reinforcement can't start on data yet (2 follows). Of those two, the "wrong" signal is the one the evidence asks for. E2 was a memory that existed and wasn't recalled, and nothing in the system records that a correction happened.

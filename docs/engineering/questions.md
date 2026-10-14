# Questions for the author

Questions the code cannot answer: where it shows *what* but not *why*, or where two parts disagree. Answers here are the only allowed source of rationale that is not already in code or a recorded note.

Format: question, what the code already shows, and what the answer unblocks. Answered items move to the bottom with the answer and date.

## Open

None. All questions were answered on 2026-10-13/14; see below.

## Answered

### A-6. Is embedding the full server in the CLI binary intended long-term? (was Q1)

- Answer (author, 2026-10-13): yes. One install matters more than binary size or compile time.
- Recorded in ADR 0038.

### A-7. Why is the access mode per org on hosted `/mcp` but per process on REST `/tools`? (was Q5b)

- Answer (author, 2026-10-13): intended. REST `/tools` accepts only the instance key, so its caller is the operator and one process-wide mode fits. Hosted `/mcp` is multi-tenant, so each org sets its own.
- Recorded in ADR 0039.

### A-8. Which write tools are meant to be available in Advisor mode? (was Q6)

- Answer (author, 2026-10-13/14):
  - `persist_blueprint_doc`, `register_library` and `discover_library`: intended in Advisor, as knowledge writes.
  - `unregister_repo`, `scrape_library` and `sync_changelogs`: Full only, on hosted `/mcp` (`HOSTED_FULL_ONLY_TOOLS`, `mcp_http.rs`).
  - The local stdio server does not gate them: its user owns the machine.
  - The web Libraries page needs the new `libraries.manage` capability (admin and member) to scrape or sync; the org's access mode does not apply there.
- Recorded in ADR 0041.

### A-9. Is the consolidation no-regression gate tuned as intended? (was Q7)

- Answer (author, 2026-10-13): no; a candidate should promote only on strict improvement. Both gates now use `candidate > baseline` (`should_promote` in `agentops-embeddings-train` and `agentops-heavy-consolidate`). A tie keeps the active model.
- Recorded in ADR 0013.

### A-10. Where does CI run? (was Q8)

- Answer (author, 2026-10-13): there is no public CI; the docs say only that. A GitHub Actions test workflow may come later.
- Recorded in ADR 0040.

### A-11. Should `repo-map.md` be committed, or the README link changed? (was Q9)

- Answer (author, 2026-10-13, through the F2 decision): change the link. README and `llms.txt` now point to `docs/engineering/00-index.md`; `repo-map.md` stays a locally generated file.

### A-12. Why SQLite by default, and Postgres for the code graph only? (was Q10)

- Answer (author, 2026-10-13): SQLite needs no setup for a solo user. Postgres is used only where graph size and concurrency on the hosted server need it; the other stores are small.
- Recorded in ADR 0037.

### A-13. Why one unified MCP server instead of the earlier three? (was Q11)

- Answer (author, 2026-10-13): both reasons. The licence and workspace split was gone, and a client needs one MCP entry instead of three.
- Recorded in ADR 0002.

### A-14. Should node recency use the 30-day `Affects` half-life? (was Q12)

- Answer (author, 2026-10-13): only for notes. `recency_multiplier` now decays gotchas, decisions and notes only; code is never ranked down for being stable. This also makes code ranking the same on SQLite and Postgres.
- Recorded in ADR 0013.

### A-15. Do the LoRA consolidation deferrals still stand? (was Q13)

- Answer (author, 2026-10-13): hide `consolidate_model` until its consumer, a local-first `explain_symbol`, exists. The tool and `POST /consolidate/model` are no longer exposed; the crate stays.
- Recorded in ADR 0013 and `findings.md` F29.

### A-1. Should AgentOps's own docs be registered in docbrain?

- Answer (author, 2026-10-12): no. Read local docs directly; no docbrain registration or scraping for AgentOps. The brief's audience note about ingesting via `scrape_library` with `slug: agentops` is out of scope.

### A-2. What does "heavy" mean today? (was Q2)

- Answer (author, 2026-10-12): nothing current. It is a remnant of a plan to sell AgentOps as SaaS: a paid "heavy" suite and a free "light" suite. The author chose the open-source route instead, and the heavy/light split no longer means anything.
- Consequences for the docs:
  - The glossary defines "heavy" as a historical name only.
  - Layer and crate descriptions use functional names; `heavy` appears only as part of crate names.
  - The leftover naming is logged in `findings.md` F11.
  - ADR A2 records the abandoned SaaS tier; ADR A1 records the merge that removed it.

### A-3. Full list of Decision notes (was Q3)

- Answer (author, 2026-10-12): all decisions were migrated into the hosted AgentOps graph. Early work used a local DB; check it for information.
- Done: `.context/graph.db` (local graph, last written 2026-10-05) holds 50 `decision`, 91 `gotcha` and 10 `note` nodes for repo `agentops`. All 50 decisions were listed; `survey/adr-candidates.md` now uses that list as its primary source. All 50 show `last_touched_at` 2026-09-21, which is likely the migration date, not the original decision date; git history dates them instead.

### A-4. Layer names for `01-architecture.md` (was Q4)

- Answer (author, 2026-10-12): approved, as follows. The "heavy" row is renamed per A-2.

| Layer | Crates |
| --- | --- |
| Foundations | `security`, `sqlite-support`, `embeddings`, `repo-access`, `agents-md`, `ruler-bridge`, `lsp-client`, `github-app`, `integration-modules` |
| Code graph and storage | `graph`, `graph-pg`, `store-open` |
| Indexing | `scanner`, `docgen` |
| Repo registry | `manifest` |
| Knowledge and retrieval | `retrieval`, `notes`, `llm`, `embeddings-train` |
| Integrations | `linear`, `integrations` |
| docbrain | `docbrain-graph`, `docbrain-ingest`, `docbrain-mcp`, `docbrain-api` |
| Semantic index and consolidation | `heavy-embeddings`, `heavy-consolidate`, `heavy-mcp` |
| Accounts and teams | `accounts`, `teams` |
| Use cases and transports | `mcp`, `api`, `heavy-api` |
| Binaries | `server` (also a library), `mcp-server`, `cli` |

### A-5. Why is `advisor` the default? (was Q5, first half)

- Answer (author, 2026-10-12): the MCP client can be any code editor or agent, from vim/nvim to VS Code and Cursor. The server cannot know or trust which one is calling, so the safe default applies.
- Matches the code comment on `AccessMode::Advisor`: "the safe default for an untrusted or read-only integration" (`agentops-core/crates/agentops-mcp/src/tools.rs`).
- The brief's "why hide write tools rather than reject calls" has a false premise: Full-only tools are hidden **and** refused (`call_tool`'s backstop).
- The second half of Q5 stays open as Q5b.

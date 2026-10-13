# Survey: ADR candidates (Phase 1)

Decisions significant enough for an ADR, with the evidence that dates them. Input for Phase 5; the author picks which become `adr/NNNN-<slug>.md`.

Sources:
- git history: `git log --grep=<topic>`.
- All 50 Decision nodes in the early local graph `.context/graph.db` (author answer A-3: everything was migrated to the hosted graph from there). Cross-checked with three `local_semantic_search` queries (`kind: "decision"`) on the hosted graph.
- Code comments that state a reason.

Decision notes are cited by title (their vault file slug is the stable id). Decisions recorded after the local graph's last write exist only on the hosted graph. The hosted searches found four not among the local 50: "register_repo now accepts local_id for repos with no git remote" (M6), "Self-service org switcher shipped (GET /me/memberships, POST /me/switch-tenant)", "Small local models (≤4GB): recommended for the cheap LLM tier only, never bundled" (R6), and "Marketing site has no login/register flow by design" (T4). Others may exist; Phase 5 re-runs the hosted searches per ADR topic.

UI-only decisions (fonts, drawer layout, graph node dragging, tag colors, auth form feedback, docs viewer navigation) are not ADR material. They go into `10-frontends.md` instead; see "Frontend decisions" at the end.

"Why recorded?" says whether the rationale already exists somewhere. **No** means the ADR needs the author's answer.

## Architecture and topology

| # | Candidate | Evidence | Why recorded? |
| --- | --- | --- | --- |
| A1 | Merge into one open-core: 3 REST APIs + 3 stdio MCP servers become `agentops-server` + `agentops-mcp-server`, no price gate | `218c3fc` 2026-08-24 "Merge into one open-core: no price gate, single REST/MCP process, minimal env vars"; decision "Backend merge: 3 REST APIs + 3 stdio MCP servers collapsed into agentops-server + agentops-mcp-server" | Mostly (note + A-2); Q11 |
| A6 | REST spec hand-written (`docs/openapi.yaml`), no code generation; AgentOps self-seeds as a docbrain library | `5918ede` 2026-08-25; decision "README, OpenAPI spec, llms.txt, and docbrain self-seed shipped" | Yes (note); see `findings.md` F3 |
| A7 | Documentation Viewer: structured `DocPage`, LLM-free docgen, opaque-string persistence to avoid a crate cycle | Decision "Documentation Viewer backend: structured DocPage, LLM-free docgen, opaque-string persistence to avoid a crate cycle" | Yes (note) |
| A2 | Paid "heavy" SaaS tier vs free "light" suite, abandoned for open source | `0385f5e` 2026-07-28 "Add heavy-tier commercial workspace with Postgres-backed GraphStore"; then A1 | Yes: author answer A-2 |
| A3 | SQLite default, Postgres (pgvector) optional via `agentops-graph-pg` + `agentops-store-open` | `7a393e7` 2026-07-28 (SQLite core pipeline), `0385f5e` (Postgres) | No |
| A4 | One `AGENTOPS_DATA_DIR` replaces ~10 storage env vars | Decision "AGENTOPS_DATA_DIR consolidates ~10 storage env vars -- AGENTOPS_DOCBRAIN_DB is not the same thing as DOCBRAIN_DB_DIR" | Yes (note) |
| A5 | `agentops-cli` embeds the full server for `serve-api` | Doc comment on `agentops_server::run` (`agentops-core/crates/agentops-server/src/lib.rs`) | What: yes. Why: Q1 |

## MCP and access control

| # | Candidate | Evidence | Why recorded? |
| --- | --- | --- | --- |
| M1 | Structural `AccessMode` (Advisor/Full) in a single `ToolSpec` table, Advisor default | `33b4aaa` 2026-07-28 "Implement agentops-mcp: real stdio MCP server with structural AccessMode enforcement"; module doc of `agentops-core/crates/agentops-mcp/src/tools.rs` | Table design: yes (code). Advisor default: Q5 |
| M2 | `add_note` allowed in Advisor mode | Comment at `agentops-core/crates/agentops-mcp/src/tools.rs:96-102` | Yes (code) |
| M3 | Per-tenant MCP access mode on the hosted server | `ee9b7e9` 2026-08-31 "... add per-tenant MCP access mode" | No: why per-org rather than per-key, Q5 |
| M4 | docbrain tools exposed over hosted `/mcp`, tenant-scoped by one SQLite file per tenant | `eba0e98` 2026-09-04; decision "Multi-tenant docbrain isolation via one SQLite file per tenant, not a TenantContext" | Yes (note) |
| M5 | Session correlation through a `session_id` argument, not a new subsystem | Decision "Module 6 session correlation is protocol-native via session_id, not a new subsystem" | Yes (note) |
| M7 | `get_symbol` returns the gotchas and decisions that affect a symbol | Decision "get_symbol surfaces affecting gotchas and decisions directly" | Yes (note) |
| M6 | `register_repo` accepts `local_id` for repos with no remote | Decision "register_repo now accepts local_id for repos with no git remote" | Yes (note) |

## Retrieval and knowledge

| # | Candidate | Evidence | Why recorded? |
| --- | --- | --- | --- |
| R1 | Vector RAG with BGE-M3 + Qdrant for the heavy path | `c1201e8` 2026-07-28 "Add vector RAG (BGE-M3 + Qdrant) for semantic search over the neuron graph" | No |
| R2 | Multi-signal (dense + BM25 + exact-name) retrieval redesign | Decision "Phase 4 multi-signal retrieval redesigned against current architecture, live-verified" | Partly; confirm weights rationale |
| R3 | CLS-inspired retrieval + local consolidation (PPR expansion, Hebbian edges, LoRA) | `4d5ffff` 2026-08-21; decision "CLS-inspired cleanup/optimization/local-consolidation plan (Initiatives 0-6) fully shipped" | Partly; eval/regression meaning is Q7 |
| R4 | `agentops-retrieval` never depends on `agentops-embeddings-train` (`EmbeddingProjector` trait) | Decision of the same name; verified in `survey/layers.md` | Yes (note) |
| R5 | Weighted decaying edges and automatic note re-matching | Decision "weighted decaying edges, auto note re-matching, and repo state added" | Yes (note) |
| R6 | Small local models only for the cheap LLM tier, never bundled | Decision "Small local models (≤4GB): recommended for the cheap LLM tier only, never bundled" | Yes (note) |
| R8 | Gotchas are curated (keep / reduce prominence with a reason), never resolved or dismissed | Decision "Gotchas are curated, never resolved/dismissed -- don't reintroduce a bug-tracker model" | Yes (note) |
| R9 | Graph edges `DependsOn`/`References` are structural, not a real call graph | Decision "Knowledge Graph screen's tabs map to real EdgeRelations -- DependsOn/References are structural, not a real call graph" | Yes (note) |
| R10 | docbrain prefers a site's `llms.txt` over HTML link-scraping | Decision "docbrain's shallow crawl now prefers a site's llms.txt over blind HTML link-scraping -- including following a meta-index one hop deeper" | Yes (note) |
| R11 | C++/C# language coverage and the file watcher | Decision "Phase 5: C++/C# language coverage + file watcher, live-verified" | Partly; confirm |
| R7 | Optional rust-analyzer LSP pass for macro-generated code | `72be1e1` 2026-09-25, `5777224` 2026-10-05 | No |

## Teams, auth, security

| # | Candidate | Evidence | Why recorded? |
| --- | --- | --- | --- |
| S1 | Tenant id doubles as organization id | Decision "Team model reuses tenant as organization ID, relaxes users.tenant uniqueness" | Yes (note) |
| S2 | Owner is a column on `organizations`, not a role | Decision "Owner is a single column on organizations, not a distinct membership role" | Yes (note) |
| S3 | Fixed + custom roles; permissions matrix drives both display and enforcement | Decisions "Roles are fixed + custom, validated at both hardcoded enum and database table" and "Permissions matrix is the single source of truth for both display and enforcement" | Yes (notes) |
| S4 | Audit log visible to Owner/Admin only, outside the matrix | Decision "Audit log visibility is hardcoded Owner/Admin-only, not part of toggleable matrix" | Yes (note) |
| S5 | Invites are copyable links, no email infrastructure | Decision "Team invites are copyable links, no email infrastructure" | Yes (note) |
| S6 | Personal integration credentials under a separate key | Decision "Personal integration credentials encrypted under a genuinely different key" | Yes (note) |
| S8 | Audit log is insert-only | Decision "Audit log is insert-only by design" | Yes (note) |
| S9 | 2FA: TOTP secret encrypted like integrations; login uses a separate challenge token, not a real session | Decisions "2FA TOTP secret encrypted with same scheme as integrations" and "2FA login flow uses separate challenge token, not real session" | Yes (notes) |
| S10 | `RUSTSEC-2023-0071` (rsa timing side-channel) accepted in `deny.toml`, required for GitHub App RS256 | Decision "RUSTSEC-2023-0071 (RSA Marvin Attack timing side-channel) accepted, not fixed -- required for GitHub App RS256" | Yes (note) |
| S11 | Signup `invite_token` gates open signup but does not consume the invite | Decision "Signup's invite_token gates open signup, but must NOT accept/consume the invite itself" | Yes (note) |
| S7 | Org deletion is a full cascade, audit log excluded | Decision "Org deletion is a full cascade — audit_log excluded, codebrain graph out of scope" | Yes (note); see `findings.md` F10 |

## Tooling and process

| # | Candidate | Evidence | Why recorded? |
| --- | --- | --- | --- |
| T1 | `connect` registers MCP + distributes instructions for Claude Code / Cursor / Codex / Gemini CLI / Windsurf; Cline, Zed and Copilot excluded | Decisions "agentops connect shipped: ..." and "Cline/Zed/VS Code Copilot deliberately excluded from connect's first-class adapter list -- Windsurf added instead" | Yes (notes) |
| T2 | Notes live in the AgentOps graph, not as files in the repo (`.claude/hooks/block-local-notes.sh`) | Hook file; `docs/vault-protocol.md` | Partly; confirm |
| T3 | Where CI runs (`.github/workflows/` holds only `release.yml`) | Repo contents | No: Q8 |
| T4 | Web app under the `/suite` base path; marketing site with no login | `1206993` 2026-09-27; decision "Marketing site has no login/register flow by design" | Partly |
| T5 | Linear integration: webhook auto-kickoff, per-account opt-in via `agentops-integration-modules` | Decisions "Phase 6 core implemented and live-verified: Linear webhook auto-kickoff" and "Phase 6c: auto-kickoff moved from boot-time config to per-account opt-in via agentops-integration-modules" | Yes (notes) |

## Frontend decisions (for `10-frontends.md`, not ADRs)

From the local graph, titles only:
- "The search detail panel's body font is deliberately JetBrains Mono -- not an oversight if it looks monospace"
- "Knowledge Graph's node detail panel is a Sheet drawer overlay, not an inline side-by-side flex panel -- fixes a whole-page-scroll bug"
- "Knowledge Graph nodes are non-draggable -- positions are force-simulation output, not something a user arranges"
- "Node-kind tag colors (Symbol/File/Gotcha/Decision/Note) live in one shared map -- don't reintroduce a local copy"
- "Whole-repo graph view (/repos/{name}/graph) is deliberately uncapped -- NODE_CAP only applies to the seed-centered BFS"
- "auth form UX decision: password feedback is inline/live, name+email feedback is a toast"
- "Documentation Viewer frontend converged on a nav-driven book index, not a single scrolled document"

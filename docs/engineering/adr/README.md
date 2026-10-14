# Architecture decision records

Decisions with a recorded reason, one file each. A record exists only when its "why" is in a decision note, a code comment or an author answer in `questions.md`. Candidates come from `survey/adr-candidates.md`.

## Records

| ADR | Decision | Candidate |
| --- | --- | --- |
| [0001](0001-abandon-paid-tier.md) | Abandon the paid heavy tier for open source | A2 |
| [0002](0002-one-open-core-server.md) | Merge three servers into one open-core process | A1 |
| [0003](0003-one-data-dir.md) | One AGENTOPS_DATA_DIR for all storage defaults | A4 |
| [0004](0004-hand-written-openapi.md) | Hand-write the OpenAPI spec | A6 |
| [0005](0005-docpage-without-crate-cycle.md) | Structured DocPage, LLM-free docgen, opaque persistence | A7 |
| [0006](0006-structural-access-mode.md) | Structural Advisor/Full access mode, Advisor by default | M1 |
| [0007](0007-add-note-in-advisor.md) | Allow knowledge writes in Advisor mode | M2 |
| [0008](0008-docbrain-file-per-tenant.md) | Isolate docbrain tenants with one SQLite file each | M4 |
| [0009](0009-session-id-correlation.md) | Correlate sessions with a session_id argument | M5 |
| [0010](0010-register-repo-local-id.md) | Register repos that have no git remote | M6 |
| [0011](0011-get-symbol-surfaces-notes.md) | Return affecting gotchas and decisions from get_symbol | M7 |
| [0012](0012-database-native-hybrid-search.md) | Database-native multi-signal retrieval | R2 |
| [0013](0013-cls-retrieval-and-consolidation.md) | CLS-inspired retrieval and local consolidation | R3 |
| [0014](0014-embedding-projector-port.md) | Keep training out of the retrieval crate | R4 |
| [0015](0015-weighted-decaying-edges.md) | Weighted decaying edges and automatic note re-matching | R5 |
| [0016](0016-curate-not-resolve-gotchas.md) | Curate gotchas instead of resolving them | R8 |
| [0017](0017-structural-not-call-graph.md) | Structural edges, not a call graph | R9 |
| [0018](0018-prefer-llms-txt.md) | Prefer llms.txt when crawling library docs | R10 |
| [0019](0019-cpp-csharp-and-watcher.md) | C++ and C# support and a file watcher | R11 |
| [0020](0020-tenant-is-organization.md) | Use the tenant id as the organization id | S1 |
| [0021](0021-owner-column.md) | Make ownership a column, not a role | S2 |
| [0022](0022-roles-and-matrix.md) | Fixed plus custom roles, one permissions matrix | S3 |
| [0023](0023-audit-log.md) | Insert-only audit log, Owner and Admin only | S4, S8 |
| [0024](0024-invite-links.md) | Invites are copyable links | S5 |
| [0025](0025-personal-credential-key.md) | Encrypt personal credentials under their own key | S6 |
| [0026](0026-org-deletion-cascade.md) | Full cascade on org deletion, except the audit log | S7 |
| [0027](0027-two-factor-auth.md) | TOTP 2FA with a separate challenge token | S9 |
| [0028](0028-accept-rustsec-2023-0071.md) | Accept RUSTSEC-2023-0071 for GitHub App signing | S10 |
| [0029](0029-signup-invite-token.md) | Signup validates an invite without consuming it | S11 |
| [0030](0030-lazy-membership.md) | Create membership rows lazily on first touch | S1 (follow-up) |
| [0031](0031-connect-and-supported-tools.md) | connect: MCP registration plus Ruler distribution for five tools | T1 |
| [0032](0032-notes-live-in-the-graph.md) | Notes in remote-connected repos go to the server | T2 |
| [0033](0033-suite-base-path-and-marketing.md) | Web app under /suite; marketing site without login | T4 |
| [0034](0034-linear-auto-kickoff-opt-in.md) | Linear auto-kickoff as a per-account module | T5 |
| [0035](0035-no-precompact-hook.md) | Session resume without a PreCompact hook | M5 (follow-up) |
| [0036](0036-hosted-input-and-auth-hardening.md) | Hosted input and auth hardening | F31, F35, F37–F39 |
| [0037](0037-sqlite-default-postgres-for-graph.md) | SQLite by default, Postgres for the code graph only | A3 |
| [0038](0038-one-cli-binary-with-the-server.md) | One `agentops` binary that embeds the server | A5 |
| [0039](0039-access-mode-per-org-on-mcp.md) | Access mode per org on hosted `/mcp`, per process on REST | M3 |
| [0040](0040-no-public-ci.md) | No public CI | T3 |
| [0041](0041-hosted-full-only-tools.md) | Full-only network and destructive tools on hosted `/mcp`; `libraries.manage` | Q6 |
| [0042](0042-strict-promotion-note-recency-hidden-lora.md) | Strict promotion, note-only recency, LoRA hidden | R3 (follow-up) |
| [0043](0043-remove-billing-role.md) | Remove the billing role and dead capabilities | F12, F13 |
| [0044](0044-reuse-module-labels.md) | Reuse module labels while the file set is unchanged | F24 |
| [0045](0045-hosted-notes-outside-the-checkout.md) | Hosted note files live beside the checkout | F30 |
| [0046](0046-small-local-models-for-the-cheap-tier.md) | Small local models only for the cheap tier, never bundled (proposed) | R6 |
| [0047](0047-bge-m3-and-qdrant.md) | BGE-M3 and Qdrant for the semantic index | R1 |
| [0048](0048-optional-rust-analyzer-pass.md) | The rust-analyzer pass is opt-in | R7 |

## Pending: rationale not recorded

None. Every survey candidate now has a record (R1, R6 and R7 resolved on 2026-10-14).

## Format

Each record has `Status`, `Context`, `Decision`, `Alternatives considered`, `Consequences` and `Evidence`. Supersede a record with a new one; do not rewrite history.

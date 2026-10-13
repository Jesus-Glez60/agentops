# Development workflow

How to build, test, release and change AgentOps, including the Claude Code setup the repo ships for its own development. Read this before your first change.

## Repository layout

| Path | Contents |
| --- | --- |
| `Cargo.toml` | Workspace root: 32 crates under `agentops-core/crates/`, 4 under `docbrain-core/crates/` |
| `.cargo/config.toml` | Build-time env: tree-sitter grammars, Windows CRT flags |
| `apps/web`, `apps/marketing` | Frontends ([10-frontends.md](10-frontends.md)) |
| `npm/agentops-cli` | The npm wrapper that downloads the CLI binary |
| `agentops-core/prompts` | Prompt pack: skills, agents, shared text ([13-agent-integration.md](13-agent-integration.md)) |
| `deploy/k8s`, `Dockerfile`, `docker-compose.yml`, `ecosystem.config.js`, `install.sh` | Deployment ([11-deployment-and-operations.md](11-deployment-and-operations.md)) |
| `docs/` | `openapi.yaml`, `vault-protocol.md`, and these engineering docs |
| `.claude/` | Claude Code settings, hook, skills and agents for working on this repo |

## Build and test

```sh
cargo build                       # whole workspace
cargo test                        # whole workspace
cargo test -p agentops-retrieval  # one crate
cd apps/web && npm ci && npm test # web unit tests
cd apps/marketing && npm ci && npm test
```

`cargo test` over the whole workspace is slow and needs a lot of disk; prefer `-p <crate>` while iterating.

### Build-time configuration

`.cargo/config.toml` sets `TSLP_LANGUAGES = "python,javascript,typescript,go,rust,cpp,csharp"`. `tree-sitter-language-pack` compiles exactly those grammars at build time. Its `download` feature, which would fetch grammars at run time, is disabled in the root `Cargo.toml`. Adding a scanner language means changing both `Language` in `agentops-scanner` and this list.

On Windows, `CFLAGS`/`CXXFLAGS` force `/MD`. The C++ and C# grammars' external scanners otherwise defaulted to the static CRT and failed to link against Rust's dynamic CRT (gotcha 54110).

Docker and Linux release builds need Debian trixie or another GCC 13+ toolchain, because of `ort` (gotcha 53991).

### Tests that need services

| Tests | Needs | Without it |
| --- | --- | --- |
| `agentops-graph-pg`, parts of `agentops-store-open` | `AGENTOPS_TEST_DATABASE_URL` pointing at Postgres with pgvector | skip silently and pass (gotcha 54247) |
| `agentops-heavy-embeddings`, parts of `agentops-heavy-mcp` | `AGENTOPS_TEST_QDRANT_URL` | skip |
| `#[ignore]` tests in `agentops-heavy-consolidate` | model download, minutes of CPU | not run by default |
| Tests that run `git` or `ssh` | a clean environment: a default branch name, a passwd entry, no worktree `.git` file | fail in containers and worktrees (gotcha 54248) |

A green run without the Postgres URL says nothing about Postgres. A Postgres-only bug in `upsert_session_usage` reached production that way (gotcha 54237). [02-crate-reference.md](02-crate-reference.md) lists test counts per crate.

### Static checks

- `cargo clippy` and `cargo fmt` follow the workspace defaults; no project config overrides them.
- `cargo deny check` enforces `deny.toml` ("Supply chain" below).
- `apps/web`: `npm run lint` (ESLint with `eslint-config-next`).

## Continuous integration

`.github/workflows/` contains only `release.yml`, which builds and publishes but runs no tests. Nothing in the repo describes where tests run before merge or deploy (`questions.md` Q8). Until that is answered, run the relevant tests yourself before pushing.

## Supply chain

`deny.toml` is the dependency policy for `cargo deny`:

- **Network crates banned:** `reqwest`, `hyper-tls`, `native-tls` and `openssl` are denied. Rust HTTP calls use `ureq` instead. The scanner must have no runtime network egress, and the ban enforces it. `qdrant-client` needs its default features off, or it pulls in `reqwest` (gotcha 54058).
- **Sources:** crates.io only; unknown registries and git sources are denied.
- **Licenses:** permissive licenses only, from an allowlist.
- **Ignored advisories**, each with a reason in `deny.toml`:

| Advisory | Crate and path | Reason given |
| --- | --- | --- |
| RUSTSEC-2025-0119 | `number_prefix` via `indicatif` | unmaintained, not a vulnerability; no upgrade upstream |
| RUSTSEC-2024-0436 | `paste` via `tokenizers` → `fastembed` | unmaintained, not a vulnerability; no upgrade upstream |
| RUSTSEC-2023-0071 | `rsa` via `jsonwebtoken` → `agentops-github-app` | timing side channel in PKCS#1 v1.5; the key only signs GitHub App JWTs and is not exposed as an oracle |

Dependabot (`.github/dependabot.yml`) opens weekly updates for Cargo, `apps/web`, `npm/agentops-cli` and GitHub Actions. Actions in `release.yml` are pinned by commit SHA.

## Release

Pushing a tag matching `v*` runs `.github/workflows/release.yml`:

1. Build `agentops`, `agentops-server` and `agentops-mcp-server` for `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `aarch64-apple-darwin` and `x86_64-pc-windows-msvc`.
2. Build the web app's standalone bundle.
3. Attach all archives to a GitHub release.
4. Publish `npm/agentops-cli` with its version set from the tag.

There is no Intel Mac build: `ort-sys` ships no prebuilt ONNX Runtime for `x86_64-apple-darwin` (workflow comment).

The npm package runs nothing at install time. Its `bin/agentops.js` downloads the matching release binary the first time `agentops` runs (`SECURITY.md`, "Supply chain notes").

A pushed tag is public. Moving or re-pushing one changes what users download.

## Claude Code setup for this repo

The repo ships the same prompt pack AgentOps installs into user projects, applied to itself.

### Settings and hook

`.claude/settings.json` pre-approves `mcp__agentops__add_note` and `mcp__agentops__ingest_notes`. It also installs `.claude/hooks/block-local-notes.sh` as a `PreToolUse` hook on `Write|Edit`. While `.context/agentops-remote.json` exists, the hook denies writes under `.agentops/notes/`, so notes go to the server ([06-notes-and-consolidation.md](06-notes-and-consolidation.md), "Local-file guard").

### Skills

`.claude/skills/` holds 16 skills generated from `agentops-core/prompts/` by the Ruler bridge. Edit the source under `agentops-core/prompts/`, never the generated copy ([13-agent-integration.md](13-agent-integration.md)).

| Skill | Use |
| --- | --- |
| `session` | Check recorded gotchas and decisions before starting a task. |
| `plan` | Plan with the reuse-before-writing ladder. |
| `audit-plan` | Audit a plan against the ladder and recorded knowledge, then a size-gated review. |
| `ponytail-audit` | Check uncommitted changes against the ladder. |
| `wrap` | End-of-session notes sweep, final audit and diff review. |
| `save-note` | Record a gotcha, decision or note through `add_note`. |
| `library-docs` | Look up library docs through docbrain first. |
| `planning-and-task-breakdown`, `spec-driven-development`, `constraint-driven-development` | Specs, task lists and quality bars. |
| `context-engineering`, `idea-refine`, `interview-me`, `find-skills`, `project-blueprint`, `reverse-engineer` | General workflow helpers. |

### Review agents

`.claude/agents/` holds four subagents:

| Agent | Role | Used by |
| --- | --- | --- |
| `plan-council-member` | Reviews a plan or diff, alone for small changes or as one of five personas for large or risky ones | `audit-plan`, `wrap` |
| `plan-council-chair` | Merges the five persona reviews into one verdict | `audit-plan`, `wrap` |
| `ponytail-auditor` | Checks code against the reuse-before-writing ladder | `ponytail-audit`, `wrap` |
| `vault-archivist` | Writes notes through `add_note`/`ingest_notes` | any agent that finds something worth recording |

The five-persona council's cost is driven mostly by the Completeness-checker's tool calls, not the chair (gotcha 53946). Run the council before committing, not after; once it ran after deploy and lost its value (gotcha 53914).

## Contribution rules

- `CONTRIBUTING.md`: forking and pull requests are closed for now; issues are open. When contributions reopen, contributors must disclose AI use and be able to explain every line.
- `CODEOWNERS` assigns review. Issue and pull-request templates are in `.github/`.
- Security issues go to the private address in `SECURITY.md`, not to public issues.
- Commit messages carry no `Co-Authored-By` or other AI-attribution trailer (root `CLAUDE.md`).
- Before writing new code, walk the reuse-before-writing ladder in `AGENTS.md`. Does it need to exist? Does the codebase, standard library, platform or an installed dependency already cover it? Would one line do?
- Record gotchas and decisions as they come up, through `add_note` ([06-notes-and-consolidation.md](06-notes-and-consolidation.md)).
- `AGENTS.md` and `CLAUDE.md` are generated, untracked and fully overwritten by `agentops install`/`connect` (gotcha 54112). Do not re-run those commands in this repo expecting hand edits to survive.

## Open questions

- [Q8](questions.md): where CI runs and whether this doc should describe it.

# AgentOps

**Turn your codebase into a knowledge graph AI coding agents can actually query — scanning, hybrid search, curated gotchas/decisions, and library docs, exposed over MCP and all self-hosted.**

[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

## What is AgentOps

AgentOps scans a repository into a persistent knowledge graph — files, symbols, dependencies, and how they relate — so an AI coding agent doesn't re-derive context from scratch every session. On top of that graph it layers hybrid search (dense embeddings + lexical/BM25 + exact-name matching, personalized-PageRank graph expansion), a curated Gotchas/Decisions notes system for the tribal knowledge that never makes it into code comments, and CLS-inspired local consolidation that tunes retrieval to a repo's own graph.

All of it is exposed to AI coding agents (Claude Code, Cursor, Codex CLI, Gemini CLI, and anything else that speaks MCP) through one MCP server — up to 38 tools for code graph, notes, tasks, sessions, and library docs, depending on the surface and access mode — plus a REST API and a web UI for humans: team management, a repo connection wizard, an interactive graph viewer, a gotcha curation workspace, usage/token tracking, and a library/docs tracker (`docbrain`) that gives your dependencies' documentation the same scan-and-search treatment.

It's a single open-core product: one Rust server process, one Next.js frontend, no price gate, one required env var to self-host.

## Features

- **Knowledge graph & search** — AST-aware scanning (Python, JS/TS, Go, Rust, C++, C#), dependency graph, hybrid semantic + lexical search, interactive graph viewer with a centrality "hotspot" overlay. Optional rust-analyzer pass (`install --with-lsp`) adds symbols for Rust macro invocations Tree-sitter can't see through.
- **Gotchas & decisions** — a curation workflow for the "why," not just the "what": notes are matched to the symbols they're about, surfaced to agents automatically, and flagged stale when that symbol changes.
- **Continuous indexing** — repos connected through the GitHub App reindex on every push (clone → scan → embed → docgen); a tracked branch can be picked per connection.
- **MCP for AI agents** — one stdio binary (`agentops-mcp-server`) locally, or the authenticated `/mcp` endpoint on a hosted server. Read-only (`advisor`) by default; write tools are hidden entirely, not just discouraged, until an org switches to `full`.
- **Library & docs tracking (`docbrain`)** — registers your dependencies, scrapes their docs at the version you actually use (including `llms.txt` where available), and makes them searchable the same way your own code is.
- **Usage & token tracking** — parses Claude Code session transcripts into per-session token/cost totals, records LLM spend for AgentOps's own LLM calls, and replays sessions offline to show where context went (`agentops usage`).
- **Claude Code hooks (opt-in)** — capture oversized tool output as searchable, redacted nodes instead of inlining it, and optionally compress noisy command output before it reaches context.
- **Team management** — orgs, fixed and custom roles, invites, per-repo access, an audit log, multi-org switching. Self-hosted, no external identity provider required.
- **Local consolidation** — at `end_session`, trains a small projection head on the repo's own graph, entirely locally, and promotes it only if it improves retrieval on a held-out split. (Fine-tuning a small code model on curated notes is built, but stays unexposed until something uses the result.)

## Quick start: run the server

Every method needs `AGENTOPS_SECRETS_MASTER_KEY` (generate with `openssl rand -hex 32`). Any deployment reachable from another machine (Docker, Kubernetes, PM2, a LAN or public host) also needs `AGENTOPS_API_KEY_HASH` (generate with `agentops api-key generate`); the server refuses to start on a non-loopback address without it. `agentops init` and the `/setup` page generate one for you. Everything else is optional and documented in [`.env.example`](.env.example).

### Docker (recommended)

```sh
cp .env.example .env   # fill in AGENTOPS_SECRETS_MASTER_KEY and AGENTOPS_API_KEY_HASH
docker compose up
```

The `app` service runs both the API (`:8420`) and the web UI (`:3000`); the `marketing` service serves the public site (`:4321`). Optional profiles: `--profile postgres` adds a bundled Postgres (pgvector) for the code-graph store (SQLite is the default, no extra service), and `--profile caddy` adds a TLS reverse proxy — see [`Caddyfile.example`](Caddyfile.example).

### Classic (binary + terminal wizard)

```sh
curl -fsSL https://raw.githubusercontent.com/Jesus-Glez60/agentops/main/install.sh | sh
agentops init
```

Downloads the platform binary and web build, then walks you through setup (master key, optional Postgres, signup mode) and starts both processes.

### PM2

```sh
cargo build --release --bin agentops-server
(cd apps/web && npm ci && npm run build)
pm2 start ecosystem.config.js
```

Visit `/setup` on first run to configure infra without editing `.env` by hand.

### Kubernetes

Raw manifests (Deployment/Service/Ingress/PVC, marketing site, optional Postgres StatefulSet) at [`deploy/k8s/`](deploy/k8s/) — copy `secret.yaml.example` to `secret.yaml` and fill in the master key first.

Whichever method, the first visitor to `/login` sets up the org and becomes Owner; signup is invite-only after that by default (`AGENTOPS_SIGNUP_MODE`).

### Optional server features

| Feature | Set |
|---|---|
| Postgres code-graph store (needed for multi-tenant hosting) | `AGENTOPS_DATABASE_URL` |
| GitHub App repo connections + push-triggered reindex | `AGENTOPS_GITHUB_APP_ID`, `AGENTOPS_GITHUB_APP_PRIVATE_KEY[_PATH]`, `AGENTOPS_GITHUB_WEBHOOK_SECRET`, `AGENTOPS_GITHUB_APP_SLUG` |
| Browser-based CLI login (device flow) | `AGENTOPS_WEB_APP_URL` |
| LLM-assisted features (`explain`, note classification, module labels, task summaries) | `AGENTOPS_ANTHROPIC_API_KEY`; optional `AGENTOPS_ANTHROPIC_MODEL` / `AGENTOPS_ANTHROPIC_MODEL_CHEAP` |
| Heavy semantic search (BGE-M3 + Qdrant) | `AGENTOPS_QDRANT_URL`, `AGENTOPS_QDRANT_COLLECTION` |
| Linear task sync | `AGENTOPS_LINEAR_API_KEY`, `AGENTOPS_LINEAR_WEBHOOK_URL` |

LLM features are opt-in: scanning, search, notes, and docs all work with no API key.

## Quick start: connect your coding tool

This part runs on your own machine, against your own repo. `npx agentops-cli` downloads the right platform binary on first run (macOS, Linux, Windows; x64 or arm64) — the bare `agentops` binary from the Classic install is the same thing.

### Against a shared/hosted server

One command, from inside the repo:

```sh
curl -fsSL https://<your-agentops-server>/connect.sh | sh
```

It installs the CLI if needed and runs `agentops connect --remote <server> --device-login`: a browser window asks you to approve the device, a personal API key is minted for you, and your coding tool gets pointed at the server's `/mcp` endpoint. No key copy-pasting and no local scan; indexing happens server-side. The equivalent by hand:

```sh
npx agentops-cli connect --remote https://<your-agentops-server>
```

Run interactively, it offers browser login (default) or pasting an existing API key from Settings → API Keys (`--api-key`). The connection is saved to `.context/agentops-remote.json`, which later commands (`usage sync`, the bundled skills) pick up automatically.

### Solo, fully local

```sh
cd your-project
npx agentops-cli install   # scan the repo into a local graph, generate AGENTS.md
npx agentops-cli connect   # register the local stdio MCP server + distribute AGENTS.md/skills
```

### Connect options

- `--agents claude,cursor,codex,gemini-cli` skips the picker. `connect` uses [Ruler](https://github.com/intellectronica/ruler), so any agent Ruler supports (Copilot, Windsurf, Aider, Zed, …) works via `--agents <id>`.
- `--global` installs skills and the MCP server at user level (e.g. `~/.claude/skills/`) instead of into one repo. An agent can then register and scan individual repos itself with the `register_repo` / `scan_repo` tools.
- `--with-hooks` (Claude Code) installs hooks that capture oversized tool output as searchable, redacted nodes. `--compress-commands` additionally rewrites known-noisy commands (test runners, `git diff`/`log`, `grep`/`rg`) so their output is compressed before it reaches context. It changes the command that actually runs, so it's a separate opt-in.
- `--access-mode full` exposes write tools to the agent (default `advisor`, read-only).

Re-run `connect` any time to add another tool.

## CLI reference

`agentops --help` for full flag detail — one line each here:

| Command | Purpose |
|---|---|
| `install` | Scan a repo into the graph and generate AGENTS.md (`--with-embeddings`, `--with-lsp`, `--notes-path`, `--dry-run`). |
| `connect` | Connect a coding tool — local stdio MCP server, or `--remote` to a hosted server (see above). |
| `status` | Show what's currently scanned for a repo. |
| `repos` | List every repo `agentops install` has run against on this machine. |
| `forget` | Remove a repo from the local manifest (graph data untouched). |
| `docgen` | Generate an onboarding/engineering doc (`repo-map.md`) from a scanned repo. |
| `changelog` | What changed in a repo's code/notes across scans. |
| `note` | Add a gotcha/decision/knowledge note, symbol-matched into the graph. |
| `ingest-notes` | Recursively ingest a notes folder (a vault or an unorganized one) into a repo's graph. |
| `search` | Search embedded symbols/notes; `--hybrid` adds lexical + exact-name signals, `--graph-expand` adds PageRank expansion. |
| `explain` | Generate an LLM explanation of a symbol and record it as a Definition node. |
| `watch` | Watch a repo and auto-rescan on file changes. |
| `sync-docs` | Scan a repo's dependencies and auto-register any docbrain doesn't know about. |
| `task` | Native tasks: `create`, `list`, `update-status`, `close`, `activity`, `summarize`, `sync-linear`. |
| `usage` | `sync` Claude Code session token/cost totals into the graph (or `--remote`); `report` replays sessions offline to show where context went and what a reset would have saved. |
| `librarian` | Evaluate whether a cheap model can catalogue this repo's gotchas: `export-labels` for hand-labeling, then `dry-run` against any Anthropic or OpenAI-compatible model. |
| `serve` | Run the stdio MCP server. |
| `serve-api` | Run the merged REST API server. |
| `docbrain-serve` | Run the docbrain-only stdio MCP server. |
| `api-key` | Generate an instance-wide API key for the REST API's optional auth. |
| `init` | Interactive first-run setup wizard for a classic terminal deployment. |
| `migrate-graph` | One-off: copy a repo's local SQLite graph into a Postgres store. |

## Web app

Search, Knowledge Graph, Documentation Viewer, Gotchas (curation), Libraries, Repositories (connect via local, SSH deploy key, or GitHub App; per-repo indexing status and branch), Settings (team, roles, integrations, MCP access mode, API keys), and Profile. The app is served under `/suite` by default (`AGENTOPS_WEB_APP_BASE_PATH`); the marketing site owns the root.

## Architecture

One Rust workspace (`agentops-core/crates/*`, `docbrain-core/crates/*`) builds three binaries:

- `agentops-server` — the merged REST API (code graph, docbrain under `/docbrain`, accounts/teams/repos/indexing) plus the authenticated `/mcp` endpoint and `/connect.sh`.
- `agentops-mcp-server` — the unified stdio MCP server that `connect` registers locally.
- `agentops` — the CLI.

Plus two frontends: `apps/web` (Next.js) and `apps/marketing` (Astro). Storage is SQLite by default, with Postgres (pgvector) for the code graph when `AGENTOPS_DATABASE_URL` is set. Embeddings run locally (ONNX), with no API cost.

- **API reference**: [`docs/openapi.yaml`](docs/openapi.yaml). Open [`docs/index.html`](docs/index.html) for the rendered version (regenerate after editing: `npx @redocly/cli build-docs docs/openapi.yaml -o docs/index.html`).
- **Engineering docs**: [`docs/engineering/00-index.md`](docs/engineering/00-index.md): architecture, data model, contracts, security, operations and ADRs. (`repo-map.md` is generated locally by `agentops docgen` and not committed.)
- **LLM-friendly summary**: [`llms.txt`](llms.txt) / [`llms-full.txt`](llms-full.txt), also served at `/llms.txt` on a running instance.
- **Notes protocol**: [`docs/vault-protocol.md`](docs/vault-protocol.md) explains how agents record gotchas/decisions (remote `add_note` vs. local files).
- **Consuming these docs from an agent**: a local `agentops-server` registers AgentOps as a `docbrain` library (metadata only) on every boot, in its own single-user docbrain store. Run `scrape_library` with `slug: agentops` once, and `search_docs`/`get_docs` then work against AgentOps's own docs. Hosted org members do not see this entry, since each org has its own docbrain store; register it like any other library there.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for current contribution status. Security issues: [SECURITY.md](SECURITY.md).

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE), at your option.

# AgentOps

**Turn your codebase into a knowledge graph AI coding agents can actually query — scanning, hybrid search, curated gotchas/decisions, and 35 MCP tools, all self-hosted.**

[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

## What is AgentOps

AgentOps scans a repository into a persistent knowledge graph — files, symbols, dependencies, and how they relate — instead of leaving an AI coding agent to re-derive context from scratch every session. On top of that graph it layers hybrid search (dense embeddings + lexical/BM25 + exact-name matching, personalized-PageRank graph expansion), a curated Gotchas/Decisions notes system for the tribal knowledge that never makes it into code comments, and CLS-inspired local consolidation that can fine-tune a small model on a repo's own curated notes.

All of it is exposed to AI coding agents (Claude Code, Cursor, and anything else that speaks MCP) via 35 MCP tools across three servers, plus a REST API and a web UI for humans — team management, a repo connection wizard, an interactive graph viewer, and a library/docs tracker (`docbrain`) that does the same scan-and-search treatment for your dependencies' documentation.

It's a single open-core product: one Rust server process, one Next.js frontend, no price gate, minimal required config (one env var to self-host).

## Features

- **Knowledge graph & search** — AST-aware scanning (Python, JS/TS, Go, Rust, C++, C#), dependency graph, hybrid semantic + lexical search, interactive graph viewer.
- **Gotchas & decisions** — a curation workflow for the "why," not just the "what," matched to the symbols they're about and surfaced to agents automatically.
- **MCP tools for AI agents** — 35 tools across three MCP servers (repo scanning/notes/search, semantic search/consolidation, library docs/changelogs) so an agent can query real project context instead of guessing.
- **Library & docs tracking (`docbrain`)** — registers your dependencies, scrapes their docs, and makes them searchable the same way your own code is.
- **Team management** — org/roles/invites/audit log, self-hosted, no external identity provider required.
- **Local model consolidation** — fine-tunes a small model on a repo's own curated notes, entirely locally.

## Quick start

Three ways to self-host, all shipped and tested end to end. Every method needs exactly one required value: `AGENTOPS_SECRETS_MASTER_KEY` (generate with `openssl rand -hex 32`).

### Docker (recommended)

```sh
cp .env.example .env   # fill in AGENTOPS_SECRETS_MASTER_KEY
docker compose up
```

Single image, both the API (`:8420`) and the web UI (`:3000`). Add `--profile postgres` to also start a bundled Postgres for the code-graph store — SQLite (no extra service) is the default.

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

Visit `/setup` on first run to configure infra without touching `.env` by hand.

### Kubernetes

Raw manifests (Deployment/Service/Ingress/PVC, optional Postgres StatefulSet) at [`deploy/k8s/`](deploy/k8s/) — copy `secret.yaml.example` to `secret.yaml` and fill in the master key first.

Whichever method, the first visitor to `/login` sets up the org (they become Owner automatically); every deployment defaults to invite-only signup after that.

## CLI reference

`agentops --help` for full flag detail — one line each here:

| Command | Purpose |
|---|---|
| `install` | Scan a repo into the graph and generate AGENTS.md. |
| `status` | Show what's currently scanned for a repo. |
| `repos` | List every repo `agentops install` has ever run against on this machine. |
| `forget` | Remove a repo from the manifest (graph data untouched). |
| `docgen` | Generate an onboarding/engineering doc (`repo-map.md`) from a scanned repo. |
| `changelog` | What changed in a repo's code/notes across scans. |
| `note` | Add a gotcha/decision/knowledge note, symbol-matched into the graph. |
| `ingest-notes` | Recursively ingest a notes folder (a vault or unorganized one) into a repo's graph. |
| `search` | Dense-vector search over embedded symbols/gotchas/decisions/notes. |
| `explain` | Generate an LLM explanation of a symbol and record it as a Definition node. |
| `watch` | Watch a repo and auto-rescan on file changes. |
| `serve` | Run the stdio MCP server. |
| `serve-api` | Run the merged REST API server. |
| `docbrain-serve` | Run the docbrain stdio MCP server (library docs/changelogs). |
| `sync-docs` | Scan a repo's dependencies and auto-register any docbrain doesn't know about. |
| `api-key` | Generate a new API key for the REST API's optional auth. |
| `task` | Manage native tasks (create/list/update-status/activity/summarize/sync-linear). |
| `init` | Interactive first-run setup wizard for a classic terminal deployment. |

## Architecture

One Rust workspace (`agentops-core/crates/*`, `docbrain-core/crates/*`) building a merged REST/MCP server (`agentops-server`) plus a stdio MCP binary (`agentops-mcp-server`) and a CLI (`agentops`), and one Next.js frontend (`apps/web`). [`repo-map.md`](repo-map.md) is a generated architecture map — produced by AgentOps scanning its own codebase.

- **API reference**: [`docs/openapi.yaml`](docs/openapi.yaml) — open [`docs/index.html`](docs/index.html) in a browser for the rendered version (regenerate after editing the spec: `npx @redocly/cli build-docs docs/openapi.yaml -o docs/index.html`).
- **LLM-friendly summary**: [`llms.txt`](llms.txt) / [`llms-full.txt`](llms-full.txt) — also served at `/llms.txt` on a running instance.
- **Consuming these docs from an agent**: AgentOps registers itself as a `docbrain` library on first boot — run the `scrape_library` MCP tool (or CLI) with `slug: agentops` once, then `search_docs`/`get_docs` work against AgentOps's own docs the same way they would for any dependency.

## Contributing

No `CONTRIBUTING.md` yet — issues and PRs are still welcome, just no formal process written down.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE), at your option.

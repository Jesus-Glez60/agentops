# agentops-heavy

**Commercial license — not covered by the root repo's MIT/Apache-2.0 license.**
See `LICENSE-COMMERCIAL.md`. This is a deliberately separate Cargo workspace
(its own `Cargo.toml`), not a member of the root workspace, so the two never
share a dependency graph or a license posture — `cargo deny check` at the
repo root has no visibility into this directory, and vice versa.

## What lives here

Per the plan's phased rollout, this is the differentiated, revenue-line part
of the product: persistent, scalable graph storage (Postgres) behind the
same `GraphStore` trait the light tier (`agentops-graph`'s
`SqliteGraphStore`) already implements, semantic search over that graph
(BGE-M3 + Qdrant), license-key gating, and hosted repo access — both the
SSH-deploy-key path and the GitHub App client code now exist (the GitHub
App itself isn't registered on github.com yet, so that path is
code-complete but operationally unverified — see `agentops-github-app`).

## Structure

```
agentops-heavy/
  Cargo.toml                    # separate workspace
  LICENSE-COMMERCIAL.md
  crates/
    agentops-graph-pg/           # PostgresGraphStore — same GraphStore trait as the light tier
    agentops-embeddings/         # BGE-M3 + Qdrant semantic search over the graph
    agentops-license/            # offline license-key verification, gates heavy-tier activation
    agentops-repo-access/        # per-repo SSH deploy-key custody + connection store
    agentops-github-app/         # GitHub App JWT signing + installation-token exchange
    agentops-heavy-api/          # REST server: repo-connection flow + /search, wraps the crates above
    agentops-heavy-mcp/          # stdio MCP server: semantic_search/semantic_index tools for agent use
  docker/
    docker-compose.yml           # Postgres + Qdrant, parameterized via .env (never committed)
    postgres-init/                # idempotent schema migrations, run on first container start
```

## Semantic search (`agentops-embeddings`)

The structural graph (`agentops-graph`) is precise but exhaustive — `list_gotchas`
or a symbol lookup returns everything of a kind, and a caller (often an LLM
agent, paying in context tokens) reads past it to find what's relevant.
`SemanticIndex` answers "what's actually relevant to this query" directly:
BGE-M3 dense embeddings (the same model the original codebrain/docbrain
used), generated locally via ONNX (`fastembed` — no Python runtime, no
external embedding API, code/docs never leave the process to get embedded),
indexed into Qdrant.

- `SemanticIndex::connect(qdrant_url, collection)` then `.ensure_collection()`.
- `collect_index_items(store, repo)` — a plain **synchronous** free function
  reading every Symbol/Gotcha/Decision node out of a `GraphStore` into
  embeddable items. Deliberately separate from the async embedding step
  below and deliberately not a method on `SemanticIndex`: `&dyn GraphStore`
  isn't provably `Sync` (`SqliteGraphStore` wraps a `rusqlite::Connection`,
  intentionally `!Sync` upstream), so a reference to it can never be held
  across an `.await` — an async function taking it directly compiles fine
  in isolation but silently produces a `!Send` future the moment something
  (like an axum handler) actually requires `Send`. Keeping the two steps
  structurally separate is what makes that mistake hard to reintroduce.
- `.index_items(items)` — embeds and upserts owned items; re-running after a
  rescan overwrites stale entries (node ids are reused as Qdrant point ids)
  instead of duplicating them.
- `.search(query, top_k, repo)` — real semantic ranking, not keyword
  matching. Verified live against the real BGE-M3 model and a real Qdrant
  instance: a query with zero keyword overlap with its target text still
  ranks the semantically related item first (`cargo test -p agentops-embeddings`
  with `AGENTOPS_TEST_QDRANT_URL` set — downloads the real ~2GB model on
  first run, cached after).
- Run `cargo run --release -p agentops-embeddings --example index_and_query --
  <graph.db path> <repo name> <query...>` to index a real scanned repo and
  query it directly from the command line.

**Where this is actually exposed:**
- `agentops-heavy-api`: `POST /search/index` / `GET /search` — for the
  dashboard. Requires `AGENTOPS_QDRANT_URL` and a valid `AGENTOPS_LICENSE_KEY`
  (semantic search is paid-tier; both routes return `402 Payment Required`
  otherwise, rather than the server refusing to start).
- `agentops-heavy-mcp`: `semantic_search`/`semantic_index` tools over stdio
  JSON-RPC — for actual agent use in Claude Code. Same license requirement,
  but stricter: this binary refuses to start at all without one, since an
  MCP server with zero tools isn't useful to hand an agent. Run it with
  `AGENTOPS_LICENSE_KEY` and `AGENTOPS_QDRANT_URL` set:
  `cargo run --release -p agentops-heavy-mcp`.

## Running locally

```
cd docker
cp .env.example .env   # fills in a random Postgres password — see .env.example
docker compose up -d
```

Never commit `.env` — it's gitignored at the repo root already
(`.env` is in the root `.gitignore`'s secrets section).

## License-key gating

`agentops-license` verifies offline, Ed25519-signed license keys — asymmetric,
not HMAC, so a shipped binary can verify a key without holding any secret
that would let its holder forge one. Only `PRODUCTION_PUBLIC_KEY` (a public
key, safe to embed) lives in source; the matching private key is held
offline, never committed, never shipped — see the note left in the session
scratchpad the day this was set up for where it currently lives, and move it
to a real secrets manager if it's still sitting there.

- `agentops_license::verify_production_license(key)` — what a heavy-tier
  binary calls at startup to gate activation.
- `cargo run -p agentops-license --example sign_license -- <licensee> [expires_at_unix] [seat_limit]`
  — issuer-side only, requires `AGENTOPS_LICENSE_PRIVATE_KEY_HEX` set to the
  offline private key. Never run this with the key inline on a command line
  (shell history); source it from a file or secrets manager into the env var.
- `cargo run -p agentops-license --example verify_license -- <key>` — sanity-check
  a key against the embedded production public key.

## Hosted repo access

Two credential paths, one connection store, one REST API — see
`SECURITY.md` for the full verification detail on each.

**SSH deploy keys** (`agentops-repo-access`) — a dedicated Ed25519 keypair
per connected repo, encrypted with OpenSSH's own bcrypt-pbkdf+AES-256-CTR
before it's ever returned from `generate_deploy_keypair_for_repo`, decrypted
only into a `0600` temp file for the duration of one clone/fetch subprocess
(`UnlockedKey`, zeroed and deleted on `Drop`, including panic unwind). The
passphrase is derived per-repo by `secrets::SecretsProvider` — the shipped
`EnvSecretsProvider` is real and correct for one self-hosted deployment;
a KMS/Vault-backed implementation is still needed before any real paying
customer's key is generated (this is the one item worth treating as a hard
blocker, not a nice-to-have — see `SECURITY.md`).

**GitHub App** (`agentops-github-app`) — the plan's recommended *primary*
path, since GitHub holds the credential and an org admin can revoke access
from GitHub's own UI instead of us custodying a private key at all.
`generate_app_jwt`/`get_installation_token` implement the auth flow;
`install_url(app_slug)` builds the install link. No App is registered on
github.com yet, so this path is code-complete but hasn't been exercised
against GitHub's real API.

**Connection store + API** (`agentops-repo-access::store`,
`agentops-heavy-api`) — SQLite, tenant-scoped at the primary-key level
(`(tenant, id)`, not an app-level filter). `agentops-heavy-api` exposes
`POST /repos/connect`, `GET /repos`, `POST /repos/{id}/verify`, and
`GET /repos/github-app/install-url`, behind the same API-key middleware as
`agentops-api`/`docbrain-api`. Run it via
`cargo run -p agentops-heavy-api --bin agentops-heavy-api`
(env: `AGENTOPS_SECRETS_MASTER_KEY` required, `AGENTOPS_HEAVY_API_ADDR`/
`AGENTOPS_HEAVY_API_DB`/`AGENTOPS_HEAVY_API_KEY_HASH`/
`AGENTOPS_GITHUB_APP_SLUG` optional). The dashboard page at
`apps/web/src/app/repos/connect` drives it — both the SSH flow and the
GitHub App install link.

**Still open**: the dashboard has no callback handling for a completed
GitHub App install (recording which installation ID belongs to which
tenant), and neither path is wired to an actual repo sync/ingestion
pipeline yet — this is credential custody, not clone-and-index.

# agentops-heavy

**Commercial license — not covered by the root repo's MIT/Apache-2.0 license.**
See `LICENSE-COMMERCIAL.md`. This is a deliberately separate Cargo workspace
(its own `Cargo.toml`), not a member of the root workspace, so the two never
share a dependency graph or a license posture — `cargo deny check` at the
repo root has no visibility into this directory, and vice versa.

## What lives here

Per the plan's phased rollout, this is the differentiated, revenue-line part
of the product: persistent, scalable graph storage (Postgres, eventually
Qdrant for embeddings) behind the same `GraphStore` trait the light tier
(`agentops-graph`'s `SqliteGraphStore`) already implements, plus the Docker
packaging to run it, license-key gating, and hosted repo access — both the
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
    agentops-license/            # offline license-key verification, gates heavy-tier activation
    agentops-repo-access/        # per-repo SSH deploy-key custody + connection store
    agentops-github-app/         # GitHub App JWT signing + installation-token exchange
    agentops-heavy-api/          # REST server: repo-connection flow, wraps the crates above
  docker/
    docker-compose.yml           # Postgres + Qdrant, parameterized via .env (never committed)
    postgres-init/                # idempotent schema migrations, run on first container start
```

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

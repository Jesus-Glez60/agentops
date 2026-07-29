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
packaging to run it, license-key gating, and hosted repo access via SSH
deploy keys (GitHub App install flow, the plan's recommended primary path,
is still future work — see `agentops-repo-access`'s crate docs).

## Structure

```
agentops-heavy/
  Cargo.toml                    # separate workspace
  LICENSE-COMMERCIAL.md
  crates/
    agentops-graph-pg/           # PostgresGraphStore — same GraphStore trait as the light tier
    agentops-license/            # offline license-key verification, gates heavy-tier activation
    agentops-repo-access/        # per-repo SSH deploy-key custody, clone/fetch over pinned SSH
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

## Hosted repo access (SSH deploy keys)

`agentops-repo-access` generates a dedicated Ed25519 keypair per connected
repo (never shared across repos — a compromised key should only expose one
repo), encrypts the private key with OpenSSH's own bcrypt-pbkdf+AES-256-CTR
before it's ever returned from `generate_deploy_keypair`, and only decrypts
it into a `0600` temp file for the duration of one clone/fetch subprocess
(`UnlockedKey`, zeroed and deleted on `Drop`, including on panic unwind).

- `generate_deploy_keypair(comment, passphrase)` — returns the public key
  (paste into GitHub's repo → Settings → Deploy Keys) and the encrypted
  private key (what gets persisted).
- `UnlockedKey::unlock(encrypted_key, passphrase)` → `clone_repo`/`fetch_repo`
  — pin the SSH host key against `GITHUB_KNOWN_HOSTS` (fetched live from
  `https://api.github.com/meta`, not hand-typed) rather than trust-on-first-use,
  which would accept a MITM's key just as readily as GitHub's real one.
- `passphrase` is a library parameter, not something this crate sources
  itself — in production it must come from a real secrets manager (KMS/Vault),
  per-tenant, not a hardcoded value or a plain database column.
- **Not yet built**: the GitHub App install flow (recommended as the primary
  path since it avoids private-key custody on our side entirely), the
  dashboard's repo-connection UI, and where the per-tenant passphrase/wrapping
  key actually lives in a deployed instance. Tracked in `SECURITY.md`.

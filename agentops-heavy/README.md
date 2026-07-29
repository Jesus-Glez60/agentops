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
packaging to run it, license-key gating, and (future work) hosted repo access
via GitHub App / SSH deploy keys.

## Structure

```
agentops-heavy/
  Cargo.toml                    # separate workspace
  LICENSE-COMMERCIAL.md
  crates/
    agentops-graph-pg/           # PostgresGraphStore — same GraphStore trait as the light tier
    agentops-license/            # offline license-key verification, gates heavy-tier activation
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

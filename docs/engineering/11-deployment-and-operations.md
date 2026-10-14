# Deployment and operations

How to configure and run AgentOps as a service, where its data lives, and what to check when something breaks. Read this before deploying, upgrading or debugging a running instance.

The runbook draws on gotchas recorded while building and operating AgentOps; the author confirmed they cover every failure hit during development (`outlines.md`, "Author review"). Storage layout is in [03-data-model.md](03-data-model.md); auth settings in [09-auth-teams-security.md](09-auth-teams-security.md).

## What runs

| Process | Binary or entry | Port | Role |
| --- | --- | --- | --- |
| API server | `agentops-server` | 8420 (`AGENTOPS_ADDR`) | All REST routes, hosted `/mcp`, webhooks, indexing jobs |
| Web app | `apps/web/.next/standalone/server.js` | 3000 (`PORT`) | Product UI under `/suite`; calls the API over `AGENTOPS_HEAVY_API_URL` |
| Marketing site | `apps/marketing` static build | 4321 | Public site at the root |
| Postgres with pgvector | optional | 5432 | Shared code-graph store |
| Qdrant | optional | 6333 | Semantic index, only with `AGENTOPS_QDRANT_URL` |

Developers and agents run `agentops-mcp-server` locally over stdio; it is not part of a server deployment.

## Configuration reference

Configuration comes from the process environment only. No Rust crate loads a `.env` file (gotcha 54032). Docker Compose reads `.env` through `env_file`, and the PM2 config parses it itself (`loadDotEnv` in `ecosystem.config.js`).

`.env.example` documents the variables below. `AGENTOPS_SECRETS_MASTER_KEY` is always required. `AGENTOPS_API_KEY_HASH` is required whenever `AGENTOPS_ADDR` is not loopback-only, which includes Docker, Kubernetes and PM2; the server refuses to start without it.

| Concern | Variables | Default |
| --- | --- | --- |
| Secrets | `AGENTOPS_SECRETS_MASTER_KEY` (hex, at least 32 bytes) | none; required |
| Storage root | `AGENTOPS_DATA_DIR` | `~/.agentops` |
| Code graph backend | `AGENTOPS_DATABASE_URL` | unset = SQLite per repo |
| Per-store overrides | `AGENTOPS_ACCOUNTS_DB`, `AGENTOPS_TEAMS_DB`, `AGENTOPS_INTEGRATIONS_DB`, `AGENTOPS_MODULES_DB`, `AGENTOPS_HEAVY_API_DB`, `DOCBRAIN_DB_DIR`, `AGENTOPS_DOCBRAIN_DB`, `AGENTOPS_MANIFEST_PATH`, `AGENTOPS_REPO_CHECKOUTS_DIR` | under `AGENTOPS_DATA_DIR` |
| Network | `AGENTOPS_ADDR`, `AGENTOPS_PUBLIC_API_URL`, `AGENTOPS_WEB_APP_URL`, `AGENTOPS_WEB_APP_BASE_PATH` | `127.0.0.1:8420` locally, `0.0.0.0:8420` in the image; base path `/suite` |
| Access | `AGENTOPS_API_KEY_HASH`, `AGENTOPS_ALLOW_UNAUTHENTICATED`, `AGENTOPS_ACCESS_MODE`, `AGENTOPS_SIGNUP_MODE` | required off loopback; unset (start-up refused without a key); `advisor`; `open` |
| Doc scraping | `AGENTOPS_ALLOW_PRIVATE_DOC_URLS` | unset = only public addresses are fetched |
| LLM | `AGENTOPS_ANTHROPIC_API_KEY` | unset = no LLM features |
| Linear | `AGENTOPS_LINEAR_API_KEY`, `AGENTOPS_LINEAR_WEBHOOK_URL` | unset |
| GitHub App | `AGENTOPS_GITHUB_APP_ID`, `AGENTOPS_GITHUB_APP_PRIVATE_KEY` or `..._PATH`, `AGENTOPS_GITHUB_WEBHOOK_SECRET`, `AGENTOPS_GITHUB_APP_SLUG` | unset = SSH deploy keys only |
| Semantic index | `AGENTOPS_QDRANT_URL`, `AGENTOPS_QDRANT_COLLECTION` | unset; `agentops_semantic` |

Also in `.env.example` since `d9f447b` (`findings.md` F15):

| Variable | Effect | Default |
| --- | --- | --- |
| `AGENTOPS_PG_POOL_MAX_SIZE` | Postgres pool size; size it from the database server's cores | 25 |
| `AGENTOPS_PG_CONNECT_RETRY_SECS` | how long start-up retries Postgres; `0` means one try | 60 |
| `AGENTOPS_ANTHROPIC_MODEL`, `AGENTOPS_ANTHROPIC_MODEL_CHEAP` | LLM model ids | `DEFAULT_MODEL`, `DEFAULT_MODEL_CHEAP` in `agentops-llm` |
| `AGENTOPS_GITHUB_INSTALL_STATE_SECRET` | signs the GitHub App install `state` | see `github_app_routes.rs` |
| `AGENTOPS_INSTALL_DIR` | CLI install location | see `agentops-cli` |

Web-app-only variables: `AGENTOPS_HEAVY_API_URL` (API base for server-side calls, default `http://127.0.0.1:8420`), `PORT`, `HOSTNAME`. Marketing: `AGENTOPS_PUBLIC_API_URL` at build time.

`agentops init` (classic install) and the web `/setup` page (PM2 install) generate a valid `.env` through `BootstrapConfig` (`agentops-core/crates/agentops-manifest/src/bootstrap.rs`). Docker and Kubernetes skip them.

## Storage

| Data | Default location | Backend |
| --- | --- | --- |
| Platform stores (accounts, teams, integrations, modules, connections and jobs) | `<data dir>/*.sqlite` | SQLite, always |
| docbrain | `<data dir>/docbrain-tenants/<tenant>.db` (hosted), `<data dir>/docbrain.db` (CLI) | SQLite, always |
| Code graph | `<repo>/.context/graph.db` per repo, or one Postgres database | SQLite or Postgres |
| Repo checkouts | `<data dir>/repo-checkouts/<connection_id>--<tenant>` | files; deleted and re-cloned on every index |
| Consolidation models | `$AGENTOPS_DATA_DIR/models/<repo>` | files |
| Scan manifest | `<data dir>/manifest.json` | JSON file |

Postgres replaces only the code graph. Everything else stays in SQLite files on the server's disk, whatever `AGENTOPS_DATABASE_URL` says.

`AGENTOPS_DATA_DIR` defaults to `~/.agentops` for every deploy method. Two methods on one host share data unless one sets its own (gotcha 53950).

### Single instance only

Run one API server per data directory. The SQLite platform stores, docbrain files and repo checkouts are all local to one process's disk. `deploy/k8s/deployment.yaml` pins `replicas: 1`, but its comment says replicas can rise once Postgres is configured. That is not enough, because the platform stores stay in SQLite (`findings.md` F42).

## Deploy methods

### Docker Compose

`Dockerfile` builds one image with both the API and the web app, supervised by `pm2-runtime ecosystem.config.js`:

1. Rust stage on `rust:1-trixie`: `cargo build --release --bin agentops-server`.
2. Node stage on `node:22-trixie-slim`: `npm ci`, `npm run build` for `apps/web`.
3. Runtime on `node:22-trixie-slim` with `git`, `openssh-client` and PM2. It sets `AGENTOPS_ADDR=0.0.0.0:8420` and checks `GET /health` every 30 s.

Debian trixie is required. The `ort` ONNX Runtime library needs GCC 13+'s `libstdc++`, and bookworm's GCC 12 fails to link (gotcha 53991; Dockerfile comment). The runtime stage must stay on the same Debian release as the build.

`docker-compose.yml` services:

| Service | Profile | Notes |
| --- | --- | --- |
| `app` | default | ports 3000 and 8420; `.env`; volume `agentops-data` at `/root/.agentops`; `./secrets` read-only at `/run/secrets` for file secrets such as the GitHub App key |
| `marketing` | default | requires `AGENTOPS_PUBLIC_API_URL` at build time |
| `postgres` | `postgres` | pinned `pgvector/pgvector` digest; `app` waits for its health check when enabled |
| `caddy` | `caddy` | TLS front; mounts `./Caddyfile` |

Compose volumes are named after the project directory. Renaming or moving the checkout starts with empty volumes (gotcha 54249).

### Overrides

To change ports or other lists on a host, use `docker-compose.override.yml`. Compose appends list values such as `ports:` instead of replacing them (gotcha 53992). Use the `!override` tag to replace them (gotcha 54246).

### Kubernetes

`deploy/k8s/` holds a `Deployment` with one replica, `/health` probes and data on a PVC. It also has a `Service`, an `Ingress`, a Postgres `StatefulSet` with the same pinned image, marketing manifests and `secret.yaml.example`.

The example secret holds `AGENTOPS_SECRETS_MASTER_KEY`, `AGENTOPS_API_KEY_HASH` (required) and `AGENTOPS_DATABASE_URL`.

### PM2 on a host

`ecosystem.config.js` runs the release binary and the standalone web server. It reads `.env` and passes it to `agentops-server`. The web process gets `PORT`, `HOSTNAME`, `AGENTOPS_HEAVY_API_URL` and the variables it reads (`AGENTOPS_PUBLIC_API_URL`, `AGENTOPS_WEB_APP_BASE_PATH`), but not the server's secrets. On the first PM2 start outside a container, the file generates whatever is missing of `AGENTOPS_SECRETS_MASTER_KEY` and `AGENTOPS_API_KEY_HASH` into `.env` (mode 0600) and prints the raw API key once, so the server starts and `/setup` is reachable. `/setup` then writes the full `.env`, keeping the running keys (fixed for `findings.md` F41, F45). Containers set `AGENTOPS_CONTAINER=1` and never generate keys: a key written inside a container would change on every recreate.

### Classic install

`install.sh` downloads the latest GitHub release's `agentops`, `agentops-server` and `agentops-mcp-server` binaries plus the web tarball into `~/.agentops/bin` and `~/.agentops/web`, then runs `agentops init`. Releases come from `.github/workflows/release.yml`.

### Reverse proxy

`Caddyfile.example` routes `/suite/*` to the web app and everything else (`/*`) to the marketing site. The API is deliberately not routed: the web app reaches it inside the container, and `/api-external` is left commented out for direct API-key access. The same split is in `deploy/k8s/ingress.yaml`.

Any reverse proxy in front of the web app or the API must set `X-Real-IP` to the real client address and overwrite a client-sent value: nginx `proxy_set_header X-Real-IP $remote_addr`, Caddy `header_up X-Real-IP {remote_host}`. Rate limits and the Active Sessions list depend on it ([09-auth-teams-security.md](09-auth-teams-security.md), "Rate limits").

## Start-up and upgrades

On start, `agentops-server` does the following while it builds its routers (`agentops_server::run`):

1. Connects to Postgres if `AGENTOPS_DATABASE_URL` is set, retrying with jittered backoff for `AGENTOPS_PG_CONNECT_RETRY_SECS`. Before this retry, a cold container start crash-looped while Postgres was still starting (gotcha 53908, fixed in `cf408a4`).
2. Re-applies `schema.sql` on Postgres; every statement is `IF NOT EXISTS`.
3. Opens each SQLite store, which applies pending `rusqlite_migration` steps.
4. Registers `agentops` in the global docbrain store (`findings.md` F3).
5. Mounts all routers on one port.

An upgrade is therefore "replace the binary or image and restart". Migrations run on open, forward only. Rules for anyone adding one are in [03-data-model.md](03-data-model.md), "Migration rules". Two stores have no migration framework and need manual `ALTER TABLE` handling: `repo_connections` (gotcha 54216) and parts of `accounts.sqlite` (gotcha 53984).

An older per-repo `graph.db` whose `edges` table lacks `repo` fails to open (gotcha 53952). Rescanning into a fresh `.context/graph.db` is the recovery.

To move per-repo SQLite graphs into Postgres, use `agentops migrate-graph` ([03-data-model.md](03-data-model.md), "migrate-graph").

## Health and logging

- `GET /health` returns `ok` with no auth. The Docker health check and both Kubernetes probes use it. It does not check Postgres, Qdrant or disk.
- The server logs with `println!` and `eprintln!`; there is no `tracing` and no log levels (gotcha 53954). Indexing jobs also write per-job logs to `indexing_job_log`, shown in the web app's indexing view.
- LLM spend is recorded in the `llm_usage` table when a caller passes a usage sink ([02-crates/knowledge-and-retrieval.md](02-crates/knowledge-and-retrieval.md)).

## Runbook

| Symptom | Likely cause | Check or fix | Source |
| --- | --- | --- | --- |
| Server exits at start on a fresh container | Postgres not ready | Raise `AGENTOPS_PG_CONNECT_RETRY_SECS`; confirm the `postgres` profile is on | gotcha 53908 |
| New routes return 404 in development | An old process still holds the port | Find and stop the stale process on 8420 or 3000 | gotchas 54095, 54094 |
| Repo shows 0 nodes or "path missing" but data exists | Checkout path scheme changed | Reindex to recreate the checkout | gotcha 47151 |
| Hosted repo never picks up new commits after `scan_repo` | `scan_repo` rescans the existing checkout only | Use `POST /repos/{id}/index` or the Reindex button | gotcha 54077 |
| Push webhook does not reindex | Connection id changed when a Discovered connection was upgraded | Fixed by `repo_url` fallback; check the connection's `repo_url` matches the GitHub repo | gotchas 54150, 54215 |
| Semantic search finds zero nodes | Raw path used instead of `repo_name` | Use the store repo name | gotcha 54006 |
| Concurrent `/mcp` calls deadlock Postgres | Per-call pools | Fixed; check the shared pool is in use | gotchas 54023 (stale), 54028 |
| Login works but the session is lost | `Secure` cookie over plain HTTP | Serve HTTPS, or check `x-forwarded-proto` | gotcha 54081 |
| Public route redirects to login | Missing from the `proxy.ts` matcher exclusions | Add it | gotchas 54056, 54031 |
| A screen spins forever | SWR `error` not handled | Handle `error` in that hook | gotcha 53942 |
| Fresh user gets 403 on `/repos` | No membership row yet | `ensure_membership` must run first | gotcha 53997 |
| Signups fail after an upgrade | New `accounts.sqlite` columns not added | Check the accounts migration functions | gotcha 53984 |
| Background scrape stuck at `Running` | Worker could not open the docbrain store | Check the server log for `run_scrape_job` | `run_scrape_job` comment |
| `/usage/sync` returns 500 on Postgres | Type binding for timestamps | Fixed; Postgres tests skip without a live DB | gotchas 54237, 54247 |
| Docker build link error in `ort` | Bookworm base image | Use trixie | gotcha 53991 |
| Override did not change ports | Compose appends lists | Use `!override` | gotchas 53992, 54246 |
| Deploy script stops without rolling back | `set -e` exits on `compose up -d` failure | Handle the failure explicitly and re-check health after rollback | gotcha 54250 |
| `docker builder prune --keep-storage` fails | Removed in Docker 29 | Use `--max-used-space` | gotcha 54252 |
| Server exits with "refusing to start: AGENTOPS_ADDR=... is not a loopback address" | No `AGENTOPS_API_KEY_HASH` on a network-reachable bind | Run `agentops api-key generate` and set the hash | `startup_auth_check` |
| Users get 429 "too many attempts" | Auth rate limit hit | Wait for `Retry-After`; if everyone shares one bucket, the proxy isn't setting `X-Real-IP` | `rate_limit.rs` |
| `scrape_library` fails with "resolves to the non-public address" | `docs_url` points at a private or internal host | Use a public URL, or set `AGENTOPS_ALLOW_PRIVATE_DOC_URLS=1` on a trusted self-host | `net.rs` |
| `/mcp` returns "'notes_path' is not accepted on the hosted server" | Client passed `notes_path` | Drop the argument; hosted notes use the connection's folder | `mcp_http.rs` |

## Open questions

None. Q8 was answered: there is no public CI (ADR 0040). Operator findings F16, F27, F28, F41, F42 and F45 are resolved.

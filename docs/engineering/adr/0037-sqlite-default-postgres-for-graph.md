# 0037. SQLite by default, Postgres for the code graph only

Survey candidate A3. Rationale comes from the author's answer A-12 in `questions.md`.

## Status

Accepted. SQLite core from `7a393e7` (2026-07-28); Postgres `GraphStore` from `0385f5e` (2026-07-28). Rationale recorded 2026-10-13.

## Context

Every store is SQLite unless `AGENTOPS_DATABASE_URL` is set. That variable moves only the code graph (`agentops-graph-pg`) to Postgres. Accounts, teams, integrations, connections, indexing jobs and docbrain stay in SQLite files under `AGENTOPS_DATA_DIR`.

## Decision

SQLite is the default because it needs no setup for a solo user. Postgres is used only for the code graph, where graph size and concurrency on the hosted server need it. The other stores are small and stay on SQLite.

## Alternatives considered

- Postgres for every store. Not chosen: the other stores are small, and a solo install would then need a database server.

## Consequences

- One `agentops-server` process per data directory: a second replica would see different SQLite data (`findings.md` F42, now documented in the Kubernetes examples).
- Two `GraphStore` backends must stay equivalent, which is why gotchas such as 54053 and 54241 exist.

## Evidence

- Author answer A-12, `questions.md`.
- `agentops_store_open::open_store` (`agentops-core/crates/agentops-store-open/src/lib.rs`).
- `03-data-model.md`, "Platform stores".

# 0008. Isolate docbrain tenants with one SQLite file each

Survey candidate M4. Rationale is taken only from the sources under "Evidence"; nothing here is inferred.

## Status

Accepted; applied to hosted `/mcp` in `eba0e98` (2026-09-04).

## Context

The pre-rebuild code threaded a `TenantContext` through every docbrain query. The rebuilt `docbrain-graph` dropped it and is single-tenant.

## Decision

Each tenant gets its own docbrain SQLite file, `<DOCBRAIN_DB_DIR>/<org>.db` (`default.db` with no org). The API and MCP layer select the file per request. Tenancy stays out of the `DocbrainStore` trait.

## Alternatives considered

- Reintroduce tenant scoping into the trait. Rejected.

## Consequences

- Strong isolation by file, with no query that can forget a filter.
- Qdrant has no tenant field, so the shared vector index does not enforce isolation; the note calls per-tenant vector isolation future work.
- The global `/docbrain/*` router and the boot-time self-seed still use the single file (`findings.md` F3; gotcha 30341).

## Evidence

- Decision "Multi-tenant docbrain isolation via one SQLite file per tenant, not a TenantContext".
- `docbrain_db_path_for_org` in `agentops-core/crates/agentops-heavy-api/src/lib.rs`.

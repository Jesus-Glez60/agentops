---
title: "Multi-tenant docbrain isolation via one SQLite file per tenant, not a TenantContext"
type: decision
---

main's agentops-heavy-api achieved multi-tenant hosting via docbrain_graph::TenantContext/Visibility threaded through every query. docbrain-graph is single-tenant this rebuild (TenantContext deliberately dropped earlier). Decided against reintroducing tenant scoping to the trait itself; instead each tenant gets its own docbrain SQLite file (docbrain_db_dir/<org>.db, default.db when no org given), selected by the API/MCP layer per-request -- the same isolate-by-connection-string approach agentops-graph-pg already uses for repos via a plain repo column, not a type-level concept. Known, flagged limitation: Qdrant itself has no tenant field, only repo/kind filters, so the shared vector index doesn't enforce tenant isolation on its own even though each tenant's source docbrain content is now stored separately -- same trust-at-index-time posture main already had. Real per-tenant vector isolation is future work.

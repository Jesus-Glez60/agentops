# 0026. Full cascade on org deletion, except the audit log

Survey candidate S7. Rationale is taken only from the sources under "Evidence"; nothing here is inferred.

## Status

Accepted, 2026-08-17 (`1ec773a`).

## Context

Deleting an organization must remove its data across several stores.

## Decision

`POST /team/delete-organization` deletes leaf stores first, best-effort: connections, credentials, the tenant's docbrain file. It then deletes team tables in one transaction. `audit_log` rows are kept. Per-repo `.context/graph.db` files are out of scope, because the server never knows their location.

## Alternatives considered

- Delete audit rows too. Rejected: the org's own deletion should stay recorded.

## Consequences

- Later code also wipes each connection's Postgres code graph (`wipe_connection_graph`); gotcha 54151, which said otherwise, is stale (`findings.md` F10).

## Evidence

- Decision "Org deletion is a full cascade — audit_log excluded, codebrain graph out of scope".

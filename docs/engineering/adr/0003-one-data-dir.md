# 0003. One AGENTOPS_DATA_DIR for all storage defaults

Survey candidate A4. Rationale is taken only from the sources under "Evidence"; nothing here is inferred.

## Status

Accepted, with the merge (`218c3fc`).

## Context

Each crate hardcoded `$HOME/.agentops` independently, and self-hosting needed about ten storage env vars.

## Decision

Every `default_*_path()`/`default_*_dir()` helper resolves under `AGENTOPS_DATA_DIR`, default `~/.agentops`. Each specific variable still overrides its own path. `AGENTOPS_API_KEY_HASH` and `AGENTOPS_ADDR` likewise replace per-service variables.

New storage defaults under `AGENTOPS_DATA_DIR` by copying the small `agentops_data_dir()` helper into the crate, the same way each MCP crate hand-rolls its own wire types.

## Alternatives considered

- Remove the specific variables. Rejected: the note calls this "a default-setting consolidation, not a removal of flexibility".
- One shared helper crate. Not chosen; the note prefers a copied small helper.

## Consequences

- No data migration: a fresh install resolves byte-identical paths.
- `AGENTOPS_DOCBRAIN_DB` (one file, CLI store) and `DOCBRAIN_DB_DIR` (a directory of per-tenant files) are different stores and easy to confuse.
- Copies drift. The model stores and `sync_docs` never adopted the helper (`findings.md` F16, F36); a duplicated Windows fix regressed once (gotcha 53947).

## Evidence

- Decision "AGENTOPS_DATA_DIR consolidates ~10 storage env vars -- AGENTOPS_DOCBRAIN_DB is not the same thing as DOCBRAIN_DB_DIR".

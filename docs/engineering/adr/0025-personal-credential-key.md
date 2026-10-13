# 0025. Encrypt personal credentials under their own key

Survey candidate S6. Rationale is taken only from the sources under "Evidence"; nothing here is inferred.

## Status

Accepted, 2026-08-17 (`1ec773a`).

## Context

Members can store personal integration credentials next to org-wide ones.

## Decision

Derive a separate AES-256-GCM key per user: `HMAC-SHA256(master_key, "user-integration-key\0" || tenant || 0x00 || user_id)`, distinct from the org key's `"integration-key\0"` label.

## Alternatives considered

- Reuse the org key and isolate by a SQL `WHERE user_id = ?`. Rejected: any query missing the filter could decrypt another member's secret.

## Consequences

- A missing filter or a direct table read still cannot decrypt.
- All keys still derive from one master key, not KMS-backed.

## Evidence

- Decision "Personal integration credentials encrypted under a genuinely different key".
- `SecretsProvider` in `agentops-core/crates/agentops-repo-access/src/secrets.rs`.

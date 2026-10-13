# 0021. Make ownership a column, not a role

Survey candidate S2. Rationale is taken only from the sources under "Evidence"; nothing here is inferred.

## Status

Accepted, 2026-08-17 (`1ec773a`).

## Context

An organization needs exactly one owner, and ownership can be transferred.

## Decision

Store `organizations.owner_user_id`. The owner's membership role stays `admin`. Owner-only actions check the column.

## Alternatives considered

- An `owner` role on membership rows. Rejected: a transfer would touch two rows, and a crash between them could leave zero or two owners.

## Consequences

- Transfer is one atomic `UPDATE`.
- The owner has every admin capability and no extra matrix row.

## Evidence

- Decision "Owner is a single column on organizations, not a distinct membership role".

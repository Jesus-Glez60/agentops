# 0035. Session resume without a PreCompact hook

Survey candidate M5 (follow-up). Rationale is taken only from the sources under "Evidence"; nothing here is inferred.

## Status

Accepted, 2026-09-24 (`6ce8c39`).

## Context

A competing tool snapshots session state in a `PreCompact` hook because its state is otherwise lost at compaction.

## Decision

Render the session guide from `SessionStart` only, gated on the payload's `source`. `session_events` already persists durably throughout the session.

## Alternatives considered

- Port the competitor's `PreCompact` plus `SessionStart` design. Rejected as unnecessary.

## Consequences

- One hook fewer. Re-derive this if `session_events` ever stops persisting incrementally.

## Evidence

- Decision "Session-resume guide doesn't need a PreCompact hook -- session_events already persists durably, unlike Context Mode's ephemeral-until-checkpoint design".
- Commit `6ce8c39`.

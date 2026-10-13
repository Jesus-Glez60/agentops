# 0009. Correlate sessions with a session_id argument

Survey candidate M5. Rationale is taken only from the sources under "Evidence"; nothing here is inferred.

## Status

Accepted, 2026-08-11 (`67cedae`).

## Context

Activity from one agent session spans many tool calls and transports.

## Decision

Tools accept an optional `session_id` and record events in a repo-scoped `session_events` table. `get_session` returns the correlated feed. There is no new server or protocol concept.

## Alternatives considered

- A dedicated session subsystem. Rejected: the protocol-native argument "works for any client already speaking MCP/REST".

## Consequences

- Clients choose ids; the server never issues one.
- Claude Code's transcript session id may differ from the id an agent passes, so usage joins are heuristic.
- The same events feed `get_session_guide`, task activity and `end_session`.

## Evidence

- Decision "Module 6 session correlation is protocol-native via session_id, not a new subsystem".

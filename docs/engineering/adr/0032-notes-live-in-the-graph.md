# 0032. Notes in remote-connected repos go to the server

Survey candidate T2. Rationale is taken only from the sources under "Evidence"; nothing here is inferred.

## Status

Accepted.

## Context

Agents repeatedly wrote notes as local Markdown files in repos connected to a server. Those notes never reached the dashboard or `list_gotchas`.

## Decision

If `.context/agentops-remote.json` exists, `add_note`/`ingest_notes` is the only correct write path (`docs/vault-protocol.md`). `.claude/hooks/block-local-notes.sh` denies writes under `.agentops/notes/` while the marker exists.

## Alternatives considered

- The prose rule alone. Rejected: it "has been silently ignored by agents/subagents repeatedly" (hook header).

## Consequences

- A hard technical gate for Claude Code. Other tools rely on the prose.
- Hosted note files do not persist on the server (`findings.md` F30).

## Evidence

- `docs/vault-protocol.md`.
- Header of `.claude/hooks/block-local-notes.sh`.

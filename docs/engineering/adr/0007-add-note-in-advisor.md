# 0007. Allow knowledge writes in Advisor mode

Survey candidate M2. Rationale is taken only from the sources under "Evidence"; nothing here is inferred.

## Status

Accepted.

## Context

Advisor mode exists to block destructive or costly actions, such as bulk rescans and paid LLM calls. An advisor should still get smarter across sessions.

## Decision

`add_note` and `ingest_notes` are Advisor-mode tools, unlike every other core write tool.

## Alternatives considered

- Gate them behind Full. Rejected: it "would defeat the point of an advisor that's supposed to keep getting smarter across sessions" (code comment).

## Consequences

- Any client in the default mode can record notes.
- `persist_blueprint_doc` is also Advisor with no stated reason (`questions.md` Q6).
- Hosted `/mcp` refuses these tools' `notes_path` argument (fixed in `17a0407`; `findings.md` F31).

## Evidence

- Comments on the `add_note` and `ingest_notes` `ToolSpec` entries in `agentops-core/crates/agentops-mcp/src/tools.rs`.

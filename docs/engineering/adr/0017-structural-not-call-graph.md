# 0017. Structural edges, not a call graph

Survey candidate R9. Rationale is taken only from the sources under "Evidence"; nothing here is inferred.

## Status

Accepted, 2026-08-12 (`b0bd997`).

## Context

The Knowledge Graph mockup implied a call graph. AgentOps had no call-graph analysis.

## Decision

Add `References` for same-file symbol references only, detected from the tree-sitter AST, with a word-boundary regex fallback for files tree-sitter cannot parse. The graph screen's tabs filter real relations (`mode_filter`). Cross-file relationships stay unmodeled.

## Alternatives considered

- Full-body word-boundary regex for every file. Rejected: a comment mentioning a name would create false edges.

## Consequences

- Isolated nodes for cross-file-only symbols are expected.
- An LSP cross-file pass exists in code but is unreachable (`findings.md` F25).

## Evidence

- Decision "Knowledge Graph screen's tabs map to real EdgeRelations -- DependsOn/References are structural, not a real call graph".
- Commit `b0bd997`.

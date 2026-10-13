# 0001. Abandon the paid heavy tier for open source

Survey candidate A2. Rationale is taken only from the sources under "Evidence"; nothing here is inferred.

## Status

Accepted. Superseded in code by ADR 0002.

## Context

AgentOps was first planned as SaaS with two suites: a paid "heavy" suite and a free "light" suite. The heavy crates were built as a separate Cargo workspace licensed LicenseRef-Commercial, with a Postgres-backed `GraphStore` (`0385f5e`, 2026-07-28).

## Decision

The author chose the open-source route instead. The heavy/light split no longer means anything; "heavy" survives only in crate and file names.

## Alternatives considered

- Keep selling the heavy suite as SaaS. Rejected by the author; no further reason recorded.

## Consequences

- Every crate is `MIT OR Apache-2.0`.
- Crate names such as `agentops-heavy-api` keep the prefix. The merge deliberately did not rename them, to keep that diff scoped (decision "Backend merge ...").
- Leftover naming is logged in `findings.md` F11.

## Evidence

- Author answer `questions.md` A-2 (2026-10-12).
- Commit `0385f5e` "Add heavy-tier commercial workspace with Postgres-backed GraphStore".
- Decision "Backend merge: 3 REST APIs + 3 stdio MCP servers collapsed into agentops-server + agentops-mcp-server".

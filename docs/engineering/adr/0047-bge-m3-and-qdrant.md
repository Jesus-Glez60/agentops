# 0047. BGE-M3 and Qdrant for the semantic index

Survey candidate R1. Rationale comes from the author's answer of 2026-10-14; no decision note exists.

## Status

Accepted. Rationale recorded 2026-10-14.

## Context

`agentops-heavy-embeddings` embeds symbols, notes and doc sections with BGE-M3 and stores them in Qdrant. It is enabled only when `AGENTOPS_QDRANT_URL` is set; the core graph uses its own local ONNX embeddings.

## Decision

BGE-M3, because it handles code and many languages with long inputs. Qdrant, as a ready vector database for what was then the paid tier (ADR 0001).

## Alternatives considered

- None recorded.

## Consequences

- Optional: the hosted server runs without `AGENTOPS_QDRANT_URL`, so indexing stage 7 is skipped there.
- Qdrant has no tenant concept, so these tools stay off hosted `/mcp`.

## Evidence

- Author answer, 2026-10-14.
- `agentops-core/crates/agentops-heavy-embeddings`, `agentops-core/crates/agentops-heavy-mcp`.

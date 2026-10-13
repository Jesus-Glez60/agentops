# 0014. Keep training out of the retrieval crate

Survey candidate R4. Rationale is taken only from the sources under "Evidence"; nothing here is inferred.

## Status

Accepted.

## Context

Hybrid search needed a trained projection at query time. The first draft added `agentops-embeddings-train`, and with it `candle`, as a dependency of `agentops-retrieval`. That would have put `candle` into the default build of almost every crate.

## Decision

`agentops-retrieval` defines `trait EmbeddingProjector { fn project(&self, embedding: &[f32]) -> Vec<f32>; }`. `agentops-mcp` supplies `LoadedProjector`. `agentops-retrieval` never constructs or loads a projection.

## Alternatives considered

- Direct dependency. Rejected for the dependency leak above.

## Consequences

- Training dependencies stay in two crates.
- The pattern applies generally: define the port in the low-level crate, implement it where the heavy dependency is already affordable.

## Evidence

- Decision "agentops-retrieval must never depend on agentops-embeddings-train directly -- EmbeddingProjector is a plain trait for exactly this reason".

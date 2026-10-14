# 0013. CLS-inspired retrieval and local consolidation

Survey candidate R3. Rationale is taken only from the sources under "Evidence"; nothing here is inferred.

## Status

Accepted, 2026-08-21 (`4d5ffff`). Amended by ADR 0042 (strict promotion, note-only recency, LoRA consolidation hidden).

## Context

The goal was retrieval grounded in the graph's own reinforced knowledge, not only a frontier model's training data. The design draws on Complementary Learning Systems theory, HippoRAG, GENESIS and CodaRAG (decision note).

## Decision

Part A, retrieval: Personalized PageRank graph expansion; Hebbian `References` edges; gist-then-detail retrieval over `DocSection`/`Covers`; recency ranking; pattern completion (`related_context`).

Part B, consolidation, has two trainers:

- an embedding projection head, trained from edge replay and promoted by a recall@5 gate (`end_session`);
- a LoRA adapter on Qwen2.5-Coder-0.5B, trained from curated notes and promoted by a cosine gate (`consolidate_model`).

## Alternatives considered

- `candle-lora` as the LoRA implementation. Impossible: it was never published (gotcha 53970).

## Consequences

- Recall@5 rose from 0.029 to 0.087 on this repo (decision note).
- Full-default LoRA runs take 45+ minutes on CPU (gotcha 53983).
- Three items were deferred, including any consumer of the LoRA adapter (`findings.md` F29; `questions.md` Q13).
- Whether ties should promote is open (`questions.md` Q7). Node recency reuses the 30-day `Affects` half-life (`questions.md` Q12).

## Evidence

- Decision "CLS-inspired cleanup/optimization/local-consolidation plan (Initiatives 0-6) fully shipped".
- Commit `4d5ffff`.
- `05-retrieval.md`, `06-notes-and-consolidation.md`.

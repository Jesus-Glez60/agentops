//! Adapter: BGE-M3 embeddings + Qdrant (SemanticIndex). Module 3 adds a CandidateSource port here.
//!
//! Renamed from `agentops-embeddings` to `agentops-heavy-embeddings` when
//! the open-core `agentops-embeddings` crate (fastembed BGE-small-en-v1.5 +
//! sqlite-vec/pgvector, shared by docbrain and codebrain's semantic search)
//! was created under `agentops-core` — same crate name, two entirely
//! different implementations, would have been a real source of confusion.
//! This crate stays heavy-tier: Qdrant + the larger BGE-M3 model, for a
//! future hybrid dense+sparse `CandidateSource` fusion (see
//! `~/Vaults/agentops-vnext/knowledge/module-3-multi-signal-retrieval.md`),
//! not a replacement for the open-core one.
//!
//! Scaffolded on feat/full-rework for the vnext rebuild -- see the plan at
//! ~/.claude/plans/i-m-thinking-that-now-modular-sparrow.md and the vault at
//! ~/Vaults/agentops-vnext/ for full context. Intentionally empty: Module 1
//! (day-one bug fixes & housekeeping) is the first real implementation pass.

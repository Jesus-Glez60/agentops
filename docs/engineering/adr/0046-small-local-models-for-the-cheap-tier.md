# 0046. Small local models only for the cheap LLM tier, never bundled

Survey candidate R6. Rationale is the decision note "Small local models (≤4GB): recommended for the cheap LLM tier only, never bundled" on the hosted graph (node 54244), readable since search hits carry node ids (`findings.md` F44).

## Status

Proposed, 2026-10-12. The note calls itself "advisory — not yet implemented".

## Context

Bundling a model of at most 4 GB (Qwen3-4B, Gemma-3-4B, Phi-4-mini class) was considered, with an option to switch to a self-hosted or hosted provider. Indexing itself is deterministic and embeddings already run locally, so a local model would cut cost and improve privacy, not maintenance.

## Decision

1. Add provider configuration per tier (for example `AGENTOPS_LLM_PROVIDER`, a base URL and a JSON mode). `Provider::OpenAiCompatible` already covers Ollama, LM Studio, llama.cpp, vLLM, OpenRouter and Groq; only `LlmConfig::from_env` and `from_env_cheap` hard-code Anthropic.
2. `agentops install` detects a running Ollama or LM Studio and offers to route the cheap tier only to it. It never downloads weights.
3. Output people read (module labels, task summaries, `explain_symbol`) stays on a strong model by default.

## Alternatives considered

- Bundling weights or a native inference runtime. Rejected in the note: 2–4 GB more per `npx` install, a return of the native-build CI problems (ort-sys on Intel Macs, the Windows CRT), and slow shared CPU inference on the hosted server.

## Consequences

- Gate: ship the local cheap tier only if 2–3 small models score within a few points of Haiku on the librarian dry run's 135 hand labels.

## Evidence

- Decision note node 54244 on the hosted graph.
- `Provider::OpenAiCompatible`, `LlmConfig::from_env`, `from_env_cheap` (`agentops-core/crates/agentops-llm/src/lib.rs`).

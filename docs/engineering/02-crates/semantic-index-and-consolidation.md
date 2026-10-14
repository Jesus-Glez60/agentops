# Crates: semantic index and consolidation

The Qdrant-backed semantic index with cross-encoder reranking, and local LoRA consolidation of code explanations. These are the `agentops-heavy-*` crates; "heavy" is a leftover name from an abandoned paid tier (`01-architecture.md`, "The heavy prefix").

Index and field definitions: [02-crate-reference.md](../02-crate-reference.md).

## agentops-heavy-embeddings

- **Path**: `agentops-core/crates/agentops-heavy-embeddings`
- **Purpose**: Semantic search over the code graph and docbrain content. BGE-M3 dense embeddings run locally through ONNX (`fastembed`), are indexed in Qdrant, and pass a second-stage `bge-reranker-v2-m3` cross-encoder.
- **Entry points**:
  - `SemanticIndex`: `connect`, `ensure_collection`, `index`, `index_items`, `search`, `search_scoped`, `vector_search`, `vector_search_scoped`.
  - `collect_index_items` (graph nodes), `collect_doc_index_items` (docbrain nodes), `IndexItem`, `SearchHit`, `repo_name`.
- **Configuration**: `AGENTOPS_QDRANT_URL` (required to enable), `AGENTOPS_QDRANT_COLLECTION` (default `agentops_semantic`, in both `agentops-mcp-server` and `agentops-heavy-api`).
- **Depends on**: `graph`, `docbrain-graph`.
- **Used by**: `heavy-api`, `heavy-mcp`, `mcp-server`.
- **Invariants** (module doc):
  - Two stages on purpose: the bi-encoder pulls a wide, cheap candidate pool; the slower, more accurate cross-encoder runs only on that shortlist.
  - Code never leaves the process to be embedded; no external embedding API.
  - Qdrant has no tenant concept. It filters by `repo`/`kind` only, so the vector index does not enforce tenant isolation (`agentops-heavy-api` module doc). This is why hosted `/mcp` does not expose these tools.
- **History**: ported from the pre-merge crate also named `agentops-embeddings`, then renamed to avoid colliding with the core crate (module doc). Gotchas 53951 and 53956 about that rename are stale (`findings.md`).
- **Known history**: gotchas 54006 (raw-path vs `repo_name` mismatch found zero nodes), 54058 (disable `qdrant-client` default features to avoid `reqwest` via snapshots).
- **Tests**: 7 (`src/lib.rs`). They need a live Qdrant via `AGENTOPS_TEST_QDRANT_URL`.

## agentops-heavy-consolidate

- **Path**: `agentops-core/crates/agentops-heavy-consolidate`
- **Purpose**: Local consolidation of code explanations into a small causal LM. A frozen base model gets a trainable LoRA adapter trained from curated notes, and is promoted only through an eval gate. It mirrors `agentops-embeddings-train`'s shape.
- **Entry points**: `consolidate_model(store, repo) -> ModelConsolidationReport`, `generate_examples`, `train`, `evaluate`, `split_held_out`, `fetch_model_files`, `AdaptedModel`, `LoraAdapter`, `LoraLayer`, `AdapterStore`.
- **Constants**: `MODEL_ID = "Qwen/Qwen2.5-Coder-0.5B-Instruct"`, `LORA_RANK = 16`, `LORA_ALPHA = 32.0`, `MIN_EXAMPLES = 5`, `MAX_EXAMPLES = 30`, `HELD_OUT_FRACTION = 0.2`, `DEFAULT_MAX_NEW_TOKENS = 64`.
- **Depends on**: `embeddings`, `graph`, `retrieval`.
- **Used by**: `heavy-api` (`POST /consolidate/model`), `heavy-mcp` (`consolidate_model` tool).
- **Invariants**:
  - The base model download is explicit and one-time, never automatic. It follows the same "opt-in, real cost, never on a scan" rule as `explain_symbol` (`src/download.rs`).
  - LoRA is hand-rolled because `candle-lora` was never published to crates.io. `B` is zero-initialized so a new adapter starts as an exact no-op (`src/lora.rs`; gotcha 53970).
  - Eval: mean cosine similarity between a greedy generation and the curated note, under the frozen base embedder. That way the eval cannot reward drift toward the trained projection. Promoted iff `candidate_score >= baseline_score` (`src/eval.rs`, `src/lib.rs`). The module doc states the gate "reduces regression risk, it does not guarantee quality".
  - Adapters are stored under `~/.agentops/models/{repo}` with an `active_model_version` file. That path ignores `AGENTOPS_DATA_DIR` (`findings.md` F16).
- **Known history**: gotcha 53983, "Initiative 6's consolidate_model at full MAX_EXAMPLES/epoch/eval-token settings takes 45+ minutes on CPU".
- **Tests**: 17 across 8 files; 5 are `#[ignore]` (model download, training).

> Unverified: `src/download.rs` mentions an `agentops consolidate --model` command. No such CLI subcommand exists; see `findings.md` F22.

## agentops-heavy-mcp

- **Path**: `agentops-core/crates/agentops-heavy-mcp`
- **Purpose**: The tool table for the semantic index and model consolidation. `agentops-mcp-server` merges it in when Qdrant is configured.
- **Entry points**: `list_tools()`, `call_tool(index, name, args)` (async), `run_stdio`, `handle_message`, plus its own JSON-RPC types.
- **Tools**: `semantic_search`, `semantic_index`, `search_docs_indexed`, `index_docs_indexed`, each with MCP annotations. `consolidate_model` is no longer listed (ADR 0042). Full inventory in `survey/mcp-inventory.md`.
- **Depends on**: `graph`, `heavy-consolidate`, `heavy-embeddings`, `store-open`, `docbrain-graph`.
- **Used by**: `heavy-api`, `mcp-server`.
- **Invariants**:
  - No access mode. The module doc reasons that every tool is "read/index-only (no code modification)".
  - Not exposed on hosted `/mcp`, because the index is not tenant-safe (`agentops-heavy-api/src/mcp_http.rs` module doc).
  - Its `search_docs_indexed`/`index_docs_indexed` were renamed from `search_docs`/`index_docs` to avoid clashing with docbrain's tools. The `agentops-mcp-server` module doc notes they "may turn out to be redundant".
- **Tests**: 6 (`src/server.rs`). Some need `AGENTOPS_TEST_QDRANT_URL`.

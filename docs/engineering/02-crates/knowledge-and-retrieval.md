# Crates: knowledge and retrieval

Search fusion, note ingestion and symbol matching, LLM features, and embedding consolidation. Read this before changing how results are ranked or how notes attach to code; behavior is explained in [05-retrieval.md](../05-retrieval.md) and [06-notes-and-consolidation.md](../06-notes-and-consolidation.md).

Index and field definitions: [02-crate-reference.md](../02-crate-reference.md).

## agentops-retrieval

- **Path**: `agentops-core/crates/agentops-retrieval`
- **Purpose**: Multi-signal retrieval. Fuses dense, lexical (BM25/FTS) and exact-name candidate lists into one ranking, with optional graph expansion.
- **Entry points**:
  - `search_hybrid(store, embedder, repo, query, top_k, kind, graph_expand, projector) -> Vec<HybridHit>`.
  - `search_gist_then_detail(store, embedder, repo, query, top_k)`: match doc sections first, then search only what they cover.
  - `personalized_pagerank(neighborhood, seeds, damping, iterations)`, `shortest_path(...) -> Vec<GraphPathHop>`.
  - `pattern_complete(store, embedder, repo, seed_id, k) -> Vec<PatternCompletionMatch>`: the engine behind `related_context`.
  - Ports: `CandidateSource` (`DenseCandidateSource`, `LexicalCandidateSource`, `ExactCandidateSource`), `EmbeddingProjector`.
- **Depends on**: `embeddings`, `graph`.
- **Used by**: `cli`, `heavy-consolidate`, `llm`, `mcp`.
- **Invariants**:
  - Fusion is reciprocal rank fusion with `RRF_K = 60` (`src/lib.rs`).
  - Lexical search is database-native: FTS5 on SQLite, a generated `tsvector` on Postgres. A separate tantivy index was rejected (module doc).
  - Core has no cross-encoder rerank. Fusion is the final ranking; reranking exists only in `agentops-heavy-embeddings` (module doc).
  - Never depends on `agentops-embeddings-train`. A trained projection arrives only as `&dyn EmbeddingProjector` (decision "agentops-retrieval must never depend on agentops-embeddings-train directly -- EmbeddingProjector is a plain trait for exactly this reason").
- **Decision**: "Phase 4 multi-signal retrieval redesigned against current architecture, live-verified".
- **Tests**: 25 (`src/lib.rs`).

## agentops-notes

- **Path**: `agentops-core/crates/agentops-notes`
- **Purpose**: Markdown note ingestion from a vault or a loose folder, plus the ports for matching notes to symbols and classifying them.
- **Entry points**:
  - `ingest_vault`, `walk_vault`, `VaultNote`, `IngestSummary` (`src/vault.rs`).
  - Matching: `SymbolMatcher` (port), `WordBoundaryMatcher`, `match_symbols`, `match_same_file_references`, `connect_many`, `ConnectSummary`.
  - Classification: `NoteClassifier` (port), `HeuristicClassifier`, `heuristic_score`, `NoteType`.
  - Notes path: `default_notes_path`, `resolve_notes_path`, `write_notes_path` (`src/config.rs`).
- **Depends on**: `graph`.
- **Used by**: `cli`, `llm`, `mcp`.
- **Invariants**:
  - No network dependency. LLM-assisted adapters for both ports live in `agentops-llm` (that crate's module doc).
  - Re-ingesting a note updates it in place (natural key), never duplicates it.
  - `bump_confirmed_at`: the automatic every-scan re-match passes `false`; explicit `add_note`/import passes `true` (doc comment on `ingest_vault`; gotcha 53965).
  - Vault notes use the pseudo-path `vault:{source_path}`.
- **Known history**: gotcha 54035, "notes written before first scan never attach to symbols".
- **Tests**: 23 (`src/vault.rs` 16, `src/config.rs` 7).

## agentops-llm

- **Path**: `agentops-core/crates/agentops-llm`
- **Purpose**: On-demand LLM features: symbol explanations, LLM-assisted note matching and classification, task summaries, module grouping for docs, the librarian classifier, and the LLM spend ledger.
- **Entry points**:
  - `explain_symbol(store, config, repo, symbol_id) -> i64`: stores a `Definition` node linked by `Documents`, upserted on re-run.
  - `call_anthropic`, `call_json` (schema-constrained output), `LlmConfig`, `Provider` (`Anthropic` or `OpenAiCompatible { name, json_mode }`), `JsonMode`.
  - `LlmAssistedMatcher`, `LlmAssistedClassifier`: adapters for `agentops-notes`' ports.
  - `summarize_task_activity`, `group_core_modules`.
  - `librarian` module: `classify_for_library`, `LibrarianVerdict`, `OPERATION = "librarian_classify"`.
  - `pricing` (`rates_for`, `cost_estimate_usd`, `ModelRates`), `usage` (`UsageRecorder`, `LlmUsageSink`), `tokens` (`count_tokens`, `truncate_to_tokens`).
- **Depends on**: `embeddings`, `graph`, `notes`, `retrieval`, `scanner`.
- **Used by**: `cli`, `heavy-api`, `mcp`.
- **Invariants**:
  - `explain_symbol` is opt-in per symbol. The one scan-time call is `group_core_modules` (module labels), skipped while the file set is unchanged (ADR 0044).
  - Every call through `send_messages` (behind `call_anthropic` and `call_json`) reports to the spend ledger (`llm_usage` table), failures included, **when** `LlmConfig.usage_sink` is set. With no sink, nothing is recorded.
  - 401 and 429 responses map to distinct, labelled errors (doc comment on `call_anthropic`).
  - Defaults: `DEFAULT_MODEL = "claude-sonnet-5-5"`, `DEFAULT_MODEL_CHEAP = "claude-haiku-4-5-20251001"`. Overridden by `AGENTOPS_ANTHROPIC_MODEL` and `AGENTOPS_ANTHROPIC_MODEL_CHEAP`.
- **Known history**: gotchas 54234 (stale cost rates overstate Opus 5.5 about 3.75x), 54235 (OpenAI-compatible providers differ in structured-output shape), 53924 (`tiktoken-rs` API).
- **Decision**: "Small local models (≤4GB): recommended for the cheap LLM tier only, never bundled".
- **Tests**: 47 (`src/lib.rs` 37, `src/pricing.rs` 4, `src/librarian.rs` 3, `src/tokens.rs` 3).

## agentops-embeddings-train

- **Path**: `agentops-core/crates/agentops-embeddings-train`
- **Purpose**: Embedding consolidation. Trains a small residual projection head on top of the frozen `LocalEmbedder`, using positive pairs from the graph's `Affects`/`References` edges.
- **Entry points**: `consolidate` (`ConsolidationReport`), `train`, `TrainConfig`, `ProjectionHead`, `collect_positive_pairs`, `merge_with_anchor`, `recall_at_k`, `split_held_out`, `ProjectionStore` (`promote`, `active_version`, `load_active`).
- **Constants**: `HIDDEN_DIM = 128`, `RECALL_K = 5`, `HELD_OUT_FRACTION = 0.2`, `MIN_REPLAY_PAIRS = 10`, `MAX_REPLAY_PAIRS = 500`, `ANCHOR_SIZE = 200`.
- **Depends on**: `embeddings`, `graph`.
- **Used by**: `mcp` (`src/consolidate.rs`).
- **Invariants**:
  - The only crate besides `agentops-heavy-consolidate` that pulls in `candle-core`/`candle-nn`. Training must never leak into the inference path (module doc).
  - No-regression gate: a candidate is promoted only if `candidate_recall >= baseline_recall` on the held-out split (`src/lib.rs`). With too few pairs it skips rather than errors.
  - An anchor set from earlier runs is mixed in against overfitting to the latest session.
  - Heads are stored as `proj_head_v{N}.safetensors` plus `active_version` under `~/.agentops/models/{repo}`. That path ignores `AGENTOPS_DATA_DIR` (`findings.md` F16).
- **Known history**: gotcha 53969, "ProjectionHead's second linear layer must be zero-initialized".
- **Tests**: 20 (`src/replay.rs` 6, `src/store.rs` 5, `src/eval.rs` 3, `src/lib.rs` 2, `src/train.rs` 2, `src/model.rs` 2).

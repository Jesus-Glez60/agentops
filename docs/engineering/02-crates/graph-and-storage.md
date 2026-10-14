# Crates: code graph and storage

The graph's entities, the `GraphStore` port, its SQLite and Postgres adapters, and the factory that picks one. Read this before adding a node or edge type, a store method, or a backend; schema detail is in [03-data-model.md](../03-data-model.md).

Index and field definitions: [02-crate-reference.md](../02-crate-reference.md).

## agentops-graph

- **Path**: `agentops-core/crates/agentops-graph`
- **Purpose**: Domain entities (`Node`, `Edge`, `NodeKind`, `EdgeRelation`), the `GraphStore` port, and its SQLite adapter `SqliteGraphStore` (`src/sqlite.rs`).
- **Entry points**:
  - `GraphStore` trait (`src/lib.rs:746`), 57 methods in these groups:
    - nodes: `add_node`, `get_node`, `find_node`, `update_node`, `set_curation`, `delete_nodes`, `nodes_by_kind`, `nodes_pinned`, `all_nodes`;
    - edges: `add_edge`, `edges_from`, `edges_to`, `reinforce_edge`, `delete_edge`, `delete_edges_from`;
    - search: `set_embedding`, `search_similar`, `search_lexical`, `search_exact`;
    - history: `record_scan`, `latest_scan`, `list_scans`, `scan_entries`, `snapshot_node_version`, `close_node_version`, `node_history`, `node_as_of`;
    - repo state and docs: `refresh_repo_state`, `get_repo_state`, `save_doc_page`, `get_doc_page`;
    - sessions and usage: `record_session_event`, `session_events*`, `upsert_session_usage`, `upsert_session_report`, `record_llm_usage`;
    - tasks: `create_task`, `list_tasks`, `update_task_status`, `upsert_external_task`, `link_task`;
    - batch variants: `upsert_nodes_batch`, `add_edges_batch`, `set_embeddings_batch`, `reinforce_edges_batch` and others.
  - Free functions: `upsert_node`, `prune_stale_nodes`, `bounded_neighborhood`, `effective_edge_weight`, `prominence_rank_multiplier`, `rank_notes_by_weight`, `note_score`.
  - Constants: `AFFECTS_EDGE_HALF_LIFE_DAYS = 30.0`, `REFERENCES_EDGE_HALF_LIFE_DAYS = 90.0`.
  - `community` module: `detect_hotspots`, `node_degrees`, `Hotspot`. Hand-rolled modularity community detection, because `petgraph` ships none (module doc).
- **Depends on**: `embeddings`, `sqlite-support`.
- **Used by**: 15 crates. It is the most depended-on crate in the workspace.
- **Invariants**:
  - Every `GraphStore` method takes `repo`; no global, id-only lookups (module doc).
  - `Affects` edges point from the note to the symbol. Reversing them silently breaks `agentops-docgen`'s gotcha badges (doc comment on `EdgeRelation::Affects`).
  - `Gotcha`/`Decision` nodes carry the title in `name` and the body in `content`. `agentops-docgen` depends on that implicit contract.
  - Ranking multipliers such as `prominence_rank_multiplier` apply to a sort key only, never to a displayed score (its doc comment).
  - `MIGRATIONS_SLICE` is append-only (`src/sqlite.rs`).
  - A trait default method cannot call a free function taking `&dyn GraphStore` (gotcha 54103).
- **Known history**: gotchas 54078/54079/53901 (natural-key collisions), 54057 (pruning nodes with live edges), 53952 (older `graph.db` missing the `repo` column on `edges`), 53965 (auto-rematch must not bump `updated_at`), 54092 (sqlite-vec `k <= 4096`).
- **Tests**: 76 (`src/sqlite.rs` 55, `src/lib.rs` 17, `src/community.rs` 4).

## agentops-graph-pg

- **Path**: `agentops-core/crates/agentops-graph-pg`
- **Purpose**: `PostgresGraphStore`, a `GraphStore` adapter for a shared multi-repo Postgres database with pgvector.
- **Entry points**: `PostgresGraphStore` (`connect`, the `GraphStore` impl, `wipe_repo`). Schema in `schema.sql`, embedded with `include_str!`.
- **Depends on**: `embeddings`, `graph`.
- **Used by**: `cli` (`migrate-graph`), `mcp`, `store-open`.
- **Invariants** (module doc unless noted):
  - Sync bridge: the store owns a Tokio runtime and blocks on it per call. Async callers must use `spawn_blocking` and must never call it from inside a running runtime (gotchas 54028, 54054).
  - Cheaply `Clone`. Long-lived processes connect once and clone per request. Connecting per request broke at 54 concurrent requests (32 failed).
  - Pool size comes from `AGENTOPS_PG_POOL_MAX_SIZE` (default 25), sized from the Postgres server's core count, not the app container's.
  - `schema.sql` is all `IF NOT EXISTS` and is re-applied on connect. CHECK constraints on `nodes.kind` and `edges.relation` must be widened for every new variant (gotcha 54053).
  - Every per-repo table must be cleared in `wipe_repo` (gotcha 54241).
  - TIMESTAMPTZ columns need explicit `::text` casts to share Rust types with SQLite's TEXT (gotchas 54055, 53953, 54237).
- **Tests**: 27 (`src/lib.rs`). They need a live Postgres via `AGENTOPS_TEST_DATABASE_URL` and skip silently otherwise (gotcha 54247).

## agentops-store-open

- **Path**: `agentops-core/crates/agentops-store-open`
- **Purpose**: The single factory every driving adapter uses to open a `GraphStore`. It was moved out of `agentops-mcp` so `agentops-heavy-mcp` can use it without depending on `agentops-mcp`.
- **Entry points**:
  - `open_store(repo_path)`: a shared Postgres store if one is scoped, else Postgres when `AGENTOPS_DATABASE_URL` is set, else SQLite at `graph_db_path(repo_path)`.
  - `graph_db_path(repo_path)` = `<repo>/.context/graph.db`.
  - `repo_name(path)`: the canonical directory name, used as the repo key.
  - `open_shared_postgres_store()`: connects once, retrying with jittered exponential backoff (cap 5 s) for `AGENTOPS_PG_CONNECT_RETRY_SECS` (default 60). Added in `cf408a4`.
  - `resolve_store(shared, path)`, `with_shared_postgres_store(store, f)`: reuse one pool through a thread-local override, so handlers need no extra parameter.
  - `describe_backend(path)`: a human-readable backend name with credentials redacted.
- **Depends on**: `graph`, `graph-pg`.
- **Used by**: `heavy-mcp`, `mcp`. `agentops-mcp` re-exports it as `agentops_mcp::store`.
- **Invariants**:
  - Request handling should open stores only through this crate. Hosted indexing stages 7-8 did not, until the F27 fix.
  - A connection string is never printed in full (`redact_credentials`).
  - `AGENTOPS_PG_CONNECT_RETRY_SECS=0` (or invalid) means "try once", not "use the default" (doc comment on `retry_budget`; gotcha 54156).
- **Tests**: 10 (`src/lib.rs`, 1 `#[ignore]`). Some need `AGENTOPS_TEST_DATABASE_URL`.

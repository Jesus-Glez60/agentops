-- Postgres schema for `PostgresGraphStore` — a structural mirror of
-- `SqliteGraphStore`'s schema (agentops-core/crates/agentops-graph/src/sqlite.rs),
-- not a port of `main`'s stale agentops-heavy schema (which predates
-- `container`, repo-scoped edges, scan_history/scan_history_entries, and
-- the `note`/`definition` node kinds).
--
-- Every statement uses IF NOT EXISTS, matching the same reasoning as
-- docbrain's heavy-tier schema init: safe to re-run manually, not just via
-- a container's first-boot init.d hook.

CREATE EXTENSION IF NOT EXISTS vector;

CREATE TABLE IF NOT EXISTS nodes (
    id          BIGSERIAL PRIMARY KEY,
    kind        TEXT NOT NULL CHECK (kind IN ('symbol', 'file', 'gotcha', 'decision', 'definition', 'note')),
    repo        TEXT NOT NULL,
    path        TEXT,
    name        TEXT,
    container   TEXT,
    start_line  BIGINT,
    end_line    BIGINT,
    content     TEXT,
    -- BGE-small-en-v1.5, 384 dims (agentops_embeddings::EMBEDDING_DIM) —
    -- inline on the row itself, unlike SqliteGraphStore's separate `vec0`
    -- virtual table (sqlite-vec can't add a vector column to a normal
    -- table the way pgvector can), so pruning a node here can never leave
    -- an orphaned embedding row behind the way SqliteGraphStore's
    -- `delete_nodes` has to explicitly guard against.
    embedding   vector(384)
);
CREATE INDEX IF NOT EXISTS idx_nodes_repo_kind ON nodes(repo, kind);
CREATE INDEX IF NOT EXISTS idx_nodes_repo_path ON nodes(repo, path);
-- Cosine distance, matching sqlite-vec's KNN for the same embedding model.
CREATE INDEX IF NOT EXISTS idx_nodes_embedding ON nodes USING ivfflat (embedding vector_cosine_ops) WITH (lists = 100);

CREATE TABLE IF NOT EXISTS edges (
    id        BIGSERIAL PRIMARY KEY,
    repo      TEXT NOT NULL,
    src_id    BIGINT NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    dst_id    BIGINT NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    relation  TEXT NOT NULL CHECK (relation IN ('depends_on', 'documents', 'affects'))
);
CREATE INDEX IF NOT EXISTS idx_edges_repo_src ON edges(repo, src_id);
CREATE INDEX IF NOT EXISTS idx_edges_repo_dst ON edges(repo, dst_id);

-- Weighted, decaying `Affects` edges (see `agentops_graph::effective_weight`):
-- `IF NOT EXISTS` on `ADD COLUMN` is real Postgres syntax (unlike SQLite,
-- which has no equivalent — see `agentops-graph/src/sqlite.rs`'s
-- `rusqlite_migration`-based approach instead), so a plain `ALTER TABLE`
-- here is enough to reach an already-populated `edges` table too, not just
-- a fresh one. `TIMESTAMPTZ`, matching `scan_history.started_at`'s existing
-- convention, not `TEXT` — the Rust side reads it back cast to text (see
-- `EDGES_COLUMNS` in `src/lib.rs`), the same pattern `SCAN_HISTORY_COLUMNS`
-- already established for `started_at`.
ALTER TABLE edges ADD COLUMN IF NOT EXISTS weight DOUBLE PRECISION NOT NULL DEFAULT 1.0;
ALTER TABLE edges ADD COLUMN IF NOT EXISTS updated_at TIMESTAMPTZ NOT NULL DEFAULT now();

CREATE TABLE IF NOT EXISTS scan_history (
    id               BIGSERIAL PRIMARY KEY,
    repo             TEXT NOT NULL,
    started_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    files_added      BIGINT NOT NULL DEFAULT 0,
    files_changed    BIGINT NOT NULL DEFAULT 0,
    files_removed    BIGINT NOT NULL DEFAULT 0,
    symbols_added    BIGINT NOT NULL DEFAULT 0,
    symbols_changed  BIGINT NOT NULL DEFAULT 0,
    symbols_removed  BIGINT NOT NULL DEFAULT 0,
    notes_added      BIGINT NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_scan_history_repo ON scan_history(repo, started_at);

-- node_id is deliberately not a hard FK to nodes(id): a Removed entry's
-- node has already been deleted by the time this table is read back.
CREATE TABLE IF NOT EXISTS scan_history_entries (
    id       BIGSERIAL PRIMARY KEY,
    scan_id  BIGINT NOT NULL REFERENCES scan_history(id),
    node_id  BIGINT NOT NULL,
    kind     TEXT NOT NULL,
    path     TEXT,
    name     TEXT,
    change   TEXT NOT NULL CHECK (change IN ('added', 'changed', 'removed'))
);
CREATE INDEX IF NOT EXISTS idx_scan_history_entries_scan ON scan_history_entries(scan_id);

-- Repo state (state memory): a single upserted-in-place snapshot row per
-- repo, deliberately not a history table (that's node_versions'/bi-temporal
-- versioning's job for node content) — "what does this repo's graph
-- currently think matters most."
CREATE TABLE IF NOT EXISTS repo_state (
    repo             TEXT PRIMARY KEY,
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_scan_id     BIGINT,
    top_gotcha_ids   TEXT NOT NULL,
    top_decision_ids TEXT NOT NULL
);

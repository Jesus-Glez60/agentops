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

-- Documentation Viewer: a single upserted-in-place generated-doc-page
-- snapshot per repo, same shape as repo_state above. `content` is an
-- already-serialized `agentops_docgen::DocPage` JSON blob, written and read
-- as opaque text (see agentops-graph's GraphStore::save_doc_page doc
-- comment for why this crate never depends on that Rust type).
CREATE TABLE IF NOT EXISTS doc_pages (
    repo         TEXT PRIMARY KEY,
    generated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    content      TEXT NOT NULL
);

-- Bi-temporal node versioning (Phase 2, 1.0 roadmap). node_id is
-- deliberately not a hard FK — history must survive the node's own
-- eventual pruning, same reasoning as scan_history_entries. TIMESTAMPTZ
-- matching edges.updated_at's existing convention; read back cast to text
-- via NODE_VERSIONS_COLUMNS in src/lib.rs, same pattern as EDGES_COLUMNS.
CREATE TABLE IF NOT EXISTS node_versions (
    id          BIGSERIAL PRIMARY KEY,
    node_id     BIGINT NOT NULL,
    content     TEXT,
    start_line  BIGINT,
    end_line    BIGINT,
    valid_from  TIMESTAMPTZ NOT NULL DEFAULT now(),
    valid_until TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS idx_node_versions_node ON node_versions(node_id);

-- Cross-tool session correlation (Phase 3, 1.0 roadmap, Module 6). Not
-- scan-scoped — one row per notable write action, tagged with the
-- caller-supplied session_id.
CREATE TABLE IF NOT EXISTS session_events (
    id          BIGSERIAL PRIMARY KEY,
    repo        TEXT NOT NULL,
    session_id  TEXT NOT NULL,
    tool_name   TEXT NOT NULL,
    description TEXT NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_session_events_repo_session ON session_events(repo, session_id);

-- Hybrid native task manager + Linear sync (Phase 3, 1.0 roadmap, Module
-- 7). external_source/external_id are both NULL for a native task; the
-- partial unique index only applies once external_source is set.
CREATE TABLE IF NOT EXISTS tasks (
    id              BIGSERIAL PRIMARY KEY,
    repo            TEXT NOT NULL,
    title           TEXT NOT NULL,
    description     TEXT,
    status          TEXT NOT NULL,
    priority        TEXT,
    assignee        TEXT,
    external_source TEXT,
    external_id     TEXT,
    session_id      TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_tasks_external ON tasks(external_source, external_id) WHERE external_source IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_tasks_repo ON tasks(repo);

CREATE TABLE IF NOT EXISTS task_links (
    task_id  BIGINT NOT NULL,
    node_id  BIGINT NOT NULL,
    relation TEXT NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_task_links_unique ON task_links(task_id, node_id, relation);

-- Lexical (BM25-equivalent) search signal (Phase 4, 1.0 roadmap) — the
-- Postgres mirror of SqliteGraphStore's FTS5 approach, same "database keeps
-- it in sync, not a Rust call site" reasoning: a generated `tsvector`
-- column plus a GIN index, so `ts_rank`-based lexical search stays current
-- automatically on every INSERT/UPDATE, with no separate write path (and
-- unlike SQLite's FTS5, Postgres's STORED GENERATED column handles the
-- pre-existing-row backfill for free — no separate backfill statement, no
-- risk of the self-referential-read corruption confirmed against SQLite's
-- FTS5 backfill, since a generated column is computed per-row by Postgres
-- itself, not populated by a second statement reading the same table).
ALTER TABLE nodes ADD COLUMN IF NOT EXISTS search_vector tsvector
    GENERATED ALWAYS AS (to_tsvector('english', coalesce(name, '') || ' ' || coalesce(content, ''))) STORED;
CREATE INDEX IF NOT EXISTS idx_nodes_search_vector ON nodes USING gin(search_vector);

-- Gotcha review workflow: triage state for a node (meaningful for Gotcha
-- kind, harmless/unused on others) -- same TEXT-column-with-a-constant
-- default shape as the SQLite migration for this same field.
-- Superseded within the same session by the curation columns below -- the
-- earlier `review_status` design modeled gotchas as bugs to close, which
-- is wrong (see the curation columns' own comment in agentops-graph).
ALTER TABLE nodes DROP COLUMN IF EXISTS review_status;
ALTER TABLE nodes ADD COLUMN IF NOT EXISTS curated BOOLEAN NOT NULL DEFAULT false;
ALTER TABLE nodes ADD COLUMN IF NOT EXISTS prominence TEXT NOT NULL DEFAULT 'full';
ALTER TABLE nodes ADD COLUMN IF NOT EXISTS curation_reason TEXT;

-- Initiative 2 (CLS-inspired retrieval plan) adds NodeKind::DocSection and
-- EdgeRelation::Covers -- both CHECK constraints need widening.
-- `nodes_kind_check`/`edges_relation_check` are Postgres's default
-- auto-generated names for the unnamed inline CHECKs in the original
-- `CREATE TABLE` statements above (`<table>_<column>_check`).
--
-- `edges_relation_check` had an earlier intermediate DROP/re-ADD here too
-- (widening 3 values -> 4, for `references`) -- **removed**, not just
-- superseded, because it's actively dangerous to leave in a file that gets
-- fully replayed via `batch_execute(SCHEMA)` on every single
-- `PostgresGraphStore::connect()` call, not just once at first deploy. As
-- soon as real `covers`-relation data exists (any repo with a
-- `NodeKind::DocSection`), replaying that intermediate 4-value `ADD
-- CONSTRAINT` re-validates every existing row against a list that no
-- longer includes a value real rows now have, and fails outright --
-- aborting the whole batch before ever reaching this final, correct
-- statement. Caught live: every `/mcp` tool call against a populated
-- Postgres-backed repo failed with "check constraint \"edges_relation_check\"
-- ... is violated by some row", opening a fresh `PostgresGraphStore`
-- (and therefore replaying this whole file) on every single call. The
-- general lesson, not just this one constraint: a schema file replayed on
-- every connection must only ever contain the *current target* shape for
-- anything that isn't naturally idempotent against real data (unlike
-- `ADD COLUMN IF NOT EXISTS`/`CREATE TABLE IF NOT EXISTS`, a `DROP
-- CONSTRAINT` + narrower re-`ADD` pair is not safe to keep around once
-- superseded) -- never a full literal migration history.
ALTER TABLE nodes DROP CONSTRAINT IF EXISTS nodes_kind_check;
ALTER TABLE nodes ADD CONSTRAINT nodes_kind_check CHECK (kind IN ('symbol', 'file', 'gotcha', 'decision', 'definition', 'note', 'doc_section'));

ALTER TABLE edges DROP CONSTRAINT IF EXISTS edges_relation_check;
ALTER TABLE edges ADD CONSTRAINT edges_relation_check CHECK (relation IN ('depends_on', 'documents', 'affects', 'references', 'covers'));

-- Initiative 3 (CLS-inspired retrieval plan) added Node.last_touched_at on
-- the SQLite backend only, deliberately not here: `nodes` queries in
-- src/lib.rs are almost all `SELECT *` (unlike `edges`, which already has a
-- dedicated `EDGES_COLUMNS` constant with a `::text` cast for exactly this
-- reason) -- wiring this column in properly means replacing every one of
-- those `SELECT *` call sites with an explicit, `::text`-cast column list,
-- a wide change with no live Postgres instance reachable in the session
-- that added it to verify against. `Node.last_touched_at` is `Option<String>`
-- specifically so `PostgresGraphStore` can return `None` (recency ranking
-- becomes a no-op there, not a crash) until this is done properly.



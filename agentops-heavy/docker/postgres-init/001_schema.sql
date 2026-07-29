-- Heavy-tier neuron graph schema — mirrors agentops-graph's SqliteGraphStore
-- schema exactly (same node/edge shape, see the plan's "neuron model": the
-- light-tier SQLite store is the schema's origin, heavy tier is "the same
-- graph, replicated into scalable stores").
--
-- The official postgres image only runs *-init.d/ scripts once, against an
-- empty data directory (documented Docker Hub behavior, not run on every
-- restart) — but every statement here is still written with IF NOT EXISTS
-- so this file is also safe to re-run manually (e.g. `psql -f`) without
-- erroring on an already-migrated database.

CREATE TABLE IF NOT EXISTS nodes (
    id          BIGSERIAL PRIMARY KEY,
    kind        TEXT NOT NULL CHECK (kind IN ('symbol', 'file', 'gotcha', 'decision')),
    repo        TEXT NOT NULL,
    path        TEXT,
    name        TEXT,
    start_line  BIGINT,
    end_line    BIGINT,
    content     TEXT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_nodes_kind ON nodes(kind);
CREATE INDEX IF NOT EXISTS idx_nodes_repo_path ON nodes(repo, path);

CREATE TABLE IF NOT EXISTS edges (
    id          BIGSERIAL PRIMARY KEY,
    src_id      BIGINT NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    dst_id      BIGINT NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    relation    TEXT NOT NULL CHECK (relation IN ('depends_on', 'documents', 'affects')),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_edges_src ON edges(src_id);
CREATE INDEX IF NOT EXISTS idx_edges_dst ON edges(dst_id);

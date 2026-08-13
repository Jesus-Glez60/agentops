//! `SqliteGraphStore`: the SQLite adapter implementing the `GraphStore`
//! port. Built on `agentops-sqlite-support` for connection/migration
//! boilerplate (the exact reuse that shared crate was built for during the
//! docbrain pass). Node upsert is *not* done via that crate's generic
//! `upsert()` helper — `nodes`' natural key (`repo, kind, path, name,
//! container`) includes nullable columns, and SQLite's `UNIQUE` constraint
//! treats every `NULL` as distinct (so `ON CONFLICT` would never fire for
//! two nodes that both happen to have a `NULL` path/name, e.g. two `File`
//! nodes) — so node upsert stays a find-then-branch (`find_node` uses `IS`,
//! not `=`, for NULL-safe comparison), matching `main`'s original approach.

use std::path::Path;
use std::sync::Once;

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension};
use rusqlite_migration::{Migrations, M};

use agentops_embeddings::EMBEDDING_DIM;

use crate::{Edge, EdgeRelation, GraphStore, Node, NodeKind, NodeProminence, NewNode, NewScanHistoryEntry, NewTask, NodeVersion, RepoState, ScanChange, ScanHistory, ScanHistoryEntry, SessionEvent, Task, TaskLink, TaskStatus};

static INIT_VEC_EXTENSION: Once = Once::new();

/// `sqlite-vec` is loaded via `sqlite3_auto_extension`, which registers it
/// for every connection opened in this process from that point on — must
/// run before any `Connection::open*` call, and only once per process.
/// Same technique `docbrain-graph` already uses for the identical purpose.
fn ensure_vec_extension_registered() {
    INIT_VEC_EXTENSION.call_once(|| unsafe {
        rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute::<
            *const (),
            unsafe extern "C" fn(*mut rusqlite::ffi::sqlite3, *mut *mut std::ffi::c_char, *const rusqlite::ffi::sqlite3_api_routines) -> std::ffi::c_int,
        >(sqlite_vec::sqlite3_vec_init as *const ())));
    });
}

fn embedding_to_bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

/// Turns a free-text query into an FTS5 `MATCH` expression that can't
/// throw a syntax error regardless of what punctuation the caller typed —
/// each whitespace-separated term is quoted as its own phrase (embedded
/// `"` doubled per FTS5's own escaping rule) and OR'd together, so this is
/// a broad "any term matches" search, not a strict AND/phrase query.
fn fts5_match_query(query: &str) -> String {
    query.split_whitespace().map(|term| format!("\"{}\"", term.replace('"', "\"\""))).collect::<Vec<_>>().join(" OR ")
}

fn vec_table_migration() -> String {
    format!("CREATE VIRTUAL TABLE IF NOT EXISTS node_vectors USING vec0(node_id INTEGER PRIMARY KEY, embedding FLOAT[{EMBEDDING_DIM}]);")
}

/// Ordered, tracked (via SQLite's `user_version` pragma) migration steps —
/// replaces a single idempotent `CREATE TABLE IF NOT EXISTS` blob re-run on
/// every open, which can never express "add a column to an existing,
/// already-populated table." A database that predates this (like this
/// repo's own `.context/graph.db`) starts at `user_version = 0`: `to_latest`
/// harmlessly re-runs the old `CREATE TABLE IF NOT EXISTS` steps as no-ops,
/// then applies the two genuinely new steps for the first time.
const MIGRATIONS_SLICE: &[M<'_>] = &[
    M::up(
        "CREATE TABLE IF NOT EXISTS nodes (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            kind        TEXT NOT NULL,
            repo        TEXT NOT NULL,
            path        TEXT,
            name        TEXT,
            container   TEXT,
            start_line  INTEGER,
            end_line    INTEGER,
            content     TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_nodes_repo_kind ON nodes(repo, kind);
        CREATE INDEX IF NOT EXISTS idx_nodes_repo_path ON nodes(repo, path);",
    ),
    M::up(
        "CREATE TABLE IF NOT EXISTS edges (
            id        INTEGER PRIMARY KEY AUTOINCREMENT,
            repo      TEXT NOT NULL,
            src_id    INTEGER NOT NULL REFERENCES nodes(id),
            dst_id    INTEGER NOT NULL REFERENCES nodes(id),
            relation  TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_edges_repo_src ON edges(repo, src_id);
        CREATE INDEX IF NOT EXISTS idx_edges_repo_dst ON edges(repo, dst_id);",
    ),
    M::up(
        "CREATE TABLE IF NOT EXISTS scan_history (
            id               INTEGER PRIMARY KEY AUTOINCREMENT,
            repo             TEXT NOT NULL,
            started_at       TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            files_added      INTEGER NOT NULL DEFAULT 0,
            files_changed    INTEGER NOT NULL DEFAULT 0,
            files_removed    INTEGER NOT NULL DEFAULT 0,
            symbols_added    INTEGER NOT NULL DEFAULT 0,
            symbols_changed  INTEGER NOT NULL DEFAULT 0,
            symbols_removed  INTEGER NOT NULL DEFAULT 0,
            notes_added      INTEGER NOT NULL DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS idx_scan_history_repo ON scan_history(repo, started_at);",
    ),
    M::up(
        // node_id is deliberately not a hard FK to nodes(id): a Removed
        // entry's node has already been deleted by the time this table is
        // read back.
        "CREATE TABLE IF NOT EXISTS scan_history_entries (
            id       INTEGER PRIMARY KEY AUTOINCREMENT,
            scan_id  INTEGER NOT NULL REFERENCES scan_history(id),
            node_id  INTEGER NOT NULL,
            kind     TEXT NOT NULL,
            path     TEXT,
            name     TEXT,
            change   TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_scan_history_entries_scan ON scan_history_entries(scan_id);",
    ),
    // Module A: weighted, decaying `Affects` edges. `weight`'s `DEFAULT 1.0`
    // is a real constant, allowed directly in `ALTER TABLE ADD COLUMN`.
    // `updated_at` can't use `DEFAULT CURRENT_TIMESTAMP` the same way —
    // confirmed against SQLite's own source that `ADD COLUMN` rejects any
    // non-constant default ("Cannot add a column with non-constant
    // default"), so it gets a constant placeholder default instead,
    // immediately backfilled via `UPDATE` (which has no such restriction).
    // Every future `add_edge` INSERT sets `updated_at` explicitly, so the
    // `''` placeholder is only ever seen by this one backfill step.
    M::up(
        "ALTER TABLE edges ADD COLUMN weight REAL NOT NULL DEFAULT 1.0;
        ALTER TABLE edges ADD COLUMN updated_at TEXT NOT NULL DEFAULT '';
        UPDATE edges SET updated_at = CURRENT_TIMESTAMP WHERE updated_at = '';",
    ),
    // Module C: repo state (state memory) — a single upserted-in-place
    // snapshot row per repo, not a history table.
    M::up(
        "CREATE TABLE IF NOT EXISTS repo_state (
            repo             TEXT PRIMARY KEY,
            updated_at       TEXT NOT NULL,
            last_scan_id     INTEGER,
            top_gotcha_ids   TEXT NOT NULL,
            top_decision_ids TEXT NOT NULL
        );",
    ),
    // Phase 2 (1.0 roadmap): bi-temporal node versioning. node_id is
    // deliberately not a hard FK — history must survive the node's own
    // eventual pruning, same reasoning as scan_history_entries.
    M::up(
        "CREATE TABLE IF NOT EXISTS node_versions (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            node_id     INTEGER NOT NULL,
            content     TEXT,
            start_line  INTEGER,
            end_line    INTEGER,
            valid_from  TEXT NOT NULL,
            valid_until TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_node_versions_node ON node_versions(node_id);",
    ),
    // Phase 3 (1.0 roadmap), Module 6: cross-tool session correlation. Not
    // scan-scoped — one row per notable write action, tagged with the
    // caller-supplied session_id so multiple MCP/REST clients sharing one
    // session_id produce one correlated activity feed.
    M::up(
        "CREATE TABLE IF NOT EXISTS session_events (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            repo        TEXT NOT NULL,
            session_id  TEXT NOT NULL,
            tool_name   TEXT NOT NULL,
            description TEXT NOT NULL,
            created_at  TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_session_events_repo_session ON session_events(repo, session_id);",
    ),
    // Phase 3 (1.0 roadmap), Module 7: hybrid native task manager + Linear
    // sync. external_source/external_id are both NULL for a native task;
    // the partial unique index only applies once external_source is set, so
    // native tasks (always NULL there) are never spuriously deduplicated
    // against each other.
    M::up(
        "CREATE TABLE IF NOT EXISTS tasks (
            id              INTEGER PRIMARY KEY AUTOINCREMENT,
            repo            TEXT NOT NULL,
            title           TEXT NOT NULL,
            description     TEXT,
            status          TEXT NOT NULL,
            priority        TEXT,
            assignee        TEXT,
            external_source TEXT,
            external_id     TEXT,
            session_id      TEXT,
            created_at      TEXT NOT NULL,
            updated_at      TEXT NOT NULL
        );
        CREATE UNIQUE INDEX IF NOT EXISTS idx_tasks_external ON tasks(external_source, external_id) WHERE external_source IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_tasks_repo ON tasks(repo);
        CREATE TABLE IF NOT EXISTS task_links (
            task_id  INTEGER NOT NULL,
            node_id  INTEGER NOT NULL,
            relation TEXT NOT NULL
        );
        CREATE UNIQUE INDEX IF NOT EXISTS idx_task_links_unique ON task_links(task_id, node_id, relation);",
    ),
    // Phase 4 (1.0 roadmap): lexical (BM25) search signal, redesigned
    // against this rebuild's actual architecture rather than the vault's
    // original tantivy-based spec (written when dense search was still a
    // separate heavy-tier/Qdrant concern) — SQLite already ships FTS5 in
    // the `bundled` build this workspace already uses, so a second
    // on-disk index with its own sync mechanism (tantivy) would just be
    // reinventing what FTS5's *external content* mode already gives for
    // free: an index the database itself keeps in sync via triggers, the
    // same "no reason to defer, no separate write path" reasoning `vec0`
    // already established for the dense index. The backfill INSERT
    // populates every pre-existing row (like this repo's own
    // already-populated graph.db) exactly once — deliberately no `WHERE id
    // NOT IN (SELECT rowid FROM nodes_fts)` guard here: confirmed via a
    // standalone probe that self-referentially reading the just-created,
    // still-empty FTS5 table inside the same INSERT...SELECT that
    // populates it silently corrupts the index (the content row lands, but
    // MATCH never finds it) — the guard is also unnecessary anyway, since
    // `rusqlite_migration`'s `user_version` tracking already guarantees
    // this whole step runs exactly once.
    M::up(
        "CREATE VIRTUAL TABLE IF NOT EXISTS nodes_fts USING fts5(name, content, content='nodes', content_rowid='id');
        CREATE TRIGGER IF NOT EXISTS nodes_fts_ai AFTER INSERT ON nodes BEGIN
            INSERT INTO nodes_fts(rowid, name, content) VALUES (new.id, new.name, new.content);
        END;
        CREATE TRIGGER IF NOT EXISTS nodes_fts_ad AFTER DELETE ON nodes BEGIN
            INSERT INTO nodes_fts(nodes_fts, rowid, name, content) VALUES ('delete', old.id, old.name, old.content);
        END;
        CREATE TRIGGER IF NOT EXISTS nodes_fts_au AFTER UPDATE ON nodes BEGIN
            INSERT INTO nodes_fts(nodes_fts, rowid, name, content) VALUES ('delete', old.id, old.name, old.content);
            INSERT INTO nodes_fts(rowid, name, content) VALUES (new.id, new.name, new.content);
        END;
        INSERT INTO nodes_fts(rowid, name, content) SELECT id, name, content FROM nodes;",
    ),
    // Gotcha review workflow: triage state for a node (meaningful for
    // Gotcha kind, harmless/unused on others). A constant default -- unlike
    // the earlier edges.updated_at migration, no backfill UPDATE is needed.
    // Superseded by the curation columns below within the same session --
    // the earlier `review_status` design modeled gotchas as bugs to close,
    // which is wrong (see the curation columns' own comment). Dropped
    // rather than left dead: `rusqlite` 0.40's bundled SQLite is well past
    // 3.35 (2021), so DROP COLUMN on a plain unindexed TEXT column is safe.
    M::up("ALTER TABLE nodes ADD COLUMN review_status TEXT NOT NULL DEFAULT 'open';"),
    M::up(
        "ALTER TABLE nodes DROP COLUMN review_status;
        ALTER TABLE nodes ADD COLUMN curated INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE nodes ADD COLUMN prominence TEXT NOT NULL DEFAULT 'full';
        ALTER TABLE nodes ADD COLUMN curation_reason TEXT;",
    ),
];

fn migrations() -> Migrations<'static> {
    Migrations::from_slice(MIGRATIONS_SLICE)
}

pub struct SqliteGraphStore {
    conn: Connection,
}

impl SqliteGraphStore {
    pub fn open(path: &Path) -> Result<Self> {
        ensure_vec_extension_registered();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| format!("creating parent dir for {}", path.display()))?;
        }
        let mut conn = Connection::open(path).with_context(|| format!("opening sqlite db at {}", path.display()))?;
        migrations().to_latest(&mut conn).context("running graph store migrations")?;
        conn.execute_batch(&vec_table_migration()).context("creating node_vectors virtual table")?;
        Ok(Self { conn })
    }

    pub fn open_in_memory() -> Result<Self> {
        ensure_vec_extension_registered();
        let mut conn = Connection::open_in_memory().context("opening in-memory sqlite db")?;
        migrations().to_latest(&mut conn).context("running graph store migrations")?;
        conn.execute_batch(&vec_table_migration()).context("creating node_vectors virtual table")?;
        Ok(Self { conn })
    }

    fn row_to_node(row: &rusqlite::Row) -> rusqlite::Result<Node> {
        let kind: String = row.get("kind")?;
        let prominence: String = row.get("prominence")?;
        Ok(Node {
            id: row.get("id")?,
            kind: NodeKind::from_db_str(&kind),
            repo: row.get("repo")?,
            path: row.get("path")?,
            name: row.get("name")?,
            container: row.get("container")?,
            start_line: row.get("start_line")?,
            end_line: row.get("end_line")?,
            content: row.get("content")?,
            curated: row.get("curated")?,
            prominence: NodeProminence::from_db_str(&prominence),
            curation_reason: row.get("curation_reason")?,
        })
    }

    fn row_to_edge(row: &rusqlite::Row) -> rusqlite::Result<Edge> {
        let relation: String = row.get("relation")?;
        Ok(Edge {
            id: row.get("id")?,
            repo: row.get("repo")?,
            src_id: row.get("src_id")?,
            dst_id: row.get("dst_id")?,
            relation: EdgeRelation::from_db_str(&relation),
            weight: row.get("weight")?,
            updated_at: row.get("updated_at")?,
        })
    }

    fn row_to_repo_state(row: &rusqlite::Row) -> rusqlite::Result<RepoState> {
        let top_gotcha_ids: String = row.get("top_gotcha_ids")?;
        let top_decision_ids: String = row.get("top_decision_ids")?;
        Ok(RepoState {
            repo: row.get("repo")?,
            updated_at: row.get("updated_at")?,
            last_scan_id: row.get("last_scan_id")?,
            top_gotcha_ids: serde_json::from_str(&top_gotcha_ids).unwrap_or_default(),
            top_decision_ids: serde_json::from_str(&top_decision_ids).unwrap_or_default(),
        })
    }

    fn row_to_node_version(row: &rusqlite::Row) -> rusqlite::Result<NodeVersion> {
        Ok(NodeVersion {
            id: row.get("id")?,
            node_id: row.get("node_id")?,
            content: row.get("content")?,
            start_line: row.get("start_line")?,
            end_line: row.get("end_line")?,
            valid_from: row.get("valid_from")?,
            valid_until: row.get("valid_until")?,
        })
    }

    fn row_to_session_event(row: &rusqlite::Row) -> rusqlite::Result<SessionEvent> {
        Ok(SessionEvent {
            id: row.get("id")?,
            repo: row.get("repo")?,
            session_id: row.get("session_id")?,
            tool_name: row.get("tool_name")?,
            description: row.get("description")?,
            created_at: row.get("created_at")?,
        })
    }

    fn row_to_task(row: &rusqlite::Row) -> rusqlite::Result<Task> {
        let status: String = row.get("status")?;
        Ok(Task {
            id: row.get("id")?,
            repo: row.get("repo")?,
            title: row.get("title")?,
            description: row.get("description")?,
            status: TaskStatus::from_db_str(&status),
            priority: row.get("priority")?,
            assignee: row.get("assignee")?,
            external_source: row.get("external_source")?,
            external_id: row.get("external_id")?,
            session_id: row.get("session_id")?,
            created_at: row.get("created_at")?,
            updated_at: row.get("updated_at")?,
        })
    }

    fn row_to_task_link(row: &rusqlite::Row) -> rusqlite::Result<TaskLink> {
        Ok(TaskLink { task_id: row.get("task_id")?, node_id: row.get("node_id")?, relation: row.get("relation")? })
    }

    fn row_to_scan_history(row: &rusqlite::Row) -> rusqlite::Result<ScanHistory> {
        Ok(ScanHistory {
            id: row.get("id")?,
            repo: row.get("repo")?,
            started_at: row.get("started_at")?,
            files_added: row.get("files_added")?,
            files_changed: row.get("files_changed")?,
            files_removed: row.get("files_removed")?,
            symbols_added: row.get("symbols_added")?,
            symbols_changed: row.get("symbols_changed")?,
            symbols_removed: row.get("symbols_removed")?,
            notes_added: row.get("notes_added")?,
        })
    }
}

impl GraphStore for SqliteGraphStore {
    fn add_node(&self, node: NewNode) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO nodes (kind, repo, path, name, container, start_line, end_line, content) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![node.kind.as_db_str(), node.repo, node.path, node.name, node.container, node.start_line, node.end_line, node.content],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    fn get_node(&self, repo: &str, id: i64) -> Result<Option<Node>> {
        let mut stmt = self.conn.prepare("SELECT * FROM nodes WHERE repo = ?1 AND id = ?2")?;
        let mut rows = stmt.query_map(rusqlite::params![repo, id], Self::row_to_node)?;
        Ok(rows.next().transpose()?)
    }

    fn nodes_by_kind(&self, repo: &str, kind: NodeKind) -> Result<Vec<Node>> {
        let mut stmt = self.conn.prepare("SELECT * FROM nodes WHERE repo = ?1 AND kind = ?2")?;
        let rows = stmt.query_map(rusqlite::params![repo, kind.as_db_str()], Self::row_to_node)?;
        rows.map(|r| r.map_err(anyhow::Error::from)).collect()
    }

    fn all_nodes(&self, repo: &str) -> Result<Vec<Node>> {
        let mut stmt = self.conn.prepare("SELECT * FROM nodes WHERE repo = ?1")?;
        let rows = stmt.query_map([repo], Self::row_to_node)?;
        rows.map(|r| r.map_err(anyhow::Error::from)).collect()
    }

    fn find_node(&self, repo: &str, kind: NodeKind, path: Option<&str>, name: Option<&str>, container: Option<&str>) -> Result<Option<Node>> {
        let mut stmt = self.conn.prepare("SELECT * FROM nodes WHERE repo = ?1 AND kind = ?2 AND path IS ?3 AND name IS ?4 AND container IS ?5")?;
        let mut rows = stmt.query_map(rusqlite::params![repo, kind.as_db_str(), path, name, container], Self::row_to_node)?;
        Ok(rows.next().transpose()?)
    }

    fn update_node(&self, repo: &str, id: i64, start_line: Option<i64>, end_line: Option<i64>, content: Option<String>) -> Result<()> {
        self.conn.execute(
            "UPDATE nodes SET start_line = ?1, end_line = ?2, content = ?3 WHERE repo = ?4 AND id = ?5",
            rusqlite::params![start_line, end_line, content, repo, id],
        )?;
        Ok(())
    }

    fn set_curation(&self, repo: &str, node_id: i64, prominence: NodeProminence, reason: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE nodes SET curated = 1, prominence = ?1, curation_reason = ?2 WHERE repo = ?3 AND id = ?4",
            rusqlite::params![prominence.as_db_str(), reason, repo, node_id],
        )?;
        Ok(())
    }

    fn delete_nodes(&self, repo: &str, ids: &[i64]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }

        // `edges.src_id`/`dst_id` both `REFERENCES nodes(id)` with no
        // cascade — a real bug found live-testing Phase 6c against this
        // repo itself: pruning any stale node that still had an edge
        // pointing to or from it (the common case — e.g. a removed file's
        // `DependsOn` edges, or a note's `Affects` edge onto a removed
        // symbol) hit a foreign-key constraint failure and aborted the
        // whole scan. Edges touching a pruned node no longer mean anything
        // anyway (one endpoint is gone), so they must be deleted first, in
        // the same repo scope as the node delete below.
        let edge_placeholders = ids.iter().enumerate().map(|(i, _)| format!("?{}", i + 2)).collect::<Vec<_>>().join(",");
        let edge_sql = format!("DELETE FROM edges WHERE repo = ?1 AND (src_id IN ({edge_placeholders}) OR dst_id IN ({edge_placeholders}))");
        let mut edge_params: Vec<&dyn rusqlite::ToSql> = vec![&repo];
        edge_params.extend(ids.iter().map(|id| id as &dyn rusqlite::ToSql));
        self.conn.execute(&edge_sql, edge_params.as_slice())?;

        let placeholders = ids.iter().enumerate().map(|(i, _)| format!("?{}", i + 2)).collect::<Vec<_>>().join(",");
        let sql = format!("DELETE FROM nodes WHERE repo = ?1 AND id IN ({placeholders})");
        let mut params: Vec<&dyn rusqlite::ToSql> = vec![&repo];
        params.extend(ids.iter().map(|id| id as &dyn rusqlite::ToSql));
        self.conn.execute(&sql, params.as_slice())?;

        // A pruned node's embedding (if any) must go with it — otherwise
        // `node_vectors` accumulates orphan rows forever, and a future
        // `search_similar` could return a hit for a node_id that no longer
        // exists in `nodes` at all. `node_vectors` isn't itself repo-scoped
        // (just `node_id`), but `ids` was already validated against `repo`
        // by the `DELETE FROM nodes` above, so no cross-repo leak risk here.
        let vec_placeholders = ids.iter().enumerate().map(|(i, _)| format!("?{}", i + 1)).collect::<Vec<_>>().join(",");
        let vec_sql = format!("DELETE FROM node_vectors WHERE node_id IN ({vec_placeholders})");
        let vec_params: Vec<&dyn rusqlite::ToSql> = ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
        self.conn.execute(&vec_sql, vec_params.as_slice())?;

        Ok(())
    }

    fn add_edge(&self, repo: &str, src_id: i64, dst_id: i64, relation: EdgeRelation) -> Result<i64> {
        // `updated_at` is set explicitly here rather than relying on a
        // schema DEFAULT — SQLite's ALTER TABLE ADD COLUMN can't take
        // CURRENT_TIMESTAMP as a default (confirmed against SQLite's own
        // source: "Cannot add a column with non-constant default"), so the
        // column has no live default at all; every insert must supply it.
        self.conn.execute(
            "INSERT INTO edges (repo, src_id, dst_id, relation, weight, updated_at) VALUES (?1, ?2, ?3, ?4, 1.0, CURRENT_TIMESTAMP)",
            rusqlite::params![repo, src_id, dst_id, relation.as_db_str()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    fn reinforce_edge(&self, repo: &str, edge_id: i64, bump_confirmed_at: bool) -> Result<()> {
        if bump_confirmed_at {
            self.conn.execute(
                "UPDATE edges SET weight = MIN(weight + 0.5, 5.0), updated_at = CURRENT_TIMESTAMP WHERE repo = ?1 AND id = ?2",
                rusqlite::params![repo, edge_id],
            )?;
        } else {
            self.conn.execute("UPDATE edges SET weight = MIN(weight + 0.5, 5.0) WHERE repo = ?1 AND id = ?2", rusqlite::params![repo, edge_id])?;
        }
        Ok(())
    }

    fn edges_from(&self, repo: &str, src_id: i64) -> Result<Vec<Edge>> {
        let mut stmt = self.conn.prepare("SELECT * FROM edges WHERE repo = ?1 AND src_id = ?2")?;
        let rows = stmt.query_map(rusqlite::params![repo, src_id], Self::row_to_edge)?;
        rows.map(|r| r.map_err(anyhow::Error::from)).collect()
    }

    fn edges_to(&self, repo: &str, dst_id: i64) -> Result<Vec<Edge>> {
        let mut stmt = self.conn.prepare("SELECT * FROM edges WHERE repo = ?1 AND dst_id = ?2")?;
        let rows = stmt.query_map(rusqlite::params![repo, dst_id], Self::row_to_edge)?;
        rows.map(|r| r.map_err(anyhow::Error::from)).collect()
    }

    fn all_edges(&self, repo: &str) -> Result<Vec<Edge>> {
        let mut stmt = self.conn.prepare("SELECT * FROM edges WHERE repo = ?1")?;
        let rows = stmt.query_map([repo], Self::row_to_edge)?;
        rows.map(|r| r.map_err(anyhow::Error::from)).collect()
    }

    fn delete_edges_from(&self, repo: &str, src_id: i64, relation: EdgeRelation) -> Result<()> {
        self.conn.execute(
            "DELETE FROM edges WHERE repo = ?1 AND src_id = ?2 AND relation = ?3",
            rusqlite::params![repo, src_id, relation.as_db_str()],
        )?;
        Ok(())
    }

    fn record_scan(&self, repo: &str, entries: &[NewScanHistoryEntry]) -> Result<i64> {
        let count = |kind: NodeKind, change: ScanChange| entries.iter().filter(|e| e.kind == kind && e.change == change).count() as i64;
        let files_added = count(NodeKind::File, ScanChange::Added);
        let files_changed = count(NodeKind::File, ScanChange::Changed);
        let files_removed = count(NodeKind::File, ScanChange::Removed);
        let symbols_added = count(NodeKind::Symbol, ScanChange::Added);
        let symbols_changed = count(NodeKind::Symbol, ScanChange::Changed);
        let symbols_removed = count(NodeKind::Symbol, ScanChange::Removed);
        let notes_added = entries.iter().filter(|e| matches!(e.kind, NodeKind::Gotcha | NodeKind::Decision | NodeKind::Note) && e.change == ScanChange::Added).count() as i64;

        self.conn.execute(
            "INSERT INTO scan_history (repo, files_added, files_changed, files_removed, symbols_added, symbols_changed, symbols_removed, notes_added) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![repo, files_added, files_changed, files_removed, symbols_added, symbols_changed, symbols_removed, notes_added],
        )?;
        let scan_id = self.conn.last_insert_rowid();

        for entry in entries {
            self.conn.execute(
                "INSERT INTO scan_history_entries (scan_id, node_id, kind, path, name, change) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![scan_id, entry.node_id, entry.kind.as_db_str(), entry.path, entry.name, entry.change.as_db_str()],
            )?;
        }
        Ok(scan_id)
    }

    fn latest_scan(&self, repo: &str) -> Result<Option<ScanHistory>> {
        let mut stmt = self.conn.prepare("SELECT * FROM scan_history WHERE repo = ?1 ORDER BY started_at DESC, id DESC LIMIT 1")?;
        let mut rows = stmt.query_map([repo], Self::row_to_scan_history)?;
        Ok(rows.next().transpose()?)
    }

    fn list_scans(&self, repo: &str) -> Result<Vec<ScanHistory>> {
        let mut stmt = self.conn.prepare("SELECT * FROM scan_history WHERE repo = ?1 ORDER BY started_at DESC, id DESC")?;
        let rows = stmt.query_map([repo], Self::row_to_scan_history)?;
        rows.map(|r| r.map_err(anyhow::Error::from)).collect()
    }

    fn scan_entries(&self, scan_id: i64) -> Result<Vec<ScanHistoryEntry>> {
        let mut stmt = self.conn.prepare("SELECT * FROM scan_history_entries WHERE scan_id = ?1")?;
        let rows = stmt.query_map([scan_id], |row| {
            let kind: String = row.get("kind")?;
            let change: String = row.get("change")?;
            Ok(ScanHistoryEntry {
                id: row.get("id")?,
                scan_id: row.get("scan_id")?,
                node_id: row.get("node_id")?,
                kind: NodeKind::from_db_str(&kind),
                path: row.get("path")?,
                name: row.get("name")?,
                change: ScanChange::from_db_str(&change),
            })
        })?;
        rows.map(|r| r.map_err(anyhow::Error::from)).collect()
    }

    fn set_embedding(&self, repo: &str, node_id: i64, embedding: &[f32]) -> Result<()> {
        anyhow::ensure!(embedding.len() == EMBEDDING_DIM, "embedding has {} dims, expected {EMBEDDING_DIM}", embedding.len());
        // Confirms node_id is actually in this repo before writing anything
        // — set_embedding must not let a caller attach a vector to a node
        // outside its own repo scope just because ids are globally unique.
        anyhow::ensure!(self.get_node(repo, node_id)?.is_some(), "node #{node_id} not found in repo {repo:?}");

        // vec0's `node_id INTEGER PRIMARY KEY` rejects a second INSERT on an
        // existing id — unlike docbrain (content-hash dedup means "already
        // exists" implies "unchanged," so it never re-embeds), a codebrain
        // symbol's content can genuinely change across rescans while its id
        // stays stable. Delete-then-insert rather than relying on
        // `INSERT OR REPLACE`/`ON CONFLICT` support on a virtual table,
        // which varies by `sqlite-vec` version.
        self.conn.execute("DELETE FROM node_vectors WHERE node_id = ?1", [node_id])?;
        self.conn.execute("INSERT INTO node_vectors (node_id, embedding) VALUES (?1, ?2)", rusqlite::params![node_id, embedding_to_bytes(embedding)])?;
        Ok(())
    }

    fn search_similar(&self, repo: &str, embedding: &[f32], top_k: usize, kind: Option<NodeKind>) -> Result<Vec<(Node, f32)>> {
        anyhow::ensure!(embedding.len() == EMBEDDING_DIM, "query embedding has {} dims, expected {EMBEDDING_DIM}", embedding.len());

        // Over-fetch then filter by repo/kind in Rust — vec0's own filtering
        // support varies by version, and this scan is cheap at the node
        // counts a self-hosted instance actually has. Same technique
        // `docbrain-graph`'s `search_similar` already uses and documents.
        let fetch_k = (top_k * 8).max(50) as i64;
        let mut stmt = self.conn.prepare("SELECT node_id, distance FROM node_vectors WHERE embedding MATCH ?1 AND k = ?2 ORDER BY distance")?;
        let rows = stmt.query_map(rusqlite::params![embedding_to_bytes(embedding), fetch_k], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, f64>(1)?)))?;

        let mut hits = Vec::new();
        for row in rows {
            let (node_id, distance) = row?;
            let Some(node) = self.get_node(repo, node_id)? else { continue };
            if kind.is_some_and(|k| node.kind != k) {
                continue;
            }
            hits.push((node, distance as f32));
            if hits.len() >= top_k {
                break;
            }
        }
        Ok(hits)
    }

    fn search_lexical(&self, repo: &str, query: &str, top_k: usize, kind: Option<NodeKind>) -> Result<Vec<(Node, f32)>> {
        let match_query = fts5_match_query(query);
        if match_query.is_empty() {
            return Ok(vec![]);
        }
        // Same over-fetch-then-filter-in-Rust technique as search_similar —
        // FTS5's MATCH can't be combined with an arbitrary repo/kind filter
        // inside the same virtual-table query efficiently, and node counts
        // at self-hosted scale make this cheap.
        let fetch_k = (top_k * 8).max(50) as i64;
        let mut stmt = self.conn.prepare("SELECT rowid, bm25(nodes_fts) FROM nodes_fts WHERE nodes_fts MATCH ?1 ORDER BY bm25(nodes_fts) LIMIT ?2")?;
        let rows = stmt.query_map(rusqlite::params![match_query, fetch_k], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, f64>(1)?)))?;

        let mut hits = Vec::new();
        for row in rows {
            let (node_id, bm25_score) = row?;
            let Some(node) = self.get_node(repo, node_id)? else { continue };
            if kind.is_some_and(|k| node.kind != k) {
                continue;
            }
            hits.push((node, bm25_score as f32));
            if hits.len() >= top_k {
                break;
            }
        }
        Ok(hits)
    }

    fn search_exact(&self, repo: &str, query: &str, top_k: usize, kind: Option<NodeKind>) -> Result<Vec<(Node, f32)>> {
        let mut stmt = self.conn.prepare(
            "SELECT * FROM nodes WHERE repo = ?1 AND (?2 IS NULL OR kind = ?2) AND (LOWER(name) = LOWER(?3) OR LOWER(name) LIKE '%' || LOWER(?3) || '%') \
             ORDER BY CASE WHEN LOWER(name) = LOWER(?3) THEN 0 ELSE 1 END, LENGTH(name) LIMIT ?4",
        )?;
        let rows = stmt.query_map(rusqlite::params![repo, kind.map(|k| k.as_db_str()), query, top_k as i64], Self::row_to_node)?;
        rows.map(|r| r.map_err(anyhow::Error::from).map(|n| { let exact = n.name.as_deref().is_some_and(|name| name.eq_ignore_ascii_case(query)); (n, if exact { 0.0 } else { 1.0 }) })).collect()
    }

    fn refresh_repo_state(&self, repo: &str) -> Result<RepoState> {
        let top_gotcha_ids = crate::rank_notes_by_weight(self, repo, NodeKind::Gotcha)?;
        let top_decision_ids = crate::rank_notes_by_weight(self, repo, NodeKind::Decision)?;
        let last_scan_id = self.latest_scan(repo)?.map(|s| s.id);
        let now: String = self.conn.query_row("SELECT CURRENT_TIMESTAMP", [], |r| r.get(0))?;
        let gotcha_json = serde_json::to_string(&top_gotcha_ids)?;
        let decision_json = serde_json::to_string(&top_decision_ids)?;

        agentops_sqlite_support::upsert(
            &self.conn,
            "repo_state",
            &["repo", "updated_at", "last_scan_id", "top_gotcha_ids", "top_decision_ids"],
            &["repo"],
            &[&repo, &now, &last_scan_id, &gotcha_json, &decision_json],
        )?;

        Ok(RepoState { repo: repo.to_string(), updated_at: now, last_scan_id, top_gotcha_ids, top_decision_ids })
    }

    fn get_repo_state(&self, repo: &str) -> Result<Option<RepoState>> {
        let mut stmt = self.conn.prepare("SELECT * FROM repo_state WHERE repo = ?1")?;
        let mut rows = stmt.query_map([repo], Self::row_to_repo_state)?;
        Ok(rows.next().transpose()?)
    }

    fn snapshot_node_version(&self, node_id: i64, content: Option<&str>, start_line: Option<i64>, end_line: Option<i64>) -> Result<()> {
        self.conn.execute(
            "UPDATE node_versions SET valid_until = CURRENT_TIMESTAMP WHERE node_id = ?1 AND valid_until IS NULL",
            rusqlite::params![node_id],
        )?;
        self.conn.execute(
            "INSERT INTO node_versions (node_id, content, start_line, end_line, valid_from, valid_until) VALUES (?1, ?2, ?3, ?4, CURRENT_TIMESTAMP, NULL)",
            rusqlite::params![node_id, content, start_line, end_line],
        )?;
        Ok(())
    }

    fn close_node_version(&self, node_id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE node_versions SET valid_until = CURRENT_TIMESTAMP WHERE node_id = ?1 AND valid_until IS NULL",
            rusqlite::params![node_id],
        )?;
        Ok(())
    }

    fn node_history(&self, node_id: i64) -> Result<Vec<NodeVersion>> {
        let mut stmt = self.conn.prepare("SELECT * FROM node_versions WHERE node_id = ?1 ORDER BY id DESC")?;
        let rows = stmt.query_map([node_id], Self::row_to_node_version)?;
        rows.map(|r| r.map_err(anyhow::Error::from)).collect()
    }

    fn node_as_of(&self, node_id: i64, timestamp: &str) -> Result<Option<NodeVersion>> {
        let mut stmt = self.conn.prepare(
            "SELECT * FROM node_versions WHERE node_id = ?1 AND valid_from <= ?2 AND (valid_until IS NULL OR valid_until > ?2) ORDER BY id DESC LIMIT 1",
        )?;
        let mut rows = stmt.query_map(rusqlite::params![node_id, timestamp], Self::row_to_node_version)?;
        Ok(rows.next().transpose()?)
    }

    fn record_session_event(&self, repo: &str, session_id: &str, tool_name: &str, description: &str) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO session_events (repo, session_id, tool_name, description, created_at) VALUES (?1, ?2, ?3, ?4, CURRENT_TIMESTAMP)",
            rusqlite::params![repo, session_id, tool_name, description],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    fn session_events(&self, repo: &str, session_id: &str) -> Result<Vec<SessionEvent>> {
        let mut stmt = self.conn.prepare("SELECT * FROM session_events WHERE repo = ?1 AND session_id = ?2 ORDER BY id ASC")?;
        let rows = stmt.query_map(rusqlite::params![repo, session_id], Self::row_to_session_event)?;
        rows.map(|r| r.map_err(anyhow::Error::from)).collect()
    }

    fn create_task(&self, task: NewTask) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO tasks (repo, title, description, status, priority, assignee, external_source, external_id, session_id, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
            rusqlite::params![task.repo, task.title, task.description, task.status.as_db_str(), task.priority, task.assignee, task.external_source, task.external_id, task.session_id],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    fn get_task(&self, id: i64) -> Result<Option<Task>> {
        let mut stmt = self.conn.prepare("SELECT * FROM tasks WHERE id = ?1")?;
        let mut rows = stmt.query_map([id], Self::row_to_task)?;
        Ok(rows.next().transpose()?)
    }

    fn list_tasks(&self, repo: &str) -> Result<Vec<Task>> {
        let mut stmt = self.conn.prepare("SELECT * FROM tasks WHERE repo = ?1 ORDER BY id ASC")?;
        let rows = stmt.query_map([repo], Self::row_to_task)?;
        rows.map(|r| r.map_err(anyhow::Error::from)).collect()
    }

    fn update_task_status(&self, id: i64, status: TaskStatus) -> Result<()> {
        self.conn.execute("UPDATE tasks SET status = ?1, updated_at = CURRENT_TIMESTAMP WHERE id = ?2", rusqlite::params![status.as_db_str(), id])?;
        Ok(())
    }

    // Deliberately not `agentops_sqlite_support::upsert` — that helper's
    // `ON CONFLICT DO UPDATE` overwrites *every* non-key column on a
    // repeated call, which would silently reset `created_at` to "now" on
    // every Linear-sync poll. A synced task's `created_at` must stay fixed
    // at its first pull; only `updated_at` and the synced fields should
    // move on subsequent polls.
    fn upsert_external_task(&self, task: NewTask) -> Result<i64> {
        anyhow::ensure!(task.external_source.is_some() && task.external_id.is_some(), "upsert_external_task requires both external_source and external_id");
        let existing: Option<i64> = self
            .conn
            .query_row("SELECT id FROM tasks WHERE external_source = ?1 AND external_id = ?2", rusqlite::params![task.external_source, task.external_id], |r| r.get(0))
            .optional()?;

        match existing {
            Some(id) => {
                self.conn.execute(
                    "UPDATE tasks SET repo = ?1, title = ?2, description = ?3, status = ?4, priority = ?5, assignee = ?6, session_id = ?7, updated_at = CURRENT_TIMESTAMP WHERE id = ?8",
                    rusqlite::params![task.repo, task.title, task.description, task.status.as_db_str(), task.priority, task.assignee, task.session_id, id],
                )?;
                Ok(id)
            }
            None => self.create_task(task),
        }
    }

    fn link_task(&self, task_id: i64, node_id: i64, relation: &str) -> Result<()> {
        self.conn.execute("INSERT OR IGNORE INTO task_links (task_id, node_id, relation) VALUES (?1, ?2, ?3)", rusqlite::params![task_id, node_id, relation])?;
        Ok(())
    }

    fn task_links(&self, task_id: i64) -> Result<Vec<TaskLink>> {
        let mut stmt = self.conn.prepare("SELECT * FROM task_links WHERE task_id = ?1")?;
        let rows = stmt.query_map([task_id], Self::row_to_task_link)?;
        rows.map(|r| r.map_err(anyhow::Error::from)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{prune_stale_nodes, upsert_node};

    fn node(repo: &str, kind: NodeKind, path: Option<&str>, name: Option<&str>) -> NewNode {
        NewNode { kind, repo: repo.to_string(), path: path.map(String::from), name: name.map(String::from), container: None, start_line: None, end_line: None, content: None }
    }

    fn node_with_container(repo: &str, path: &str, name: &str, container: &str) -> NewNode {
        NewNode { kind: NodeKind::Symbol, repo: repo.to_string(), path: Some(path.to_string()), name: Some(name.to_string()), container: Some(container.to_string()), start_line: None, end_line: None, content: None }
    }

    #[test]
    fn upsert_inserts_then_updates_in_place_preserving_id() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let mut n = node("repo-a", NodeKind::Symbol, Some("a.rs"), Some("foo"));
        n.content = Some("fn foo() {}".to_string());
        let id1 = upsert_node(&store, n.clone()).unwrap();

        n.content = Some("fn foo() { changed }".to_string());
        let id2 = upsert_node(&store, n).unwrap();

        assert_eq!(id1, id2, "re-upserting the same natural key must preserve the node's id");
        let node = store.get_node("repo-a", id1).unwrap().unwrap();
        assert_eq!(node.content.as_deref(), Some("fn foo() { changed }"));
        assert_eq!(store.all_nodes("repo-a").unwrap().len(), 1);
    }

    #[test]
    fn find_node_null_safe_comparison_does_not_cross_match_different_names() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        // Two File nodes, both with name = NULL, must be distinguishable by path.
        upsert_node(&store, node("repo-a", NodeKind::File, Some("a.rs"), None)).unwrap();
        upsert_node(&store, node("repo-a", NodeKind::File, Some("b.rs"), None)).unwrap();
        assert_eq!(store.all_nodes("repo-a").unwrap().len(), 2, "two File nodes with NULL name must not collide");

        let found = store.find_node("repo-a", NodeKind::File, Some("a.rs"), None, None).unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().path.as_deref(), Some("a.rs"));
    }

    /// Regression test for a confirmed real bug found via live testing
    /// against this actual repo: `agentops-graph/src/lib.rs` defines
    /// `as_db_str` three separate times (once per enum's `impl` block).
    /// Before `container` existed, all three collapsed into one row under
    /// `(repo, kind, path, name)`, silently discarding two of the three.
    #[test]
    fn same_name_in_different_containers_within_one_file_does_not_collide() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        upsert_node(&store, node_with_container("repo-a", "lib.rs", "as_db_str", "NodeKind")).unwrap();
        upsert_node(&store, node_with_container("repo-a", "lib.rs", "as_db_str", "EdgeRelation")).unwrap();
        upsert_node(&store, node_with_container("repo-a", "lib.rs", "as_db_str", "ScanChange")).unwrap();

        let all = store.nodes_by_kind("repo-a", NodeKind::Symbol).unwrap();
        assert_eq!(all.len(), 3, "three distinct impls' methods must not collapse into one node: {all:?}");

        let found = store.find_node("repo-a", NodeKind::Symbol, Some("lib.rs"), Some("as_db_str"), Some("EdgeRelation")).unwrap();
        assert_eq!(found.unwrap().container.as_deref(), Some("EdgeRelation"));
    }

    #[test]
    fn nodes_never_leak_across_repos() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        upsert_node(&store, node("repo-a", NodeKind::Symbol, Some("a.rs"), Some("foo"))).unwrap();
        upsert_node(&store, node("repo-b", NodeKind::Symbol, Some("a.rs"), Some("foo"))).unwrap();

        assert_eq!(store.all_nodes("repo-a").unwrap().len(), 1);
        assert_eq!(store.all_nodes("repo-b").unwrap().len(), 1);
        assert_eq!(store.nodes_by_kind("repo-a", NodeKind::Symbol).unwrap().len(), 1);

        let a_id = store.all_nodes("repo-a").unwrap()[0].id;
        // Same symbol name/path exists in repo-b with a different id (or the
        // same id, depending on insert order) — get_node must not return a
        // repo-b node when asked under repo-a's scope even if ids collide.
        let b_id = store.all_nodes("repo-b").unwrap()[0].id;
        assert!(store.get_node("repo-a", b_id).unwrap().is_none() || a_id != b_id);
    }

    #[test]
    fn edges_are_repo_scoped() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let a = upsert_node(&store, node("repo-a", NodeKind::Symbol, Some("a.rs"), Some("foo"))).unwrap();
        let b = upsert_node(&store, node("repo-a", NodeKind::Symbol, Some("a.rs"), Some("bar"))).unwrap();
        store.add_edge("repo-a", a, b, EdgeRelation::DependsOn).unwrap();

        assert_eq!(store.edges_from("repo-a", a).unwrap().len(), 1);
        assert_eq!(store.edges_from("repo-b", a).unwrap().len(), 0, "a different repo scope must see zero edges even for the same node id");
    }

    #[test]
    fn prune_stale_nodes_removes_only_untouched_nodes_in_scope() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let keep = upsert_node(&store, node("repo-a", NodeKind::File, Some("keep.rs"), None)).unwrap();
        let stale = upsert_node(&store, node("repo-a", NodeKind::File, Some("stale.rs"), None)).unwrap();
        let other_repo = upsert_node(&store, node("repo-b", NodeKind::File, Some("stale.rs"), None)).unwrap();

        let pruned = prune_stale_nodes(&store, "repo-a", NodeKind::File, &[keep]).unwrap();
        assert_eq!(pruned.len(), 1);
        assert_eq!(pruned[0].id, stale);
        assert!(store.get_node("repo-a", keep).unwrap().is_some());
        assert!(store.get_node("repo-a", stale).unwrap().is_none());
        assert!(store.get_node("repo-b", other_repo).unwrap().is_some(), "pruning repo-a must not touch repo-b");
    }

    #[test]
    fn record_scan_derives_aggregate_counts_from_entries() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let entries = vec![
            NewScanHistoryEntry { node_id: 1, kind: NodeKind::File, path: Some("a.rs".into()), name: None, change: ScanChange::Added },
            NewScanHistoryEntry { node_id: 2, kind: NodeKind::Symbol, path: Some("a.rs".into()), name: Some("foo".into()), change: ScanChange::Added },
            NewScanHistoryEntry { node_id: 3, kind: NodeKind::Symbol, path: Some("a.rs".into()), name: Some("bar".into()), change: ScanChange::Changed },
            NewScanHistoryEntry { node_id: 4, kind: NodeKind::Symbol, path: Some("b.rs".into()), name: Some("baz".into()), change: ScanChange::Removed },
        ];
        let scan_id = store.record_scan("repo-a", &entries).unwrap();

        let scan = store.latest_scan("repo-a").unwrap().unwrap();
        assert_eq!(scan.id, scan_id);
        assert_eq!(scan.files_added, 1);
        assert_eq!(scan.symbols_added, 1);
        assert_eq!(scan.symbols_changed, 1);
        assert_eq!(scan.symbols_removed, 1);

        assert_eq!(store.scan_entries(scan_id).unwrap().len(), 4);
        assert_eq!(store.list_scans("repo-a").unwrap().len(), 1);
    }

    #[test]
    fn scan_history_is_repo_scoped() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        store.record_scan("repo-a", &[]).unwrap();
        assert!(store.latest_scan("repo-b").unwrap().is_none());
    }

    fn unit_vec(dominant: usize) -> Vec<f32> {
        let mut v = vec![0.0_f32; EMBEDDING_DIM];
        v[dominant] = 1.0;
        v
    }

    #[test]
    fn search_similar_ranks_nearest_first() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let close = upsert_node(&store, node("repo-a", NodeKind::Symbol, Some("a.rs"), Some("close"))).unwrap();
        let far = upsert_node(&store, node("repo-a", NodeKind::Symbol, Some("b.rs"), Some("far"))).unwrap();
        store.set_embedding("repo-a", close, &unit_vec(0)).unwrap();
        store.set_embedding("repo-a", far, &unit_vec(EMBEDDING_DIM - 1)).unwrap();

        let hits = store.search_similar("repo-a", &unit_vec(0), 2, None).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].0.id, close, "the near vector must rank first: {hits:?}");
    }

    #[test]
    fn search_similar_respects_repo_and_kind_scoping() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let symbol = upsert_node(&store, node("repo-a", NodeKind::Symbol, Some("a.rs"), Some("sym"))).unwrap();
        let gotcha = upsert_node(&store, node("repo-a", NodeKind::Gotcha, None, Some("gotcha"))).unwrap();
        let other_repo = upsert_node(&store, node("repo-b", NodeKind::Symbol, Some("a.rs"), Some("sym"))).unwrap();
        store.set_embedding("repo-a", symbol, &unit_vec(0)).unwrap();
        store.set_embedding("repo-a", gotcha, &unit_vec(0)).unwrap();
        store.set_embedding("repo-b", other_repo, &unit_vec(0)).unwrap();

        let kind_scoped = store.search_similar("repo-a", &unit_vec(0), 10, Some(NodeKind::Symbol)).unwrap();
        assert_eq!(kind_scoped.len(), 1);
        assert_eq!(kind_scoped[0].0.id, symbol);

        let repo_scoped = store.search_similar("repo-a", &unit_vec(0), 10, None).unwrap();
        assert!(repo_scoped.iter().all(|(n, _)| n.repo == "repo-a"), "must never return a different repo's node: {repo_scoped:?}");
    }

    #[test]
    fn search_lexical_finds_a_symbol_by_content_keyword_no_embedding_needed() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        upsert_node(&store, NewNode { kind: NodeKind::Symbol, repo: "repo-a".into(), path: Some("a.rs".into()), name: Some("verify_token".into()), container: None, start_line: Some(1), end_line: Some(3), content: Some("fn verify_token() { check_signature_expiry() }".into()) })
            .unwrap();
        upsert_node(&store, NewNode { kind: NodeKind::Symbol, repo: "repo-a".into(), path: Some("b.rs".into()), name: Some("unrelated".into()), container: None, start_line: Some(1), end_line: Some(2), content: Some("fn unrelated() {}".into()) })
            .unwrap();

        let hits = store.search_lexical("repo-a", "signature expiry", 10, None).unwrap();
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(hits[0].0.name.as_deref(), Some("verify_token"));
    }

    #[test]
    fn search_lexical_stays_in_sync_across_update_and_delete_with_no_explicit_reindex_call() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let id = upsert_node(&store, node("repo-a", NodeKind::Symbol, Some("a.rs"), Some("foo"))).unwrap();
        store.update_node("repo-a", id, Some(1), Some(2), Some("fn foo() { unique_marker_xyz() }".into())).unwrap();

        let hits = store.search_lexical("repo-a", "unique_marker_xyz", 10, None).unwrap();
        assert_eq!(hits.len(), 1, "the FTS5 trigger must pick up update_node's write with no separate index call: {hits:?}");

        store.delete_nodes("repo-a", &[id]).unwrap();
        let hits_after_delete = store.search_lexical("repo-a", "unique_marker_xyz", 10, None).unwrap();
        assert!(hits_after_delete.is_empty(), "a deleted node must not remain searchable: {hits_after_delete:?}");
    }

    #[test]
    fn search_lexical_never_crosses_repo_scope() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        upsert_node(&store, NewNode { kind: NodeKind::Symbol, repo: "repo-b".into(), path: Some("a.rs".into()), name: Some("sym".into()), container: None, start_line: None, end_line: None, content: Some("shared_keyword_marker".into()) })
            .unwrap();

        let hits = store.search_lexical("repo-a", "shared_keyword_marker", 10, None).unwrap();
        assert!(hits.is_empty(), "a different repo's content must never leak in: {hits:?}");
    }

    #[test]
    fn search_lexical_tolerates_punctuation_that_would_otherwise_be_invalid_fts5_syntax() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        upsert_node(&store, node("repo-a", NodeKind::Symbol, Some("a.rs"), Some("foo"))).unwrap();

        // A raw FTS5 MATCH with unbalanced quotes/operators like this would
        // normally throw a syntax error — must not panic or bubble a raw
        // SQL error up to the caller.
        let result = store.search_lexical("repo-a", "AND \"unterminated OR (", 10, None);
        assert!(result.is_ok(), "{result:?}");
    }

    #[test]
    fn search_exact_ranks_an_exact_name_match_before_a_substring_match() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        upsert_node(&store, node("repo-a", NodeKind::Symbol, Some("a.rs"), Some("verify_token_expiry"))).unwrap();
        upsert_node(&store, node("repo-a", NodeKind::Symbol, Some("b.rs"), Some("verify_token"))).unwrap();

        let hits = store.search_exact("repo-a", "verify_token", 10, None).unwrap();
        assert_eq!(hits.len(), 2, "{hits:?}");
        assert_eq!(hits[0].0.name.as_deref(), Some("verify_token"), "the exact match must rank first: {hits:?}");
    }

    #[test]
    fn search_exact_is_case_insensitive() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        upsert_node(&store, node("repo-a", NodeKind::Symbol, Some("a.rs"), Some("VerifyToken"))).unwrap();

        let hits = store.search_exact("repo-a", "verifytoken", 10, None).unwrap();
        assert_eq!(hits.len(), 1, "{hits:?}");
    }

    #[test]
    fn search_exact_respects_repo_and_kind_scoping() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        upsert_node(&store, node("repo-a", NodeKind::Symbol, Some("a.rs"), Some("shared_name"))).unwrap();
        upsert_node(&store, node("repo-a", NodeKind::Gotcha, None, Some("shared_name"))).unwrap();
        upsert_node(&store, node("repo-b", NodeKind::Symbol, Some("a.rs"), Some("shared_name"))).unwrap();

        let kind_scoped = store.search_exact("repo-a", "shared_name", 10, Some(NodeKind::Symbol)).unwrap();
        assert_eq!(kind_scoped.len(), 1);

        let repo_scoped = store.search_exact("repo-a", "shared_name", 10, None).unwrap();
        assert!(repo_scoped.iter().all(|(n, _)| n.repo == "repo-a"), "{repo_scoped:?}");
    }

    /// Regression test for the confirmed lifecycle pitfall this audit
    /// caught: a symbol's content (and thus its embedding) can change
    /// across rescans while its node_id stays stable. `vec0`'s
    /// `node_id INTEGER PRIMARY KEY` would reject a naive second INSERT.
    #[test]
    fn set_embedding_replaces_rather_than_erroring_on_an_existing_node() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let id = upsert_node(&store, node("repo-a", NodeKind::Symbol, Some("a.rs"), Some("sym"))).unwrap();
        store.set_embedding("repo-a", id, &unit_vec(0)).unwrap();
        store.set_embedding("repo-a", id, &unit_vec(1)).unwrap();

        let hits = store.search_similar("repo-a", &unit_vec(1), 1, None).unwrap();
        assert_eq!(hits.len(), 1, "the replaced embedding must be the only one found, not two accumulated rows: {hits:?}");
        assert_eq!(hits[0].0.id, id);
    }

    #[test]
    fn new_nodes_default_to_uncurated_full_prominence() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let id = upsert_node(&store, node("repo-a", NodeKind::Gotcha, None, Some("g"))).unwrap();
        let n = store.get_node("repo-a", id).unwrap().unwrap();
        assert!(!n.curated);
        assert_eq!(n.prominence, NodeProminence::Full);
        assert_eq!(n.curation_reason, None);
    }

    #[test]
    fn set_curation_survives_a_subsequent_rescan() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let n = node("repo-a", NodeKind::Gotcha, None, Some("g"));
        let id = upsert_node(&store, n.clone()).unwrap();

        store.set_curation("repo-a", id, NodeProminence::Reduced, Some("only affects old Linux envs")).unwrap();
        let curated = store.get_node("repo-a", id).unwrap().unwrap();
        assert!(curated.curated);
        assert_eq!(curated.prominence, NodeProminence::Reduced);
        assert_eq!(curated.curation_reason.as_deref(), Some("only affects old Linux envs"));

        // Same natural key upserted again, exactly what a rescan does --
        // upsert_node must never touch curation, only set_curation.
        upsert_node(&store, n).unwrap();
        let after_rescan = store.get_node("repo-a", id).unwrap().unwrap();
        assert_eq!(after_rescan.prominence, NodeProminence::Reduced, "a rescan must not silently reset a curated gotcha's prominence back to Full");
        assert_eq!(after_rescan.curation_reason.as_deref(), Some("only affects old Linux envs"));
    }

    /// Regression test for the other confirmed lifecycle pitfall: pruning a
    /// node must also clean up its `node_vectors` row, or a future
    /// `search_similar` could return a hit for a node_id that no longer
    /// exists in `nodes` at all.
    #[test]
    fn delete_nodes_cleans_up_orphaned_embedding_rows() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let keep = upsert_node(&store, node("repo-a", NodeKind::File, Some("keep.rs"), None)).unwrap();
        let stale = upsert_node(&store, node("repo-a", NodeKind::File, Some("stale.rs"), None)).unwrap();
        store.set_embedding("repo-a", keep, &unit_vec(0)).unwrap();
        store.set_embedding("repo-a", stale, &unit_vec(0)).unwrap();

        prune_stale_nodes(&store, "repo-a", NodeKind::File, &[keep]).unwrap();

        let hits = store.search_similar("repo-a", &unit_vec(0), 10, None).unwrap();
        assert_eq!(hits.len(), 1, "the pruned node's embedding must be gone too, not orphaned: {hits:?}");
        assert_eq!(hits[0].0.id, keep);
    }

    /// Regression test for a real bug found live-testing Phase 6c against
    /// this repo itself: pruning a stale node that still had an edge
    /// pointing to or from it hit a foreign-key constraint failure
    /// (`edges.src_id`/`dst_id` both `REFERENCES nodes(id)`, no cascade)
    /// and aborted the whole scan — the common case (a removed file's
    /// `DependsOn` edges to a kept file), not an edge case.
    #[test]
    fn delete_nodes_removes_edges_touching_the_pruned_node_first() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let keep = upsert_node(&store, node("repo-a", NodeKind::File, Some("keep.rs"), None)).unwrap();
        let stale = upsert_node(&store, node("repo-a", NodeKind::File, Some("stale.rs"), None)).unwrap();
        // Edges in both directions relative to the node being pruned.
        store.add_edge("repo-a", stale, keep, EdgeRelation::DependsOn).unwrap();
        store.add_edge("repo-a", keep, stale, EdgeRelation::DependsOn).unwrap();

        prune_stale_nodes(&store, "repo-a", NodeKind::File, &[keep]).unwrap();

        assert!(store.get_node("repo-a", stale).unwrap().is_none(), "the stale node must actually be gone");
        assert!(store.edges_from("repo-a", keep).unwrap().is_empty(), "an edge pointing at a pruned node must not survive it");
    }

    #[test]
    fn set_embedding_rejects_a_node_outside_the_given_repo() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let id = upsert_node(&store, node("repo-a", NodeKind::Symbol, Some("a.rs"), Some("sym"))).unwrap();
        assert!(store.set_embedding("repo-b", id, &unit_vec(0)).is_err());
    }

    #[test]
    fn new_edges_start_at_weight_one_with_a_real_timestamp() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let a = upsert_node(&store, node("repo-a", NodeKind::Gotcha, None, Some("g"))).unwrap();
        let b = upsert_node(&store, node("repo-a", NodeKind::Symbol, Some("a.rs"), Some("sym"))).unwrap();
        let edge_id = store.add_edge("repo-a", a, b, EdgeRelation::Affects).unwrap();

        let edge = store.edges_from("repo-a", a).unwrap().into_iter().find(|e| e.id == edge_id).unwrap();
        assert_eq!(edge.weight, 1.0);
        assert!(!edge.updated_at.is_empty(), "updated_at must be populated on insert, not left blank");
    }

    #[test]
    fn reinforce_edge_bumps_weight_and_caps_it() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let a = upsert_node(&store, node("repo-a", NodeKind::Gotcha, None, Some("g"))).unwrap();
        let b = upsert_node(&store, node("repo-a", NodeKind::Symbol, Some("a.rs"), Some("sym"))).unwrap();
        let edge_id = store.add_edge("repo-a", a, b, EdgeRelation::Affects).unwrap();

        for _ in 0..20 {
            store.reinforce_edge("repo-a", edge_id, true).unwrap();
        }

        let edge = store.edges_from("repo-a", a).unwrap().into_iter().find(|e| e.id == edge_id).unwrap();
        assert_eq!(edge.weight, 5.0, "weight must be capped, not grow unbounded");
    }

    #[test]
    fn reinforce_edge_is_repo_scoped() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let a = upsert_node(&store, node("repo-a", NodeKind::Gotcha, None, Some("g"))).unwrap();
        let b = upsert_node(&store, node("repo-a", NodeKind::Symbol, Some("a.rs"), Some("sym"))).unwrap();
        let edge_id = store.add_edge("repo-a", a, b, EdgeRelation::Affects).unwrap();

        store.reinforce_edge("repo-b", edge_id, true).unwrap();

        let edge = store.edges_from("repo-a", a).unwrap().into_iter().find(|e| e.id == edge_id).unwrap();
        assert_eq!(edge.weight, 1.0, "reinforcing under the wrong repo must not affect the real edge");
    }

    #[test]
    fn refresh_repo_state_ranks_gotchas_by_reinforced_weight() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let popular = upsert_node(&store, node("repo-a", NodeKind::Gotcha, None, Some("popular"))).unwrap();
        let rare = upsert_node(&store, node("repo-a", NodeKind::Gotcha, None, Some("rare"))).unwrap();
        let sym = upsert_node(&store, node("repo-a", NodeKind::Symbol, Some("a.rs"), Some("sym"))).unwrap();

        let popular_edge = store.add_edge("repo-a", popular, sym, EdgeRelation::Affects).unwrap();
        store.add_edge("repo-a", rare, sym, EdgeRelation::Affects).unwrap();
        store.reinforce_edge("repo-a", popular_edge, true).unwrap();
        store.reinforce_edge("repo-a", popular_edge, true).unwrap();

        let state = store.refresh_repo_state("repo-a").unwrap();
        assert_eq!(state.top_gotcha_ids, vec![popular, rare], "the more-reinforced gotcha must rank first");
        assert!(state.top_decision_ids.is_empty());

        let cached = store.get_repo_state("repo-a").unwrap().unwrap();
        assert_eq!(cached.top_gotcha_ids, state.top_gotcha_ids, "get_repo_state must return what refresh_repo_state just persisted");
    }

    #[test]
    fn note_score_dampens_a_reduced_prominence_gotcha_below_an_otherwise_identical_full_one() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let full = upsert_node(&store, node("repo-a", NodeKind::Gotcha, None, Some("full"))).unwrap();
        let reduced = upsert_node(&store, node("repo-a", NodeKind::Gotcha, None, Some("reduced"))).unwrap();
        let sym = upsert_node(&store, node("repo-a", NodeKind::Symbol, Some("a.rs"), Some("sym"))).unwrap();

        // Identical edges -- the only difference is curation.
        store.add_edge("repo-a", full, sym, EdgeRelation::Affects).unwrap();
        store.add_edge("repo-a", reduced, sym, EdgeRelation::Affects).unwrap();
        store.set_curation("repo-a", reduced, NodeProminence::Reduced, Some("niche")).unwrap();

        let full_node = store.get_node("repo-a", full).unwrap().unwrap();
        let reduced_node = store.get_node("repo-a", reduced).unwrap().unwrap();
        let full_score = crate::note_score(&store, "repo-a", &full_node).unwrap();
        let reduced_score = crate::note_score(&store, "repo-a", &reduced_node).unwrap();
        assert!(reduced_score < full_score, "a Reduced gotcha with identical edges must score lower: full={full_score}, reduced={reduced_score}");
        assert!((reduced_score - full_score * 0.1).abs() < 1e-9, "expected exactly the 0.1 dampening multiplier: full={full_score}, reduced={reduced_score}");
    }

    #[test]
    fn get_repo_state_returns_none_before_any_refresh() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        assert!(store.get_repo_state("repo-a").unwrap().is_none());
    }

    /// The exact scenario this repo's own `.context/graph.db` is in:
    /// existing rows from before `rusqlite_migration` existed, at the
    /// SQLite default `user_version = 0`. Confirms the migration both adds
    /// the new columns/table *and* leaves existing rows intact.
    #[test]
    fn migrating_a_pre_existing_database_preserves_rows_and_adds_new_columns() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        // Simulate the old, pre-migration-library schema: no weight/updated_at
        // columns, no repo_state table — exactly what MIGRATIONS_SLICE's
        // first four steps produce.
        conn.execute_batch(
            "CREATE TABLE nodes (id INTEGER PRIMARY KEY AUTOINCREMENT, kind TEXT NOT NULL, repo TEXT NOT NULL, path TEXT, name TEXT, container TEXT, start_line INTEGER, end_line INTEGER, content TEXT);
             CREATE TABLE edges (id INTEGER PRIMARY KEY AUTOINCREMENT, repo TEXT NOT NULL, src_id INTEGER NOT NULL, dst_id INTEGER NOT NULL, relation TEXT NOT NULL);
             INSERT INTO nodes (kind, repo, name) VALUES ('gotcha', 'repo-a', 'pre-existing');
             INSERT INTO edges (repo, src_id, dst_id, relation) VALUES ('repo-a', 1, 1, 'affects');",
        )
        .unwrap();

        let mut conn = conn;
        migrations().to_latest(&mut conn).unwrap();

        let mut store = SqliteGraphStore { conn };
        let node = store.get_node("repo-a", 1).unwrap().unwrap();
        assert_eq!(node.name.as_deref(), Some("pre-existing"), "pre-existing row must survive the migration");

        let edge = store.edges_from("repo-a", 1).unwrap().into_iter().next().unwrap();
        assert_eq!(edge.weight, 1.0, "pre-existing edges must backfill to the default weight");
        assert!(!edge.updated_at.is_empty(), "pre-existing edges must backfill a real updated_at, not stay blank");

        let lexical_hits = store.search_lexical("repo-a", "pre-existing", 10, None).unwrap();
        assert_eq!(lexical_hits.len(), 1, "the FTS5 backfill must index rows that existed before this migration ran: {lexical_hits:?}");

        // Running it again must be a true no-op, not an error or a second backfill.
        migrations().to_latest(&mut store.conn).unwrap();
    }

    #[test]
    fn snapshot_node_version_closes_the_open_row_and_opens_a_new_one() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let id = upsert_node(&store, node("repo-a", NodeKind::Symbol, Some("a.rs"), Some("foo"))).unwrap();

        store.snapshot_node_version(id, Some("fn foo() {}"), Some(1), Some(3)).unwrap();
        let history = store.node_history(id).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].content.as_deref(), Some("fn foo() {}"));
        assert!(history[0].valid_until.is_none(), "the first version must still be open");

        store.snapshot_node_version(id, Some("fn foo() { changed }"), Some(1), Some(3)).unwrap();
        let history = store.node_history(id).unwrap();
        assert_eq!(history.len(), 2, "must have two versions now, not overwritten in place");
        assert_eq!(history[0].content.as_deref(), Some("fn foo() { changed }"), "most recent first");
        assert!(history[0].valid_until.is_none());
        assert!(history[1].valid_until.is_some(), "the first version must now be closed");
    }

    #[test]
    fn close_node_version_closes_without_opening_a_new_one() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let id = upsert_node(&store, node("repo-a", NodeKind::Symbol, Some("a.rs"), Some("foo"))).unwrap();
        store.snapshot_node_version(id, Some("fn foo() {}"), Some(1), Some(3)).unwrap();

        store.close_node_version(id).unwrap();

        let history = store.node_history(id).unwrap();
        assert_eq!(history.len(), 1, "close must not insert a new version");
        assert!(history[0].valid_until.is_some());
    }

    #[test]
    fn node_as_of_returns_the_version_valid_at_that_time_not_just_the_latest() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let id = upsert_node(&store, node("repo-a", NodeKind::Symbol, Some("a.rs"), Some("foo"))).unwrap();

        store.snapshot_node_version(id, Some("v1"), None, None).unwrap();
        let mid_timestamp: String = store.conn.query_row("SELECT CURRENT_TIMESTAMP", [], |r| r.get(0)).unwrap();
        // Ensure the two snapshots land in different seconds — CURRENT_TIMESTAMP
        // has one-second granularity, and the test needs `mid_timestamp` to be
        // strictly between the two versions' timestamps to be meaningful.
        std::thread::sleep(std::time::Duration::from_secs(1));
        store.snapshot_node_version(id, Some("v2"), None, None).unwrap();

        let as_of_mid = store.node_as_of(id, &mid_timestamp).unwrap().unwrap();
        assert_eq!(as_of_mid.content.as_deref(), Some("v1"), "must return what was valid at that time, not the current version");

        let now: String = store.conn.query_row("SELECT CURRENT_TIMESTAMP", [], |r| r.get(0)).unwrap();
        let as_of_now = store.node_as_of(id, &now).unwrap().unwrap();
        assert_eq!(as_of_now.content.as_deref(), Some("v2"));
    }

    #[test]
    fn node_as_of_returns_none_for_a_node_with_no_version_history() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let id = upsert_node(&store, node("repo-a", NodeKind::Symbol, Some("a.rs"), Some("foo"))).unwrap();
        assert!(store.node_as_of(id, "2020-01-01 00:00:00").unwrap().is_none());
        assert!(store.node_history(id).unwrap().is_empty());
    }

    #[test]
    fn session_events_returns_only_the_matching_repo_and_session_in_order() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        store.record_session_event("repo-a", "sess-1", "scan_repo", "scanned repo-a").unwrap();
        store.record_session_event("repo-a", "sess-1", "add_note", "wrote a gotcha").unwrap();
        store.record_session_event("repo-a", "sess-2", "scan_repo", "a different session").unwrap();
        store.record_session_event("repo-b", "sess-1", "scan_repo", "same session id, different repo").unwrap();

        let events = store.session_events("repo-a", "sess-1").unwrap();
        assert_eq!(events.len(), 2, "must exclude other sessions and other repos: {events:?}");
        assert_eq!(events[0].tool_name, "scan_repo", "oldest first");
        assert_eq!(events[1].tool_name, "add_note");
    }

    fn new_task(repo: &str, title: &str) -> NewTask {
        NewTask { repo: repo.into(), title: title.into(), description: None, status: TaskStatus::Todo, priority: None, assignee: None, external_source: None, external_id: None, session_id: None }
    }

    #[test]
    fn create_then_get_and_list_a_native_task() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let id = store.create_task(new_task("repo-a", "fix the thing")).unwrap();

        let task = store.get_task(id).unwrap().unwrap();
        assert_eq!(task.title, "fix the thing");
        assert_eq!(task.status, TaskStatus::Todo);
        assert!(task.external_source.is_none(), "a native task must have no external_source");

        let listed = store.list_tasks("repo-a").unwrap();
        assert_eq!(listed.len(), 1);
    }

    #[test]
    fn update_task_status_moves_it_along_and_bumps_updated_at() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let id = store.create_task(new_task("repo-a", "task")).unwrap();

        store.update_task_status(id, TaskStatus::InProgress).unwrap();
        let task = store.get_task(id).unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::InProgress);
    }

    #[test]
    fn upsert_external_task_is_idempotent_on_external_source_and_id_and_preserves_created_at() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let mut task = new_task("repo-a", "Linear issue: fix login");
        task.external_source = Some("linear".into());
        task.external_id = Some("LIN-123".into());

        let first_id = store.upsert_external_task(task.clone()).unwrap();
        let first = store.get_task(first_id).unwrap().unwrap();

        std::thread::sleep(std::time::Duration::from_secs(1));
        task.title = "Linear issue: fix login (updated)".into();
        task.status = TaskStatus::InProgress;
        let second_id = store.upsert_external_task(task).unwrap();
        let second = store.get_task(second_id).unwrap().unwrap();

        assert_eq!(first_id, second_id, "re-pulling the same external issue must update in place, not duplicate");
        assert_eq!(store.list_tasks("repo-a").unwrap().len(), 1);
        assert_eq!(second.title, "Linear issue: fix login (updated)");
        assert_eq!(second.status, TaskStatus::InProgress);
        assert_eq!(second.created_at, first.created_at, "created_at must survive a resync, not reset to now");
        assert_ne!(second.updated_at, first.updated_at, "updated_at must move on a real resync");
    }

    #[test]
    fn link_task_is_idempotent_and_task_links_returns_only_that_tasks_links() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let task_id = store.create_task(new_task("repo-a", "task")).unwrap();
        let other_task_id = store.create_task(new_task("repo-a", "other task")).unwrap();
        let symbol_id = upsert_node(&store, node("repo-a", NodeKind::Symbol, Some("a.rs"), Some("foo"))).unwrap();

        store.link_task(task_id, symbol_id, "affects").unwrap();
        store.link_task(task_id, symbol_id, "affects").unwrap();
        store.link_task(other_task_id, symbol_id, "affects").unwrap();

        let links = store.task_links(task_id).unwrap();
        assert_eq!(links.len(), 1, "re-linking the same pair must not duplicate: {links:?}");
        assert_eq!(links[0].node_id, symbol_id);
    }
}

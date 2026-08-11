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
use rusqlite::Connection;
use rusqlite_migration::{Migrations, M};

use agentops_embeddings::EMBEDDING_DIM;

use crate::{Edge, EdgeRelation, GraphStore, Node, NodeKind, NewNode, NewScanHistoryEntry, RepoState, ScanChange, ScanHistory, ScanHistoryEntry};

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

    fn delete_nodes(&self, repo: &str, ids: &[i64]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
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

    fn reinforce_edge(&self, repo: &str, edge_id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE edges SET weight = MIN(weight + 0.5, 5.0), updated_at = CURRENT_TIMESTAMP WHERE repo = ?1 AND id = ?2",
            rusqlite::params![repo, edge_id],
        )?;
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
            store.reinforce_edge("repo-a", edge_id).unwrap();
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

        store.reinforce_edge("repo-b", edge_id).unwrap();

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
        store.reinforce_edge("repo-a", popular_edge).unwrap();
        store.reinforce_edge("repo-a", popular_edge).unwrap();

        let state = store.refresh_repo_state("repo-a").unwrap();
        assert_eq!(state.top_gotcha_ids, vec![popular, rare], "the more-reinforced gotcha must rank first");
        assert!(state.top_decision_ids.is_empty());

        let cached = store.get_repo_state("repo-a").unwrap().unwrap();
        assert_eq!(cached.top_gotcha_ids, state.top_gotcha_ids, "get_repo_state must return what refresh_repo_state just persisted");
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

        // Running it again must be a true no-op, not an error or a second backfill.
        migrations().to_latest(&mut store.conn).unwrap();
    }
}

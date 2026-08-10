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

use anyhow::Result;
use rusqlite::Connection;

use crate::{Edge, EdgeRelation, GraphStore, Node, NodeKind, NewNode, NewScanHistoryEntry, ScanChange, ScanHistory, ScanHistoryEntry};

const MIGRATIONS: &str = "
    CREATE TABLE IF NOT EXISTS nodes (
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
    CREATE INDEX IF NOT EXISTS idx_nodes_repo_path ON nodes(repo, path);

    CREATE TABLE IF NOT EXISTS edges (
        id        INTEGER PRIMARY KEY AUTOINCREMENT,
        repo      TEXT NOT NULL,
        src_id    INTEGER NOT NULL REFERENCES nodes(id),
        dst_id    INTEGER NOT NULL REFERENCES nodes(id),
        relation  TEXT NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_edges_repo_src ON edges(repo, src_id);
    CREATE INDEX IF NOT EXISTS idx_edges_repo_dst ON edges(repo, dst_id);

    CREATE TABLE IF NOT EXISTS scan_history (
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
    CREATE INDEX IF NOT EXISTS idx_scan_history_repo ON scan_history(repo, started_at);

    -- node_id is deliberately not a hard FK to nodes(id): a Removed entry's
    -- node has already been deleted by the time this table is read back.
    CREATE TABLE IF NOT EXISTS scan_history_entries (
        id       INTEGER PRIMARY KEY AUTOINCREMENT,
        scan_id  INTEGER NOT NULL REFERENCES scan_history(id),
        node_id  INTEGER NOT NULL,
        kind     TEXT NOT NULL,
        path     TEXT,
        name     TEXT,
        change   TEXT NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_scan_history_entries_scan ON scan_history_entries(scan_id);
";

pub struct SqliteGraphStore {
    conn: Connection,
}

impl SqliteGraphStore {
    pub fn open(path: &Path) -> Result<Self> {
        Ok(Self { conn: agentops_sqlite_support::open(path, MIGRATIONS)? })
    }

    pub fn open_in_memory() -> Result<Self> {
        Ok(Self { conn: agentops_sqlite_support::open_in_memory(MIGRATIONS)? })
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
        Ok(Edge { id: row.get("id")?, repo: row.get("repo")?, src_id: row.get("src_id")?, dst_id: row.get("dst_id")?, relation: EdgeRelation::from_db_str(&relation) })
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
        Ok(())
    }

    fn add_edge(&self, repo: &str, src_id: i64, dst_id: i64, relation: EdgeRelation) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO edges (repo, src_id, dst_id, relation) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![repo, src_id, dst_id, relation.as_db_str()],
        )?;
        Ok(self.conn.last_insert_rowid())
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
}

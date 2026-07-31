//! The "neuron" graph store — nodes (symbols, files, gotchas, decisions) connected
//! by edges (depends_on, documents, affects).
//!
//! `SqliteGraphStore` is the light-tier implementation (single file, no server).
//! `agentops-graph-pg` (in `agentops-heavy/`, commercially licensed) implements
//! the same `GraphStore` trait against Postgres for the heavy tier — see the
//! plan's §"The neuron model".

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeKind {
    Symbol,
    File,
    Gotcha,
    Decision,
}

impl NodeKind {
    /// The string this `NodeKind` is stored as — `pub` so other `GraphStore`
    /// implementations (e.g. `agentops-graph-pg`) can reuse the exact same
    /// mapping instead of re-deriving it.
    pub fn as_str(&self) -> &'static str {
        match self {
            NodeKind::Symbol => "symbol",
            NodeKind::File => "file",
            NodeKind::Gotcha => "gotcha",
            NodeKind::Decision => "decision",
        }
    }

    pub fn from_str(s: &str) -> Result<Self> {
        match s {
            "symbol" => Ok(NodeKind::Symbol),
            "file" => Ok(NodeKind::File),
            "gotcha" => Ok(NodeKind::Gotcha),
            "decision" => Ok(NodeKind::Decision),
            other => anyhow::bail!("unknown node kind: {other}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EdgeRelation {
    DependsOn,
    Documents,
    Affects,
}

impl EdgeRelation {
    pub fn as_str(&self) -> &'static str {
        match self {
            EdgeRelation::DependsOn => "depends_on",
            EdgeRelation::Documents => "documents",
            EdgeRelation::Affects => "affects",
        }
    }

    pub fn from_str(s: &str) -> Result<Self> {
        match s {
            "depends_on" => Ok(EdgeRelation::DependsOn),
            "documents" => Ok(EdgeRelation::Documents),
            "affects" => Ok(EdgeRelation::Affects),
            other => anyhow::bail!("unknown edge relation: {other}"),
        }
    }
}

/// A new node to insert — `id` is assigned by the store.
#[derive(Debug, Clone)]
pub struct NewNode {
    pub kind: NodeKind,
    pub repo: String,
    pub path: Option<String>,
    pub name: Option<String>,
    pub start_line: Option<i64>,
    pub end_line: Option<i64>,
    /// Raw content for this node (file header, symbol source, gotcha text, ...).
    /// Callers are expected to have already run this through the
    /// `agentops-security` redaction gate before it reaches the store.
    pub content: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub id: i64,
    pub kind: NodeKind,
    pub repo: String,
    pub path: Option<String>,
    pub name: Option<String>,
    pub start_line: Option<i64>,
    pub end_line: Option<i64>,
    pub content: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    pub id: i64,
    pub src_id: i64,
    pub dst_id: i64,
    pub relation: EdgeRelation,
}

/// Storage backend for the neuron graph. `SqliteGraphStore` is the light-tier
/// implementation; `agentops-graph-pg` (Postgres-backed) is the heavy tier.
///
/// Deliberately no `Send`/`Sync` supertrait bound here: `&dyn GraphStore`
/// needs `Sync` (not `Send`) to be usable across an `.await` point, but
/// `SqliteGraphStore` wraps `rusqlite::Connection`, which is intentionally
/// `!Sync` upstream (a SQLite connection isn't safe for concurrent access
/// from multiple threads without external synchronization) — adding that
/// bound here would make `SqliteGraphStore` stop implementing this trait.
/// The correct rule for callers: never hold a `&dyn GraphStore` across an
/// `.await` — do the synchronous graph-reading work in a plain (non-async)
/// function first, get owned data back, then go async. See
/// `agentops-embeddings::index_graph_store`/`collect_index_items` for the
/// pattern this protects against getting wrong again.
pub trait GraphStore {
    fn add_node(&self, node: NewNode) -> Result<i64>;
    fn add_edge(&self, src_id: i64, dst_id: i64, relation: EdgeRelation) -> Result<i64>;
    fn get_node(&self, id: i64) -> Result<Option<Node>>;
    fn nodes_by_kind(&self, kind: NodeKind) -> Result<Vec<Node>>;
    fn edges_from(&self, src_id: i64) -> Result<Vec<Edge>>;
    fn edges_to(&self, dst_id: i64) -> Result<Vec<Edge>>;
    fn all_nodes(&self) -> Result<Vec<Node>>;
    fn all_edges(&self) -> Result<Vec<Edge>>;

    /// Finds a node by its natural key — `(repo, kind, path, name)` — rather
    /// than its id. The primitive `upsert_node`/rescanning is built on: a
    /// rescan needs to recognize "this is the same file/symbol as last
    /// time," not just insert blindly.
    fn find_node(&self, repo: &str, kind: NodeKind, path: Option<&str>, name: Option<&str>) -> Result<Option<Node>>;

    /// Updates an existing node's line range and content in place —
    /// deliberately id-preserving, so any edge pointing at this node (e.g. a
    /// gotcha's `Affects` edge) stays valid across a rescan instead of
    /// dangling.
    fn update_node(&self, id: i64, start_line: Option<i64>, end_line: Option<i64>, content: Option<String>) -> Result<()>;

    /// Deletes the given nodes and any edges touching them (as either
    /// endpoint) — used to prune nodes from a prior scan that no longer
    /// exist in the current one (a removed file, a renamed/deleted symbol).
    fn delete_nodes(&self, ids: &[i64]) -> Result<()>;
}

/// Inserts `node`, or — if a node with the same `(repo, kind, path, name)`
/// already exists — updates that node's content/line-range in place and
/// returns its existing id. This is what rescanning a repo should call
/// instead of `add_node` directly: a naive `add_node` on every scan
/// duplicates every file/symbol node once per rescan (each rescan adds a
/// fresh copy without removing the stale one), which both bloats the store
/// and — for anything embedding node content for semantic search — bloats
/// and skews retrieval with near-duplicate near-identical points. Built on
/// the trait's primitives so every `GraphStore` implementation gets this
/// behavior for free, rather than each one re-implementing upsert logic.
pub fn upsert_node(store: &dyn GraphStore, node: NewNode) -> Result<i64> {
    match store.find_node(&node.repo, node.kind, node.path.as_deref(), node.name.as_deref())? {
        Some(existing) => {
            store.update_node(existing.id, node.start_line, node.end_line, node.content)?;
            Ok(existing.id)
        }
        None => store.add_node(node),
    }
}

/// Deletes every node of `kind` in `repo` whose id is not in `keep_ids` —
/// call this after upserting a scan's files/symbols with the full set of
/// ids that scan touched, to prune whatever's left over from a prior scan
/// (a file that was deleted, a symbol that was renamed or removed).
/// Returns how many nodes were pruned.
pub fn prune_stale_nodes(store: &dyn GraphStore, repo: &str, kind: NodeKind, keep_ids: &[i64]) -> Result<usize> {
    let stale: Vec<i64> = store
        .nodes_by_kind(kind)?
        .into_iter()
        .filter(|n| n.repo == repo && !keep_ids.contains(&n.id))
        .map(|n| n.id)
        .collect();
    let count = stale.len();
    if !stale.is_empty() {
        store.delete_nodes(&stale)?;
    }
    Ok(count)
}

/// Embedded, single-file graph store backed by SQLite — the light-tier `GraphStore`.
pub struct SqliteGraphStore {
    conn: Connection,
}

impl SqliteGraphStore {
    /// Opens (or creates) the graph store at `path`, e.g. `.context/graph.db`.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating parent dir for {}", path.display()))?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("opening graph store at {}", path.display()))?;
        Self::migrate(&conn)?;
        Ok(Self { conn })
    }

    /// In-memory store, useful for tests.
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::migrate(&conn)?;
        Ok(Self { conn })
    }

    fn migrate(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS nodes (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                kind        TEXT NOT NULL,
                repo        TEXT NOT NULL,
                path        TEXT,
                name        TEXT,
                start_line  INTEGER,
                end_line    INTEGER,
                content     TEXT,
                created_at  TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );
            CREATE INDEX IF NOT EXISTS idx_nodes_kind ON nodes(kind);
            CREATE INDEX IF NOT EXISTS idx_nodes_repo_path ON nodes(repo, path);

            CREATE TABLE IF NOT EXISTS edges (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                src_id      INTEGER NOT NULL REFERENCES nodes(id),
                dst_id      INTEGER NOT NULL REFERENCES nodes(id),
                relation    TEXT NOT NULL,
                created_at  TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );
            CREATE INDEX IF NOT EXISTS idx_edges_src ON edges(src_id);
            CREATE INDEX IF NOT EXISTS idx_edges_dst ON edges(dst_id);
            ",
        )?;
        Ok(())
    }

    fn row_to_node(row: &rusqlite::Row) -> rusqlite::Result<Node> {
        let kind_str: String = row.get("kind")?;
        Ok(Node {
            id: row.get("id")?,
            kind: NodeKind::from_str(&kind_str)
                .map_err(|e| rusqlite::Error::InvalidColumnType(0, e.to_string(), rusqlite::types::Type::Text))?,
            repo: row.get("repo")?,
            path: row.get("path")?,
            name: row.get("name")?,
            start_line: row.get("start_line")?,
            end_line: row.get("end_line")?,
            content: row.get("content")?,
        })
    }

    fn row_to_edge(row: &rusqlite::Row) -> rusqlite::Result<Edge> {
        let relation_str: String = row.get("relation")?;
        Ok(Edge {
            id: row.get("id")?,
            src_id: row.get("src_id")?,
            dst_id: row.get("dst_id")?,
            relation: EdgeRelation::from_str(&relation_str)
                .map_err(|e| rusqlite::Error::InvalidColumnType(0, e.to_string(), rusqlite::types::Type::Text))?,
        })
    }
}

impl GraphStore for SqliteGraphStore {
    fn add_node(&self, node: NewNode) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO nodes (kind, repo, path, name, start_line, end_line, content)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                node.kind.as_str(),
                node.repo,
                node.path,
                node.name,
                node.start_line,
                node.end_line,
                node.content,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    fn add_edge(&self, src_id: i64, dst_id: i64, relation: EdgeRelation) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO edges (src_id, dst_id, relation) VALUES (?1, ?2, ?3)",
            rusqlite::params![src_id, dst_id, relation.as_str()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    fn get_node(&self, id: i64) -> Result<Option<Node>> {
        let mut stmt = self.conn.prepare("SELECT * FROM nodes WHERE id = ?1")?;
        let mut rows = stmt.query_map([id], Self::row_to_node)?;
        Ok(rows.next().transpose()?)
    }

    fn nodes_by_kind(&self, kind: NodeKind) -> Result<Vec<Node>> {
        let mut stmt = self.conn.prepare("SELECT * FROM nodes WHERE kind = ?1")?;
        let rows = stmt.query_map([kind.as_str()], Self::row_to_node)?;
        rows.map(|r| r.map_err(anyhow::Error::from)).collect()
    }

    fn edges_from(&self, src_id: i64) -> Result<Vec<Edge>> {
        let mut stmt = self.conn.prepare("SELECT * FROM edges WHERE src_id = ?1")?;
        let rows = stmt.query_map([src_id], Self::row_to_edge)?;
        rows.map(|r| r.map_err(anyhow::Error::from)).collect()
    }

    fn edges_to(&self, dst_id: i64) -> Result<Vec<Edge>> {
        let mut stmt = self.conn.prepare("SELECT * FROM edges WHERE dst_id = ?1")?;
        let rows = stmt.query_map([dst_id], Self::row_to_edge)?;
        rows.map(|r| r.map_err(anyhow::Error::from)).collect()
    }

    fn all_nodes(&self) -> Result<Vec<Node>> {
        let mut stmt = self.conn.prepare("SELECT * FROM nodes")?;
        let rows = stmt.query_map([], Self::row_to_node)?;
        rows.map(|r| r.map_err(anyhow::Error::from)).collect()
    }

    fn all_edges(&self) -> Result<Vec<Edge>> {
        let mut stmt = self.conn.prepare("SELECT * FROM edges")?;
        let rows = stmt.query_map([], Self::row_to_edge)?;
        rows.map(|r| r.map_err(anyhow::Error::from)).collect()
    }

    fn find_node(&self, repo: &str, kind: NodeKind, path: Option<&str>, name: Option<&str>) -> Result<Option<Node>> {
        // `IS` (not `=`) so a NULL path/name matches NULL rather than never
        // matching at all, per SQL's usual NULL-comparison semantics.
        let mut stmt = self.conn.prepare("SELECT * FROM nodes WHERE repo = ?1 AND kind = ?2 AND path IS ?3 AND name IS ?4")?;
        let mut rows = stmt.query_map(rusqlite::params![repo, kind.as_str(), path, name], Self::row_to_node)?;
        Ok(rows.next().transpose()?)
    }

    fn update_node(&self, id: i64, start_line: Option<i64>, end_line: Option<i64>, content: Option<String>) -> Result<()> {
        self.conn.execute(
            "UPDATE nodes SET start_line = ?1, end_line = ?2, content = ?3 WHERE id = ?4",
            rusqlite::params![start_line, end_line, content, id],
        )?;
        Ok(())
    }

    fn delete_nodes(&self, ids: &[i64]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let params: Vec<&dyn rusqlite::ToSql> = ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
        // Two separate IN-clauses (src_id, dst_id) each need their own copy
        // of the bound params — reusing `params` once wouldn't cover both.
        let doubled_params: Vec<&dyn rusqlite::ToSql> = params.iter().copied().chain(params.iter().copied()).collect();
        self.conn.execute(
            &format!("DELETE FROM edges WHERE src_id IN ({placeholders}) OR dst_id IN ({placeholders})"),
            doubled_params.as_slice(),
        )?;
        self.conn.execute(&format!("DELETE FROM nodes WHERE id IN ({placeholders})"), params.as_slice())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_query_node() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let id = store
            .add_node(NewNode {
                kind: NodeKind::Symbol,
                repo: "demo".into(),
                path: Some("src/lib.rs".into()),
                name: Some("do_thing".into()),
                start_line: Some(1),
                end_line: Some(10),
                content: Some("fn do_thing() {}".into()),
            })
            .unwrap();

        let node = store.get_node(id).unwrap().expect("node exists");
        assert_eq!(node.name.as_deref(), Some("do_thing"));
        assert_eq!(node.kind, NodeKind::Symbol);
    }

    #[test]
    fn gotcha_node_connects_to_symbol_via_affects_edge() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let symbol_id = store
            .add_node(NewNode {
                kind: NodeKind::Symbol,
                repo: "demo".into(),
                path: Some("src/auth.rs".into()),
                name: Some("verify_token".into()),
                start_line: Some(5),
                end_line: Some(20),
                content: Some("fn verify_token() {}".into()),
            })
            .unwrap();

        let gotcha_id = store
            .add_node(NewNode {
                kind: NodeKind::Gotcha,
                repo: "demo".into(),
                path: None,
                name: Some("token-expiry-off-by-one".into()),
                start_line: None,
                end_line: None,
                content: Some("Token expiry check was off by one day; fixed in commit abc123.".into()),
            })
            .unwrap();

        store.add_edge(gotcha_id, symbol_id, EdgeRelation::Affects).unwrap();

        // The gotcha should surface when querying edges pointing at the symbol.
        let incoming = store.edges_to(symbol_id).unwrap();
        assert_eq!(incoming.len(), 1);
        assert_eq!(incoming[0].src_id, gotcha_id);
        assert_eq!(incoming[0].relation, EdgeRelation::Affects);

        let gotchas = store.nodes_by_kind(NodeKind::Gotcha).unwrap();
        assert_eq!(gotchas.len(), 1);
        assert_eq!(gotchas[0].id, gotcha_id);
    }

    #[test]
    fn store_persists_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join(".context").join("graph.db");

        {
            let store = SqliteGraphStore::open(&db_path).unwrap();
            store
                .add_node(NewNode {
                    kind: NodeKind::File,
                    repo: "demo".into(),
                    path: Some("README.md".into()),
                    name: None,
                    start_line: None,
                    end_line: None,
                    content: Some("# demo".into()),
                })
                .unwrap();
        }

        let store = SqliteGraphStore::open(&db_path).unwrap();
        let files = store.nodes_by_kind(NodeKind::File).unwrap();
        assert_eq!(files.len(), 1);
    }

    fn symbol_node(repo: &str, path: &str, name: &str, content: &str) -> NewNode {
        NewNode { kind: NodeKind::Symbol, repo: repo.into(), path: Some(path.into()), name: Some(name.into()), start_line: Some(1), end_line: Some(2), content: Some(content.into()) }
    }

    #[test]
    fn upserting_the_same_symbol_twice_updates_in_place_instead_of_duplicating() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let id1 = upsert_node(&store, symbol_node("demo", "src/lib.rs", "do_thing", "fn do_thing() { 1 }")).unwrap();
        let id2 = upsert_node(&store, symbol_node("demo", "src/lib.rs", "do_thing", "fn do_thing() { 2 }")).unwrap();

        assert_eq!(id1, id2, "rescanning the same symbol must reuse its id, not create a new one");
        let symbols = store.nodes_by_kind(NodeKind::Symbol).unwrap();
        assert_eq!(symbols.len(), 1, "must not duplicate the node on a second upsert");
        assert!(symbols[0].content.as_deref().unwrap().contains('2'), "content must be updated to the latest scan");
    }

    #[test]
    fn upsert_preserves_id_so_existing_gotcha_edges_survive_a_rescan() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let symbol_id = upsert_node(&store, symbol_node("demo", "src/auth.rs", "verify_token", "fn verify_token() {}")).unwrap();
        let gotcha_id = store
            .add_node(NewNode { kind: NodeKind::Gotcha, repo: "demo".into(), path: None, name: Some("g".into()), start_line: None, end_line: None, content: Some("gotcha text".into()) })
            .unwrap();
        store.add_edge(gotcha_id, symbol_id, EdgeRelation::Affects).unwrap();

        // Simulate a rescan of the same, unchanged symbol.
        let rescanned_id = upsert_node(&store, symbol_node("demo", "src/auth.rs", "verify_token", "fn verify_token() {}")).unwrap();
        assert_eq!(rescanned_id, symbol_id);

        let incoming = store.edges_to(symbol_id).unwrap();
        assert_eq!(incoming.len(), 1, "the gotcha's edge must still resolve after a rescan");
        assert_eq!(incoming[0].src_id, gotcha_id);
    }

    #[test]
    fn prune_stale_nodes_removes_symbols_missing_from_the_latest_scan() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let kept_id = upsert_node(&store, symbol_node("demo", "src/lib.rs", "kept_fn", "..")).unwrap();
        let removed_id = upsert_node(&store, symbol_node("demo", "src/lib.rs", "removed_fn", "..")).unwrap();

        // This scan only found `kept_fn` — `removed_fn` must have been deleted from the source.
        let pruned = prune_stale_nodes(&store, "demo", NodeKind::Symbol, &[kept_id]).unwrap();

        assert_eq!(pruned, 1);
        assert!(store.get_node(kept_id).unwrap().is_some());
        assert!(store.get_node(removed_id).unwrap().is_none());
    }

    #[test]
    fn prune_stale_nodes_also_removes_edges_touching_the_pruned_node() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let symbol_id = upsert_node(&store, symbol_node("demo", "src/lib.rs", "removed_fn", "..")).unwrap();
        let gotcha_id = store
            .add_node(NewNode { kind: NodeKind::Gotcha, repo: "demo".into(), path: None, name: Some("g".into()), start_line: None, end_line: None, content: Some("text".into()) })
            .unwrap();
        store.add_edge(gotcha_id, symbol_id, EdgeRelation::Affects).unwrap();

        prune_stale_nodes(&store, "demo", NodeKind::Symbol, &[]).unwrap();

        assert!(store.edges_to(symbol_id).unwrap().is_empty(), "dangling edge to a pruned node must be cleaned up too");
    }

    #[test]
    fn prune_stale_nodes_never_touches_a_different_repo() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let other_repo_id = upsert_node(&store, symbol_node("other-repo", "src/lib.rs", "fn_a", "..")).unwrap();

        prune_stale_nodes(&store, "demo", NodeKind::Symbol, &[]).unwrap();

        assert!(store.get_node(other_repo_id).unwrap().is_some(), "pruning one repo must never delete another repo's nodes");
    }
}

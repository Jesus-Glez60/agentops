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
pub trait GraphStore {
    fn add_node(&self, node: NewNode) -> Result<i64>;
    fn add_edge(&self, src_id: i64, dst_id: i64, relation: EdgeRelation) -> Result<i64>;
    fn get_node(&self, id: i64) -> Result<Option<Node>>;
    fn nodes_by_kind(&self, kind: NodeKind) -> Result<Vec<Node>>;
    fn edges_from(&self, src_id: i64) -> Result<Vec<Edge>>;
    fn edges_to(&self, dst_id: i64) -> Result<Vec<Edge>>;
    fn all_nodes(&self) -> Result<Vec<Node>>;
    fn all_edges(&self) -> Result<Vec<Edge>>;
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
}

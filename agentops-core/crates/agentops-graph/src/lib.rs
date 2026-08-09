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
    /// An LLM-generated explanation of what a symbol does, connected via
    /// `EdgeRelation::Documents` (not `Affects` — a `Definition` documents
    /// code, generated *from* the code itself, which is a different kind of
    /// claim than a human-authored `Gotcha`/`Decision`). See `agentops-llm`.
    ///
    /// NOTE for implementers adding a `NodeKind` variant: this enum's own
    /// `as_str`/`from_str` are the only truly exhaustive match on `NodeKind`
    /// in the codebase — everywhere else (agentops-docgen, the `status`
    /// count in agentops-api/agentops-cli/agentops-mcp, and
    /// agentops-heavy's agentops-embeddings::collect_index_items, a
    /// separate commercial workspace) enumerates kinds positively, so a new
    /// variant is silently invisible there until each is updated by hand.
    Definition,
    /// A vault/notes-folder entry that isn't a gotcha or decision (frontmatter
    /// `type: knowledge`/`type: context`, or untyped) — ingested via
    /// `agentops-notes::ingest_vault`. Reuses `Gotcha`/`Decision` directly for
    /// those two frontmatter types, since that's exactly what those kinds
    /// already mean; this variant is only for the ones that don't map to an
    /// existing kind.
    Note,
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
            NodeKind::Definition => "definition",
            NodeKind::Note => "note",
        }
    }

    pub fn from_str(s: &str) -> Result<Self> {
        match s {
            "symbol" => Ok(NodeKind::Symbol),
            "file" => Ok(NodeKind::File),
            "gotcha" => Ok(NodeKind::Gotcha),
            "decision" => Ok(NodeKind::Decision),
            "definition" => Ok(NodeKind::Definition),
            "note" => Ok(NodeKind::Note),
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

/// One scan's summary — the persisted, queryable counterpart to
/// `agentops-mcp`'s `ScanPersistSummary` (which stays synchronous/ephemeral
/// for callers that just want a printout). `node_id` on
/// `ScanHistoryEntry` is deliberately NOT a foreign key to `nodes(id)`: a
/// `removed` entry's node has already been deleted by the time this row is
/// read back, so a real FK constraint would make removed-node history
/// unreadable (or block the delete) the moment it's needed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanHistoryRow {
    pub id: i64,
    pub repo: String,
    pub started_at: String,
    pub finished_at: String,
    pub git_sha: Option<String>,
    pub files_added: i64,
    pub files_changed: i64,
    pub files_removed: i64,
    pub symbols_added: i64,
    pub symbols_changed: i64,
    pub symbols_removed: i64,
    pub notes_added: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScanEntryKind {
    File,
    Symbol,
}

impl ScanEntryKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ScanEntryKind::File => "file",
            ScanEntryKind::Symbol => "symbol",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScanChange {
    Added,
    Changed,
    Removed,
}

impl ScanChange {
    pub fn as_str(&self) -> &'static str {
        match self {
            ScanChange::Added => "added",
            ScanChange::Changed => "changed",
            ScanChange::Removed => "removed",
        }
    }
}

/// A single added/changed/removed file or symbol, recorded against a scan —
/// what `persist()` builds up as it classifies each node during a rescan.
#[derive(Debug, Clone)]
pub struct NewScanHistoryEntry {
    pub node_id: i64,
    pub kind: ScanEntryKind,
    pub path: Option<String>,
    pub name: Option<String>,
    pub change: ScanChange,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanHistoryEntry {
    pub id: i64,
    pub scan_id: i64,
    pub node_id: i64,
    pub kind: String,
    pub path: Option<String>,
    pub name: Option<String>,
    pub change: String,
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
/// `agentops-embeddings::collect_index_items` for the pattern this protects
/// against getting wrong again.
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

    /// Deletes every `relation` edge originating from `src_id` — used to
    /// replace a node's outgoing edges of one kind wholesale on a rescan
    /// (e.g. a file's `DependsOn` edges: its dependency set can change
    /// without the file node itself changing id, so re-adding fresh edges
    /// without first clearing the old ones would accumulate stale/wrong
    /// ones forever, the same class of bug the node-upsert path already
    /// guards against).
    fn delete_edges_from(&self, src_id: i64, relation: EdgeRelation) -> Result<()>;
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
/// Returns the pruned nodes themselves (not just a count) — callers building
/// a scan-to-scan changelog need to know *which* files/symbols were removed,
/// not just how many; deleting first and returning identity after would lose
/// that, so the full `Node` is captured before `delete_nodes` runs.
pub fn prune_stale_nodes(store: &dyn GraphStore, repo: &str, kind: NodeKind, keep_ids: &[i64]) -> Result<Vec<Node>> {
    let stale: Vec<Node> = store
        .nodes_by_kind(kind)?
        .into_iter()
        .filter(|n| n.repo == repo && !keep_ids.contains(&n.id))
        .collect();
    if !stale.is_empty() {
        let ids: Vec<i64> = stale.iter().map(|n| n.id).collect();
        store.delete_nodes(&ids)?;
    }
    Ok(stale)
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

            CREATE TABLE IF NOT EXISTS scan_history (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                repo            TEXT NOT NULL,
                started_at      TEXT NOT NULL,
                finished_at     TEXT NOT NULL,
                git_sha         TEXT,
                files_added     INTEGER NOT NULL,
                files_changed   INTEGER NOT NULL,
                files_removed   INTEGER NOT NULL,
                symbols_added   INTEGER NOT NULL,
                symbols_changed INTEGER NOT NULL,
                symbols_removed INTEGER NOT NULL,
                notes_added     INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS idx_scan_history_repo ON scan_history(repo);

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

    fn row_to_scan_history_row(row: &rusqlite::Row) -> rusqlite::Result<ScanHistoryRow> {
        Ok(ScanHistoryRow {
            id: row.get("id")?,
            repo: row.get("repo")?,
            started_at: row.get("started_at")?,
            finished_at: row.get("finished_at")?,
            git_sha: row.get("git_sha")?,
            files_added: row.get("files_added")?,
            files_changed: row.get("files_changed")?,
            files_removed: row.get("files_removed")?,
            symbols_added: row.get("symbols_added")?,
            symbols_changed: row.get("symbols_changed")?,
            symbols_removed: row.get("symbols_removed")?,
            notes_added: row.get("notes_added")?,
        })
    }

    fn row_to_scan_history_entry(row: &rusqlite::Row) -> rusqlite::Result<ScanHistoryEntry> {
        Ok(ScanHistoryEntry {
            id: row.get("id")?,
            scan_id: row.get("scan_id")?,
            node_id: row.get("node_id")?,
            kind: row.get("kind")?,
            path: row.get("path")?,
            name: row.get("name")?,
            change: row.get("change")?,
        })
    }

    /// Records one completed scan: a `scan_history` summary row (counts
    /// derived from `entries`) plus one `scan_history_entries` row per
    /// added/changed/removed file or symbol. Returns the new scan's id.
    #[allow(clippy::too_many_arguments)]
    pub fn record_scan(
        &self,
        repo: &str,
        started_at: &str,
        finished_at: &str,
        git_sha: Option<&str>,
        entries: &[NewScanHistoryEntry],
        notes_added: i64,
    ) -> Result<i64> {
        let count = |kind: ScanEntryKind, change: ScanChange| {
            entries.iter().filter(|e| e.kind == kind && e.change == change).count() as i64
        };
        self.conn.execute(
            "INSERT INTO scan_history (repo, started_at, finished_at, git_sha,
                files_added, files_changed, files_removed,
                symbols_added, symbols_changed, symbols_removed, notes_added)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            rusqlite::params![
                repo,
                started_at,
                finished_at,
                git_sha,
                count(ScanEntryKind::File, ScanChange::Added),
                count(ScanEntryKind::File, ScanChange::Changed),
                count(ScanEntryKind::File, ScanChange::Removed),
                count(ScanEntryKind::Symbol, ScanChange::Added),
                count(ScanEntryKind::Symbol, ScanChange::Changed),
                count(ScanEntryKind::Symbol, ScanChange::Removed),
                notes_added,
            ],
        )?;
        let scan_id = self.conn.last_insert_rowid();

        for entry in entries {
            self.conn.execute(
                "INSERT INTO scan_history_entries (scan_id, node_id, kind, path, name, change)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![scan_id, entry.node_id, entry.kind.as_str(), entry.path, entry.name, entry.change.as_str()],
            )?;
        }
        Ok(scan_id)
    }

    /// The most recently recorded scan for `repo`, if any.
    pub fn latest_scan(&self, repo: &str) -> Result<Option<ScanHistoryRow>> {
        let mut stmt = self.conn.prepare("SELECT * FROM scan_history WHERE repo = ?1 ORDER BY id DESC LIMIT 1")?;
        let mut rows = stmt.query_map([repo], Self::row_to_scan_history_row)?;
        Ok(rows.next().transpose()?)
    }

    /// A specific scan's summary row by id.
    pub fn get_scan(&self, scan_id: i64) -> Result<Option<ScanHistoryRow>> {
        let mut stmt = self.conn.prepare("SELECT * FROM scan_history WHERE id = ?1")?;
        let mut rows = stmt.query_map([scan_id], Self::row_to_scan_history_row)?;
        Ok(rows.next().transpose()?)
    }

    /// Every added/changed/removed file/symbol entry recorded for `scan_id`.
    pub fn scan_diff(&self, scan_id: i64) -> Result<Vec<ScanHistoryEntry>> {
        let mut stmt = self.conn.prepare("SELECT * FROM scan_history_entries WHERE scan_id = ?1 ORDER BY id")?;
        let rows = stmt.query_map([scan_id], Self::row_to_scan_history_entry)?;
        rows.map(|r| r.map_err(anyhow::Error::from)).collect()
    }

    /// Up to `limit` most recent scans for `repo`, most recent first.
    pub fn list_scans(&self, repo: &str, limit: i64) -> Result<Vec<ScanHistoryRow>> {
        let mut stmt = self.conn.prepare("SELECT * FROM scan_history WHERE repo = ?1 ORDER BY id DESC LIMIT ?2")?;
        let rows = stmt.query_map(rusqlite::params![repo, limit], Self::row_to_scan_history_row)?;
        rows.map(|r| r.map_err(anyhow::Error::from)).collect()
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

    fn delete_edges_from(&self, src_id: i64, relation: EdgeRelation) -> Result<()> {
        self.conn.execute("DELETE FROM edges WHERE src_id = ?1 AND relation = ?2", rusqlite::params![src_id, relation.as_str()])?;
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

        assert_eq!(pruned.len(), 1);
        assert_eq!(pruned[0].id, removed_id, "the returned identity must be the pruned node, not the kept one");
        assert_eq!(pruned[0].name.as_deref(), Some("removed_fn"));
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

    #[test]
    fn delete_edges_from_removes_only_the_matching_relation_from_that_source() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let a = upsert_node(&store, symbol_node("demo", "a.rs", "a", "..")).unwrap();
        let b = upsert_node(&store, symbol_node("demo", "b.rs", "b", "..")).unwrap();
        let c = upsert_node(&store, symbol_node("demo", "c.rs", "c", "..")).unwrap();

        store.add_edge(a, b, EdgeRelation::DependsOn).unwrap();
        store.add_edge(a, c, EdgeRelation::Affects).unwrap(); // different relation, same src — must survive
        store.add_edge(c, a, EdgeRelation::DependsOn).unwrap(); // different src, same relation — must survive

        store.delete_edges_from(a, EdgeRelation::DependsOn).unwrap();

        let remaining_from_a = store.edges_from(a).unwrap();
        assert_eq!(remaining_from_a.len(), 1);
        assert_eq!(remaining_from_a[0].relation, EdgeRelation::Affects);
        assert_eq!(store.edges_from(c).unwrap().len(), 1, "a different source's edges of the same relation must be untouched");
    }

    #[test]
    fn record_scan_derives_counts_from_the_entries_it_was_given() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let entries = vec![
            NewScanHistoryEntry { node_id: 1, kind: ScanEntryKind::File, path: Some("a.rs".into()), name: None, change: ScanChange::Added },
            NewScanHistoryEntry { node_id: 2, kind: ScanEntryKind::Symbol, path: Some("a.rs".into()), name: Some("f".into()), change: ScanChange::Added },
            NewScanHistoryEntry { node_id: 3, kind: ScanEntryKind::Symbol, path: Some("b.rs".into()), name: Some("g".into()), change: ScanChange::Changed },
            NewScanHistoryEntry { node_id: 4, kind: ScanEntryKind::File, path: Some("c.rs".into()), name: None, change: ScanChange::Removed },
        ];
        let scan_id = store.record_scan("demo", "2026-08-04T00:00:00Z", "2026-08-04T00:00:01Z", Some("abc123"), &entries, 2).unwrap();

        let row = store.latest_scan("demo").unwrap().expect("scan was just recorded");
        assert_eq!(row.id, scan_id);
        assert_eq!(row.git_sha.as_deref(), Some("abc123"));
        assert_eq!(row.files_added, 1);
        assert_eq!(row.files_removed, 1);
        assert_eq!(row.symbols_added, 1);
        assert_eq!(row.symbols_changed, 1);
        assert_eq!(row.notes_added, 2);

        let diff = store.scan_diff(scan_id).unwrap();
        assert_eq!(diff.len(), 4, "scan_diff must return the identity-level entries, not just the summary counts");
        assert!(diff.iter().any(|e| e.name.as_deref() == Some("g") && e.change == "changed"));
    }

    #[test]
    fn scan_diff_survives_the_pruned_nodes_it_describes_being_deleted() {
        // scan_history_entries.node_id is deliberately not a real FK to
        // nodes(id) -- a "removed" entry's node is already gone from the
        // nodes table by the time this is read back.
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let removed_id = upsert_node(&store, symbol_node("demo", "src/lib.rs", "gone_fn", "..")).unwrap();
        store.delete_nodes(&[removed_id]).unwrap();

        let entries = vec![NewScanHistoryEntry {
            node_id: removed_id,
            kind: ScanEntryKind::Symbol,
            path: Some("src/lib.rs".into()),
            name: Some("gone_fn".into()),
            change: ScanChange::Removed,
        }];
        let scan_id = store.record_scan("demo", "t0", "t1", None, &entries, 0).unwrap();

        let diff = store.scan_diff(scan_id).unwrap();
        assert_eq!(diff.len(), 1);
        assert_eq!(diff[0].node_id, removed_id);
        assert_eq!(diff[0].change, "removed");
    }

    #[test]
    fn list_scans_returns_most_recent_first_and_respects_the_limit() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        store.record_scan("demo", "t0", "t0", None, &[], 0).unwrap();
        let second = store.record_scan("demo", "t1", "t1", None, &[], 0).unwrap();
        store.record_scan("other-repo", "t2", "t2", None, &[], 0).unwrap();

        let scans = store.list_scans("demo", 1).unwrap();
        assert_eq!(scans.len(), 1, "limit must be respected");
        assert_eq!(scans[0].id, second, "most recent scan for the repo must come first");
    }
}

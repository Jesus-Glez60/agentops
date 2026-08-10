//! Domain entities (`Node`, `Edge`, `NodeKind`, `EdgeRelation`) and the
//! `GraphStore` port (trait only — the SQLite adapter lives in `sqlite.rs`,
//! kept deliberately separate per the hexagonal architecture guide, matching
//! the pattern `docbrain-graph` already established this rebuild).
//!
//! **Repo-scoping is required on every method, no exceptions.** `main`'s
//! `GraphStore` only required `repo` on `find_node`; `get_node`,
//! `nodes_by_kind`, `edges_from`/`edges_to`, and `all_nodes`/`all_edges` were
//! global/id-based, relying on callers to filter by repo themselves (one
//! caller needed a dedicated regression test just to guard this by
//! convention). Every real deployment today opens one SQLite file per repo,
//! so this isn't defending against an active bug — it's the same reason
//! `DocbrainStore` is a trait at all: a light-tier per-repo-file adapter and
//! a hypothetical future shared/multi-tenant adapter should both be
//! implementable behind this exact port without redesigning call sites.

mod sqlite;

pub use sqlite::SqliteGraphStore;

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeKind {
    Symbol,
    File,
    Gotcha,
    Decision,
    Definition,
    Note,
}

impl NodeKind {
    fn as_db_str(&self) -> &'static str {
        match self {
            NodeKind::Symbol => "symbol",
            NodeKind::File => "file",
            NodeKind::Gotcha => "gotcha",
            NodeKind::Decision => "decision",
            NodeKind::Definition => "definition",
            NodeKind::Note => "note",
        }
    }

    fn from_db_str(s: &str) -> Self {
        match s {
            "file" => NodeKind::File,
            "gotcha" => NodeKind::Gotcha,
            "decision" => NodeKind::Decision,
            "definition" => NodeKind::Definition,
            "note" => NodeKind::Note,
            _ => NodeKind::Symbol,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EdgeRelation {
    DependsOn,
    Documents,
    /// A note (`Gotcha`/`Decision`) affects a symbol — directed **from the
    /// note to the symbol**. `agentops-docgen` queries `edges_from(note_id)`
    /// to badge a symbol with "N known gotcha(s) apply"; reversing this
    /// direction silently breaks that badging with no error anywhere.
    Affects,
}

impl EdgeRelation {
    fn as_db_str(&self) -> &'static str {
        match self {
            EdgeRelation::DependsOn => "depends_on",
            EdgeRelation::Documents => "documents",
            EdgeRelation::Affects => "affects",
        }
    }

    fn from_db_str(s: &str) -> Self {
        match s {
            "documents" => EdgeRelation::Documents,
            "affects" => EdgeRelation::Affects,
            _ => EdgeRelation::DependsOn,
        }
    }
}

/// One node: a symbol, a file, or a note (`Gotcha`/`Decision`/`Note`) —
/// `Gotcha`/`Decision` carry a title in `name` and rendered body in
/// `content` (an implicit contract `agentops-docgen` depends on).
///
/// `container` disambiguates same-named symbols within one file (e.g.
/// `impl Foo { fn new() }` and `impl Bar { fn new() }` in the same Rust
/// file, or two classes in one TS file both defining `render`) — confirmed
/// via live testing to be a real collision otherwise: `name` alone is kept
/// bare (not `Foo::new`) so `agentops-notes`' word-boundary matching against
/// freeform note text (which references bare identifiers, not qualified
/// ones) keeps working unchanged.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub id: i64,
    pub kind: NodeKind,
    pub repo: String,
    pub path: Option<String>,
    pub name: Option<String>,
    pub container: Option<String>,
    pub start_line: Option<i64>,
    pub end_line: Option<i64>,
    pub content: Option<String>,
}

/// Fields needed to add a node; `id` is assigned by the store.
#[derive(Debug, Clone)]
pub struct NewNode {
    pub kind: NodeKind,
    pub repo: String,
    pub path: Option<String>,
    pub name: Option<String>,
    pub container: Option<String>,
    pub start_line: Option<i64>,
    pub end_line: Option<i64>,
    pub content: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    pub id: i64,
    pub repo: String,
    pub src_id: i64,
    pub dst_id: i64,
    pub relation: EdgeRelation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScanChange {
    Added,
    Changed,
    Removed,
}

impl ScanChange {
    fn as_db_str(&self) -> &'static str {
        match self {
            ScanChange::Added => "added",
            ScanChange::Changed => "changed",
            ScanChange::Removed => "removed",
        }
    }

    fn from_db_str(s: &str) -> Self {
        match s {
            "changed" => ScanChange::Changed,
            "removed" => ScanChange::Removed,
            _ => ScanChange::Added,
        }
    }
}

/// One row of scan-history detail. `node_id` is deliberately not enforced
/// as a foreign key by the adapter — a `Removed` entry's node is already
/// deleted by the time this is read back; a real FK would make removed-node
/// history unreadable or block the delete outright.
#[derive(Debug, Clone)]
pub struct NewScanHistoryEntry {
    pub node_id: i64,
    pub kind: NodeKind,
    pub path: Option<String>,
    pub name: Option<String>,
    pub change: ScanChange,
}

#[derive(Debug, Clone)]
pub struct ScanHistoryEntry {
    pub id: i64,
    pub scan_id: i64,
    pub node_id: i64,
    pub kind: NodeKind,
    pub path: Option<String>,
    pub name: Option<String>,
    pub change: ScanChange,
}

/// One completed scan. Aggregate counts are derived from `entries` at
/// insert time, not stored/maintained independently.
#[derive(Debug, Clone)]
pub struct ScanHistory {
    pub id: i64,
    pub repo: String,
    pub started_at: String,
    pub files_added: i64,
    pub files_changed: i64,
    pub files_removed: i64,
    pub symbols_added: i64,
    pub symbols_changed: i64,
    pub symbols_removed: i64,
    pub notes_added: i64,
}

/// The codebase graph port. One adapter today (`SqliteGraphStore`); the
/// trait boundary is what lets a future adapter (Postgres-backed, shared
/// across repos) exist without touching any use-case or MCP-handler code.
pub trait GraphStore {
    fn add_node(&self, node: NewNode) -> Result<i64>;
    fn get_node(&self, repo: &str, id: i64) -> Result<Option<Node>>;
    fn nodes_by_kind(&self, repo: &str, kind: NodeKind) -> Result<Vec<Node>>;
    fn all_nodes(&self, repo: &str) -> Result<Vec<Node>>;
    /// Natural-key lookup — identity is `(repo, kind, path, name,
    /// container)`, `NULL` path/name/container compared with `IS` so two
    /// nodes both missing a value (e.g. two `File` nodes with no `name`)
    /// don't spuriously match each other via `=`.
    fn find_node(&self, repo: &str, kind: NodeKind, path: Option<&str>, name: Option<&str>, container: Option<&str>) -> Result<Option<Node>>;
    /// Updates in place, preserving `id` — so edges into/out of this node
    /// (e.g. a `Gotcha`'s `Affects` edge) survive a rescan.
    fn update_node(&self, repo: &str, id: i64, start_line: Option<i64>, end_line: Option<i64>, content: Option<String>) -> Result<()>;
    fn delete_nodes(&self, repo: &str, ids: &[i64]) -> Result<()>;

    fn add_edge(&self, repo: &str, src_id: i64, dst_id: i64, relation: EdgeRelation) -> Result<i64>;
    fn edges_from(&self, repo: &str, src_id: i64) -> Result<Vec<Edge>>;
    fn edges_to(&self, repo: &str, dst_id: i64) -> Result<Vec<Edge>>;
    fn all_edges(&self, repo: &str) -> Result<Vec<Edge>>;
    fn delete_edges_from(&self, repo: &str, src_id: i64, relation: EdgeRelation) -> Result<()>;

    fn record_scan(&self, repo: &str, entries: &[NewScanHistoryEntry]) -> Result<i64>;
    fn latest_scan(&self, repo: &str) -> Result<Option<ScanHistory>>;
    fn list_scans(&self, repo: &str) -> Result<Vec<ScanHistory>>;
    fn scan_entries(&self, scan_id: i64) -> Result<Vec<ScanHistoryEntry>>;
}

/// Natural-key upsert: finds an existing node by `(repo, kind, path, name,
/// container)` and updates it in place (id-preserving) if found, else
/// inserts a new one. A free function rather than a trait method, since
/// it's built entirely out of `find_node`/`add_node`/`update_node` — no
/// adapter needs to reimplement it.
pub fn upsert_node(store: &dyn GraphStore, node: NewNode) -> Result<i64> {
    match store.find_node(&node.repo, node.kind, node.path.as_deref(), node.name.as_deref(), node.container.as_deref())? {
        Some(existing) => {
            store.update_node(&node.repo, existing.id, node.start_line, node.end_line, node.content.clone())?;
            Ok(existing.id)
        }
        None => store.add_node(node),
    }
}

/// Deletes every node of `kind` in `repo` not in `keep_ids`, returning the
/// pruned nodes (so a caller like `scan_and_persist` can turn them into
/// `Removed` scan-history entries). Repo-scoped by construction now that
/// `nodes_by_kind` itself is — `main`'s equivalent had to filter by repo in
/// application code after an unscoped query, with a dedicated regression
/// test just to guard that convention.
pub fn prune_stale_nodes(store: &dyn GraphStore, repo: &str, kind: NodeKind, keep_ids: &[i64]) -> Result<Vec<Node>> {
    let existing = store.nodes_by_kind(repo, kind)?;
    let stale: Vec<Node> = existing.into_iter().filter(|n| !keep_ids.contains(&n.id)).collect();
    if !stale.is_empty() {
        let ids: Vec<i64> = stale.iter().map(|n| n.id).collect();
        store.delete_nodes(repo, &ids)?;
    }
    Ok(stale)
}

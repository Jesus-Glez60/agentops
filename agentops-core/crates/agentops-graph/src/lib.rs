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
    pub fn as_db_str(&self) -> &'static str {
        match self {
            NodeKind::Symbol => "symbol",
            NodeKind::File => "file",
            NodeKind::Gotcha => "gotcha",
            NodeKind::Decision => "decision",
            NodeKind::Definition => "definition",
            NodeKind::Note => "note",
        }
    }

    pub fn from_db_str(s: &str) -> Self {
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
    pub fn as_db_str(&self) -> &'static str {
        match self {
            EdgeRelation::DependsOn => "depends_on",
            EdgeRelation::Documents => "documents",
            EdgeRelation::Affects => "affects",
        }
    }

    pub fn from_db_str(s: &str) -> Self {
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
    /// Starts at `1.0`, bumped by `reinforce_edge` each time the same
    /// `(note, symbol)` pair is re-matched (e.g. on every rescan) — see
    /// `effective_weight` for how this decays with age at read time.
    /// `DependsOn` edges are fully replaced every scan (via
    /// `delete_edges_from` and a fresh `add_edge`), so their weight is
    /// always freshly `1.0` — that's correct, not a bug: a dependency edge
    /// is a deterministic structural fact, not something that should
    /// accumulate "relevance."
    pub weight: f64,
    pub updated_at: String,
}

/// Half-life, in days, `effective_weight` decays an `Affects` edge's weight
/// over — a deliberate first-pass constant, not configurable yet.
pub const AFFECTS_EDGE_HALF_LIFE_DAYS: f64 = 30.0;

/// Decay is a pure function applied at read time, not a stored/background
/// value — sidesteps needing any scheduler/background-job infrastructure
/// (none exists in this project) for something that only matters when an
/// edge is actually being read/ranked.
pub fn effective_weight(weight: f64, age_days: f64) -> f64 {
    weight * 0.5_f64.powf(age_days / AFFECTS_EDGE_HALF_LIFE_DAYS)
}

/// Parses an `Edge.updated_at`/`Node`-adjacent timestamp string into "days
/// since then, as of now" — a pure, dependency-free function shared by both
/// backends so ranking is computed identically rather than one adapter
/// doing it in SQL and the other in Rust. Deliberately tolerant of format
/// differences between the two backends' timestamp text representations:
/// SQLite's `CURRENT_TIMESTAMP` produces `"YYYY-MM-DD HH:MM:SS"`; Postgres's
/// `updated_at::text` cast (the same convention `scan_history.started_at`
/// already uses) produces `"YYYY-MM-DD HH:MM:SS[.ffffff][+TZ]"` — both share
/// the same first 19 characters, which is all that's parsed; anything after
/// that (fractional seconds, timezone offset) is ignored, since day-level
/// precision is all `effective_weight`'s 30-day half-life needs.
pub fn age_days(timestamp: &str) -> f64 {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs_f64();
    match parse_unix_seconds(timestamp) {
        Some(then) => ((now - then) / 86_400.0).max(0.0),
        // An unparseable/missing timestamp is treated as "very old" (fully
        // decayed) rather than "brand new" — a safe default that ranks it
        // last instead of accidentally floating an unreinforced edge to
        // the top.
        None => f64::MAX,
    }
}

fn parse_unix_seconds(timestamp: &str) -> Option<f64> {
    let prefix = timestamp.get(0..19)?;
    let (date, time) = prefix.split_once(' ')?;
    let mut date_parts = date.split('-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: i64 = date_parts.next()?.parse().ok()?;
    let day: i64 = date_parts.next()?.parse().ok()?;
    let mut time_parts = time.split(':');
    let hour: i64 = time_parts.next()?.parse().ok()?;
    let minute: i64 = time_parts.next()?.parse().ok()?;
    let second: i64 = time_parts.next()?.parse().ok()?;

    // Howard Hinnant's `days_from_civil` — a well-known, dependency-free
    // Gregorian civil-date-to-days algorithm, correct for any proleptic
    // Gregorian date without pulling in a date/time crate for one function.
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days_since_epoch = era * 146_097 + doe - 719_468;

    Some((days_since_epoch * 86_400 + hour * 3600 + minute * 60 + second) as f64)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScanChange {
    Added,
    Changed,
    Removed,
}

impl ScanChange {
    pub fn as_db_str(&self) -> &'static str {
        match self {
            ScanChange::Added => "added",
            ScanChange::Changed => "changed",
            ScanChange::Removed => "removed",
        }
    }

    pub fn from_db_str(s: &str) -> Self {
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

/// A repo-level snapshot cache — "what does this repo's graph currently
/// think matters most" — deliberately *not* a history table (that's
/// `node_versions`/bi-temporal versioning's job for node content). One row
/// per repo, upserted in place on every `refresh_repo_state`, not
/// append-only.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoState {
    pub repo: String,
    pub updated_at: String,
    pub last_scan_id: Option<i64>,
    pub top_gotcha_ids: Vec<i64>,
    pub top_decision_ids: Vec<i64>,
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
    /// Bumps `edge_id`'s weight (capped) and its `updated_at` to now —
    /// called instead of `add_edge` when `connect_many` finds the target
    /// edge already exists, so repeat matches strengthen a relationship
    /// instead of being silently skipped.
    fn reinforce_edge(&self, repo: &str, edge_id: i64) -> Result<()>;

    fn record_scan(&self, repo: &str, entries: &[NewScanHistoryEntry]) -> Result<i64>;
    fn latest_scan(&self, repo: &str) -> Result<Option<ScanHistory>>;
    fn list_scans(&self, repo: &str) -> Result<Vec<ScanHistory>>;
    fn scan_entries(&self, scan_id: i64) -> Result<Vec<ScanHistoryEntry>>;

    /// Attaches (or replaces) `node_id`'s embedding — a separate step from
    /// node creation, not a `NewNode` field, so callers that never embed
    /// (most of `agentops-notes`) never need to know embeddings exist at
    /// all. Update-safe: a node's content (and thus embedding) can change
    /// across rescans while its id stays stable, so this must overwrite any
    /// existing embedding for `node_id`, not just insert-once. Mirrors
    /// `DocbrainStore::upsert_doc_node`'s "insert node, then a separate
    /// insert into the vector table" precedent, just as its own method
    /// rather than folded into `add_node`.
    fn set_embedding(&self, repo: &str, node_id: i64, embedding: &[f32]) -> Result<()>;
    /// KNN search — nearest first, `distance` as the score (lower = closer).
    /// `kind`, if given, restricts results to one `NodeKind` (e.g. only
    /// `Gotcha`/`Decision` for a "what do we already know" search, vs. only
    /// `Symbol` for "what code does this").
    fn search_similar(&self, repo: &str, embedding: &[f32], top_k: usize, kind: Option<NodeKind>) -> Result<Vec<(Node, f32)>>;

    /// Recomputes and upserts `repo`'s `RepoState` row from the current
    /// graph (top 5 `Gotcha`/`Decision` nodes by `effective_weight`, summed
    /// across their own outgoing `Affects` edges — see `rank_notes_by_weight`).
    /// Ranking is computed identically on both backends — in Rust, not in
    /// SQL — so the two adapters can never silently diverge on what "top"
    /// means.
    fn refresh_repo_state(&self, repo: &str) -> Result<RepoState>;
    /// Reads the cached snapshot without recomputing it. `None` means no
    /// scan with gotchas/decisions has ever refreshed it yet for this repo
    /// — a real, expected state for a repo scanned before this feature
    /// existed, not an error.
    fn get_repo_state(&self, repo: &str) -> Result<Option<RepoState>>;
}

/// Ranks every `kind` node (`Gotcha`/`Decision`) in `repo` by the sum of
/// `effective_weight` across its own outgoing `Affects` edges (a note is
/// the `src`, the symbols it affects are `dst`s — see `EdgeRelation::Affects`'s
/// doc comment), returning the top 5 node ids, highest first. A free
/// function built entirely out of `nodes_by_kind`/`edges_from` — like
/// `upsert_node`/`prune_stale_nodes` — so both `SqliteGraphStore` and
/// `PostgresGraphStore` call the exact same ranking logic from their
/// `refresh_repo_state` impl instead of each maintaining their own copy.
pub fn rank_notes_by_weight(store: &dyn GraphStore, repo: &str, kind: NodeKind) -> Result<Vec<i64>> {
    let nodes = store.nodes_by_kind(repo, kind)?;
    let mut scored: Vec<(i64, f64)> = Vec::with_capacity(nodes.len());
    for node in &nodes {
        let score: f64 = store
            .edges_from(repo, node.id)?
            .iter()
            .filter(|e| e.relation == EdgeRelation::Affects)
            .map(|e| effective_weight(e.weight, age_days(&e.updated_at)))
            .sum();
        scored.push((node.id, score));
    }
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    Ok(scored.into_iter().take(5).map(|(id, _)| id).collect())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_weight_at_zero_age_equals_the_raw_weight() {
        assert_eq!(effective_weight(3.0, 0.0), 3.0);
    }

    #[test]
    fn effective_weight_halves_at_exactly_one_half_life() {
        let decayed = effective_weight(4.0, AFFECTS_EDGE_HALF_LIFE_DAYS);
        assert!((decayed - 2.0).abs() < 1e-9, "expected ~2.0, got {decayed}");
    }

    #[test]
    fn effective_weight_approaches_zero_for_very_old_edges() {
        let decayed = effective_weight(5.0, AFFECTS_EDGE_HALF_LIFE_DAYS * 20.0);
        assert!(decayed < 0.001, "expected near-zero, got {decayed}");
    }

    #[test]
    fn age_days_of_now_is_approximately_zero() {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        // SQLite's CURRENT_TIMESTAMP shape: "YYYY-MM-DD HH:MM:SS" (UTC).
        let formatted = format_unix_for_test(now);
        assert!(age_days(&formatted) < 0.01, "expected ~0 days, got {}", age_days(&formatted));
    }

    #[test]
    fn age_days_tolerates_postgres_style_fractional_seconds_and_timezone_suffix() {
        // Postgres's `updated_at::text` cast produces this shape, unlike
        // SQLite's plain "YYYY-MM-DD HH:MM:SS" — both must parse the same.
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        let formatted = format!("{}.123456+00", format_unix_for_test(now));
        assert!(age_days(&formatted) < 0.01, "expected ~0 days despite the suffix, got {}", age_days(&formatted));
    }

    #[test]
    fn age_days_of_unparseable_timestamp_is_treated_as_fully_decayed() {
        assert_eq!(age_days("not a timestamp"), f64::MAX);
    }

    /// A tiny, dependency-free formatter for these tests only — mirrors
    /// what SQLite's own `CURRENT_TIMESTAMP` produces, without needing a
    /// date/time crate just to write the test.
    fn format_unix_for_test(unix_secs: u64) -> String {
        let days = (unix_secs / 86_400) as i64;
        let secs_of_day = unix_secs % 86_400;
        let (h, m, s) = (secs_of_day / 3600, (secs_of_day % 3600) / 60, secs_of_day % 60);

        // Inverse of `parse_unix_seconds`'s `days_from_civil` — civil date
        // from a day count since the epoch (also Howard Hinnant's algorithm).
        let z = days + 719_468;
        let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m_ = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = if m_ <= 2 { y + 1 } else { y };

        format!("{y:04}-{m_:02}-{d:02} {h:02}:{m:02}:{s:02}")
    }
}

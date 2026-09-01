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

use std::collections::HashMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NodeKind {
    Symbol,
    File,
    Gotcha,
    Decision,
    Definition,
    Note,
    /// One `agentops-docgen::DocSection` indexed as its own searchable node
    /// (Initiative 2, CLS-inspired retrieval plan) -- the "gist"/cortical
    /// tier: `content` is the section's blocks flattened to plain text,
    /// `name` is the section title, `path` is `"doc_section:{section.id}"`
    /// (stable across regenerations, same pseudo-path idiom
    /// `agentops-notes` already uses for vault notes: `"vault:{source_path}"`).
    /// Connected to the underlying nodes it covers via `EdgeRelation::Covers`
    /// -- deliberately its own relation, not `Documents`: that relation's
    /// existing sole consumer (`agentops-docgen::sectioned::documenting_summary`)
    /// assumes every `Documents` edge's source is a `Definition` node whose
    /// `content` is a one-liner explanation, and would misread a
    /// `DocSection`'s much longer flattened text the same way.
    DocSection,
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
            NodeKind::DocSection => "doc_section",
        }
    }

    pub fn from_db_str(s: &str) -> Self {
        match s {
            "file" => NodeKind::File,
            "gotcha" => NodeKind::Gotcha,
            "decision" => NodeKind::Decision,
            "definition" => NodeKind::Definition,
            "note" => NodeKind::Note,
            "doc_section" => NodeKind::DocSection,
            _ => NodeKind::Symbol,
        }
    }
}

/// Curation state for a `Gotcha` node's `prominence` field -- same
/// as_db_str/from_db_str-backed-TEXT-column pattern as `TaskStatus`.
/// Curation never removes/hides a gotcha (it's permanent knowledge an
/// agent needs to keep seeing) -- `Reduced` only lowers how prominently it
/// ranks, always paired with a `curation_reason` explaining why.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeProminence {
    Full,
    Reduced,
}

impl NodeProminence {
    pub fn as_db_str(&self) -> &'static str {
        match self {
            NodeProminence::Full => "full",
            NodeProminence::Reduced => "reduced",
        }
    }

    pub fn from_db_str(s: &str) -> Self {
        match s {
            "reduced" => NodeProminence::Reduced,
            _ => NodeProminence::Full,
        }
    }
}

/// Ranking-only multiplier for a `Reduced`-prominence node's relevance
/// score -- 1.0 (no change) for `Full`. Never apply this to a value that's
/// also displayed as a "real" number (raw cosine similarity, BM25/RRF
/// fused score, KNN distance) -- multiply a copy used only as a sort key,
/// so the number shown to a human or agent stays truthful.
pub fn prominence_rank_multiplier(prominence: NodeProminence) -> f64 {
    match prominence {
        NodeProminence::Full => 1.0,
        NodeProminence::Reduced => 0.1,
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
    /// A symbol references another symbol defined in the **same file** —
    /// directed **from the referencing symbol to the referenced one**.
    /// Detected via same-file identifier matching (AST-precise where
    /// tree-sitter parsed the file, word-boundary text matching as a
    /// fallback where it didn't) — not real call-graph analysis, and never
    /// cross-file. See `agentops_scanner::resolve_same_file_symbol_references`.
    ///
    /// **Plastic, like `Affects` (Initiative 1, CLS-inspired retrieval
    /// plan) — no longer purely deterministic-and-replaced.** Earlier in
    /// this codebase's history, every `References` edge was deleted and
    /// recreated at `weight: 1.0` on every scan that touched its source
    /// symbol, on the reasoning that a same-file reference is a
    /// deterministic structural fact with no "relevance" to accumulate.
    /// That's been superseded: a reference re-confirmed on a later rescan
    /// now reinforces its existing edge (`reinforce_edge`, same as a
    /// repeat-matched `Affects` edge) instead of resetting to `1.0`, and a
    /// reference whose target genuinely disappeared is pruned via
    /// `delete_edge` rather than the whole per-symbol set being wiped and
    /// rebuilt. See `agentops-mcp::scan::persist`'s References block and
    /// `REFERENCES_EDGE_HALF_LIFE_DAYS` (its own half-life, not
    /// `AFFECTS_EDGE_HALF_LIFE_DAYS` — same per-relation-semantics
    /// convention as everything else on this enum, not an oversight).
    References,
    /// A `NodeKind::DocSection` covers a `Symbol`/`File` it documents —
    /// directed **from the section to the covered node** (Initiative 2,
    /// CLS-inspired retrieval plan). Deliberately its own relation, not
    /// `Documents`: `Documents`' sole existing consumer
    /// (`agentops-docgen::sectioned::documenting_summary`) assumes every
    /// `Documents` edge's source is a `Definition` node whose `content` is a
    /// one-liner explanation, and would misread a `DocSection`'s much
    /// longer flattened section text the same way if this reused it.
    /// `search_gist_then_detail` (`agentops-retrieval`) walks a matched
    /// section's outgoing `Covers` edges to scope its second, detail-tier
    /// search pass.
    Covers,
}

impl EdgeRelation {
    pub fn as_db_str(&self) -> &'static str {
        match self {
            EdgeRelation::DependsOn => "depends_on",
            EdgeRelation::Documents => "documents",
            EdgeRelation::Affects => "affects",
            EdgeRelation::References => "references",
            EdgeRelation::Covers => "covers",
        }
    }

    pub fn from_db_str(s: &str) -> Self {
        match s {
            "documents" => EdgeRelation::Documents,
            "affects" => EdgeRelation::Affects,
            "references" => EdgeRelation::References,
            "covers" => EdgeRelation::Covers,
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
    /// Curation state for a `Gotcha` node (meaningless but harmless on other
    /// kinds) -- all three default per the column's own SQL default (not
    /// set via `NewNode`/`add_node`), moved via the dedicated `set_curation`
    /// mutator only. Never touched by `upsert_node`/rescans, so a curated
    /// gotcha's prominence/reason survives every future scan.
    pub curated: bool,
    pub prominence: NodeProminence,
    /// `Some` (non-empty) iff `prominence == Reduced` -- why this gotcha's
    /// prominence was lowered, shown alongside it everywhere it surfaces
    /// (dashboard, MCP tools, docgen, prompts) so the demotion is never a
    /// silent, unexplained downranking.
    pub curation_reason: Option<String>,
    /// When this node was last inserted or updated (Initiative 3,
    /// CLS-inspired retrieval plan) -- feeds `search_hybrid`'s recency
    /// ranking via the same `effective_weight`/`age_days` decay already
    /// used for edge plasticity. `Option` because `PostgresGraphStore`
    /// doesn't populate it yet (see `schema.sql`'s own note on why) and
    /// deliberately returns `None` rather than a wrong/stale value —
    /// `None` makes recency ranking a no-op there, not a crash or a lie.
    pub last_touched_at: Option<String>,
}

/// A single Core Modules grouping for the Documentation Viewer -- either
/// LLM-labeled (`agentops-llm::group_core_modules`) or a directory-name
/// heuristic fallback (`agentops-docgen::sectioned::build_doc_page`).
/// Deliberately just data, and deliberately living here rather than in
/// either producer/consumer crate: `agentops-docgen` and `agentops-llm`
/// both depend on `agentops-graph` already but must not depend on each
/// other (docgen stays network-free; llm stays scan/rank-free), so this
/// crate is the one place both can share the type without creating a
/// cycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleLabel {
    pub label: String,
    pub file_paths: Vec<String>,
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
    /// `(note, symbol)` pair (`Affects`) or the same same-file reference
    /// (`References`, Initiative 1) is re-matched/re-confirmed (e.g. on
    /// every rescan) — see `effective_weight`/`effective_edge_weight` for
    /// how this decays with age at read time. `DependsOn` edges are fully
    /// replaced every scan (via `delete_edges_from` and a fresh
    /// `add_edge`), so their weight is always freshly `1.0` — that's
    /// correct, not a bug: a file-level dependency is a deterministic
    /// structural fact, not something that should accumulate "relevance."
    pub weight: f64,
    pub updated_at: String,
}

/// Half-life, in days, `effective_weight` decays an `Affects` edge's weight
/// over — a deliberate first-pass constant, not configurable yet.
pub const AFFECTS_EDGE_HALF_LIFE_DAYS: f64 = 30.0;

/// Half-life, in days, a `References` edge's weight decays over (Initiative
/// 1, CLS-inspired retrieval plan) — deliberately longer than `Affects`'
/// 30 days: a same-file symbol reference is a passively-reconfirmed
/// structural fact re-observed by every rescan that still finds it, not a
/// deliberate human action the way manually re-adding a note is, so it
/// shouldn't decay to irrelevance on the same timescale.
pub const REFERENCES_EDGE_HALF_LIFE_DAYS: f64 = 90.0;

/// Decay is a pure function applied at read time, not a stored/background
/// value — sidesteps needing any scheduler/background-job infrastructure
/// (none exists in this project) for something that only matters when an
/// edge is actually being read/ranked. Defaults to `Affects`' half-life —
/// existing callers (`note_score`, `rank_notes_by_weight`) only ever rank
/// `Affects` edges, so this signature stays unchanged; a caller that must
/// handle a mix of relations (e.g. `agentops-retrieval`'s Personalized
/// PageRank, which spreads activation over both `Affects` and
/// `References`) should use `effective_edge_weight` instead, which picks
/// the right half-life per edge.
pub fn effective_weight(weight: f64, age_days: f64) -> f64 {
    effective_weight_with_half_life(weight, age_days, AFFECTS_EDGE_HALF_LIFE_DAYS)
}

/// Same decay curve as `effective_weight`, but with an explicit half-life
/// rather than always assuming `Affects`' 30 days.
pub fn effective_weight_with_half_life(weight: f64, age_days: f64, half_life_days: f64) -> f64 {
    weight * 0.5_f64.powf(age_days / half_life_days)
}

/// Applies the correct half-life for `edge.relation` -- the single place
/// that knows which `*_HALF_LIFE_DAYS` constant governs which relation, so
/// a caller ranking/spreading over a mix of edge relations doesn't have to
/// duplicate that mapping itself.
pub fn effective_edge_weight(edge: &Edge) -> f64 {
    let half_life = match edge.relation {
        EdgeRelation::References => REFERENCES_EDGE_HALF_LIFE_DAYS,
        // `Covers` edges are fully replaced on every doc regeneration (like
        // `DependsOn`), not reinforced -- there's no "repeat confirmation"
        // signal to decay, so the specific half-life picked here is moot in
        // practice; bucketed with the other non-`References` relations for
        // an exhaustive match rather than inventing an unused third constant.
        EdgeRelation::DependsOn | EdgeRelation::Documents | EdgeRelation::Affects | EdgeRelation::Covers => AFFECTS_EDGE_HALF_LIFE_DAYS,
    };
    effective_weight_with_half_life(edge.weight, age_days(&edge.updated_at), half_life)
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

/// One version of a node's content over time — bi-temporal history.
/// `node_id` is deliberately not a hard FK, same reasoning as
/// `NewScanHistoryEntry`: history must survive the node's own eventual
/// pruning. `valid_until: None` means this is the currently-open version.
#[derive(Debug, Clone)]
pub struct NodeVersion {
    pub id: i64,
    pub node_id: i64,
    pub content: Option<String>,
    pub start_line: Option<i64>,
    pub end_line: Option<i64>,
    pub valid_from: String,
    pub valid_until: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskStatus {
    Todo,
    InProgress,
    InReview,
    Done,
    Cancelled,
}

impl TaskStatus {
    pub fn as_db_str(&self) -> &'static str {
        match self {
            TaskStatus::Todo => "todo",
            TaskStatus::InProgress => "in_progress",
            TaskStatus::InReview => "in_review",
            TaskStatus::Done => "done",
            TaskStatus::Cancelled => "cancelled",
        }
    }

    pub fn from_db_str(s: &str) -> Self {
        match s {
            "in_progress" => TaskStatus::InProgress,
            "in_review" => TaskStatus::InReview,
            "done" => TaskStatus::Done,
            "cancelled" => TaskStatus::Cancelled,
            _ => TaskStatus::Todo,
        }
    }
}

/// Module 7 (1.0 roadmap): AgentOps's own native task state, not just an
/// activity tag — see `~/Vaults/agentops-vnext/decisions/hybrid-task-manager-linear.md`.
/// `external_source`/`external_id` are both `None` for a native task;
/// `Some("linear")`/`Some(<issue id>)` for one pulled from Linear —
/// `(external_source, external_id)` is the natural key a Linear-sync upsert
/// keys on. `session_id` is Module 6's correlation id: once set, every
/// `session_events` row under it is transitively this task's own activity
/// feed via `get_task_activity`.
#[derive(Debug, Clone)]
pub struct NewTask {
    pub repo: String,
    pub title: String,
    pub description: Option<String>,
    pub status: TaskStatus,
    pub priority: Option<String>,
    pub assignee: Option<String>,
    pub external_source: Option<String>,
    pub external_id: Option<String>,
    pub session_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Task {
    pub id: i64,
    pub repo: String,
    pub title: String,
    pub description: Option<String>,
    pub status: TaskStatus,
    pub priority: Option<String>,
    pub assignee: Option<String>,
    pub external_source: Option<String>,
    pub external_id: Option<String>,
    pub session_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Links a task to the symbol/file/gotcha/decision it touches —
/// `(task_id, node_id, relation)` is a natural key: re-linking the same
/// pair is a no-op, not a duplicate row.
#[derive(Debug, Clone)]
pub struct TaskLink {
    pub task_id: i64,
    pub node_id: i64,
    pub relation: String,
}

/// One notable write action tagged with a caller-supplied `session_id` —
/// Module 6's cross-tool session correlation. Not scan-scoped (unlike
/// `ScanHistoryEntry`, which only exists inside one `scan_repo` call): any
/// write tool (`scan_repo`, `add_note`, `ingest_notes`, `explain_symbol`)
/// records one row per call when the caller passes the same `session_id`
/// across calls, regardless of tool or repo-write-type, so `get_session`
/// can return one correlated feed spanning multiple MCP/REST clients
/// working the same session.
#[derive(Debug, Clone)]
pub struct SessionEvent {
    pub id: i64,
    pub repo: String,
    pub session_id: String,
    pub tool_name: String,
    pub description: String,
    pub created_at: String,
}

/// The codebase graph port. One adapter today (`SqliteGraphStore`); the
/// trait boundary is what lets a future adapter (Postgres-backed, shared
/// across repos) exist without touching any use-case or MCP-handler code.
pub trait GraphStore {
    /// `PostgresGraphStore` enforces this trait's own natural-key identity
    /// rule (`repo, kind, path, name, container`) at the schema level via a
    /// unique index (`idx_nodes_natural_key`) — a direct `add_node` call for
    /// a node whose natural key collides with an existing row now fails
    /// loudly there instead of silently creating a duplicate. Not a concern
    /// for any current call site (every one already goes through
    /// `upsert_node`, which checks first), but worth knowing before adding a
    /// new one that calls `add_node` directly.
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
    /// Marks a node curated and sets its prominence in one call -- there's
    /// no standalone "mark curated but change nothing" action, since
    /// choosing a prominence *is* the curation act. `reason` should be
    /// `Some` (non-empty) when `prominence` is `Reduced`, `None` when
    /// `Full` -- enforced by the API layer, not the store. A separate
    /// mutator, not a new `update_node` parameter, so `upsert_node`
    /// (called by every rescan/note-ingest) never touches it: a gotcha's
    /// curation survives every future rescan the same way an embedding does.
    fn set_curation(&self, repo: &str, node_id: i64, prominence: NodeProminence, reason: Option<&str>) -> Result<()>;
    fn delete_nodes(&self, repo: &str, ids: &[i64]) -> Result<()>;

    fn add_edge(&self, repo: &str, src_id: i64, dst_id: i64, relation: EdgeRelation) -> Result<i64>;
    fn edges_from(&self, repo: &str, src_id: i64) -> Result<Vec<Edge>>;
    fn edges_to(&self, repo: &str, dst_id: i64) -> Result<Vec<Edge>>;
    fn all_edges(&self, repo: &str) -> Result<Vec<Edge>>;
    fn delete_edges_from(&self, repo: &str, src_id: i64, relation: EdgeRelation) -> Result<()>;
    /// Deletes exactly one edge by id -- unlike `delete_edges_from`'s bulk
    /// "every edge of this relation from this source" semantics, needed for
    /// selective pruning where some edges from the same source must survive
    /// (reinforced) while others are dropped in the same pass (a
    /// `References` edge whose target reference disappeared this scan --
    /// see `agentops-mcp::scan::persist`, Initiative 1 of the CLS-inspired
    /// retrieval plan). A no-op, not an error, if `edge_id` doesn't exist or
    /// belongs to a different repo.
    fn delete_edge(&self, repo: &str, edge_id: i64) -> Result<()>;
    /// Bumps `edge_id`'s weight (capped), always. Bumps `updated_at` to now
    /// only when `bump_confirmed_at` is true — called instead of `add_edge`
    /// when `connect_many` finds the target edge already exists, so repeat
    /// matches strengthen a relationship instead of being silently skipped.
    ///
    /// `bump_confirmed_at` must be `false` for the automatic every-scan
    /// note re-match (Module B, `agentops_notes::ingest_vault` called from
    /// `scan::persist`) and `true` only for an explicit, human-initiated
    /// reinforcement (re-adding the same note via `add_note`). Otherwise
    /// `updated_at` would be refreshed to "now" on every single rescan
    /// regardless of whether the target symbol's content actually changed
    /// — a real bug caught while wiring up bi-temporal staleness surfacing
    /// (`tool_get_symbol`): the automatic rematch made every gotcha look
    /// freshly confirmed immediately after any edit, permanently defeating
    /// the "has this symbol changed since the note was last confirmed
    /// relevant" comparison against `node_history`.
    fn reinforce_edge(&self, repo: &str, edge_id: i64, bump_confirmed_at: bool) -> Result<()>;

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
    /// Reads back `node_id`'s raw embedding, if it has one -- `None` covers
    /// both "node doesn't exist in this repo" and "exists but was never
    /// embedded", the same way `search_similar` already treats an
    /// unembedded node as simply absent from KNN results rather than an
    /// error. Needed by `agentops-embeddings-train` (Initiative 5,
    /// CLS-inspired retrieval plan) for hard-negative mining and
    /// query-time re-ranking, where a raw vector is needed for a
    /// *specific* node id rather than a KNN search's already-ranked list.
    fn get_embedding(&self, repo: &str, node_id: i64) -> Result<Option<Vec<f32>>>;
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

    /// Upserts `repo`'s generated Documentation Viewer page as an
    /// already-serialized JSON blob (`agentops_docgen::DocPage`, serialized
    /// by the caller). Deliberately typed as a plain string rather than
    /// `&DocPage`: `agentops-docgen` depends on this crate for `GraphStore`,
    /// so the reverse dependency would be a cycle. The caller (`agentops-mcp`,
    /// which already depends on both `agentops-graph` and
    /// `agentops-docgen`) owns the typed value and the `serde_json::to_string`
    /// call; this trait only ever sees opaque text.
    fn save_doc_page(&self, repo: &str, generated_at: &str, content_json: &str) -> Result<()>;
    /// Reads back `(generated_at, content_json)` for `repo`, if a doc page
    /// has ever been generated for it. `None` is the expected state for a
    /// repo scanned before this feature existed, or before its first scan
    /// completes.
    fn get_doc_page(&self, repo: &str) -> Result<Option<(String, String)>>;

    /// Records `node_id`'s content as of now: closes whatever version was
    /// previously open for it (if any — a no-op for a brand-new node's
    /// first version) and opens a new one with `content`/`start_line`/
    /// `end_line`. Called from `persist()` for every `Added`/`Changed`
    /// symbol, right alongside the existing `scan_history_entries` write —
    /// same trigger, same `added`/`changed` classification, no new
    /// comparison logic. Always pass the *new*, post-upsert values, not the
    /// old ones: the old content is already durably captured in whatever
    /// version this call closes out.
    fn snapshot_node_version(&self, node_id: i64, content: Option<&str>, start_line: Option<i64>, end_line: Option<i64>) -> Result<()>;
    /// Closes `node_id`'s currently-open version with no replacement —
    /// called on `Removed` symbols, so `node_history` accurately shows "the
    /// last known content was valid until removal," not "still open
    /// forever" for a node that no longer exists.
    fn close_node_version(&self, node_id: i64) -> Result<()>;
    /// Every version of `node_id`, most recent first.
    fn node_history(&self, node_id: i64) -> Result<Vec<NodeVersion>>;
    /// The version that was valid at `timestamp` (`valid_from <= timestamp`
    /// and (`valid_until IS NULL` or `valid_until > timestamp`)) — `None` if
    /// `node_id` has no version history at all (e.g. scanned before this
    /// feature existed).
    fn node_as_of(&self, node_id: i64, timestamp: &str) -> Result<Option<NodeVersion>>;

    /// Records one `SessionEvent` row — see its doc comment. `repo`-scoped
    /// like everything else in this trait, but note `session_events` (unlike
    /// `session_id` returns) is queried by `(repo, session_id)` together:
    /// a session_id is only meaningful within one repo's activity feed.
    fn record_session_event(&self, repo: &str, session_id: &str, tool_name: &str, description: &str) -> Result<i64>;
    /// Every event recorded under `session_id` in `repo`, oldest first —
    /// the correlated feed `get_session` renders.
    fn session_events(&self, repo: &str, session_id: &str) -> Result<Vec<SessionEvent>>;

    /// Creates a native task. For a Linear-sourced task, prefer
    /// `upsert_external_task` instead, which is idempotent on
    /// `(external_source, external_id)` — this always inserts a new row.
    fn create_task(&self, task: NewTask) -> Result<i64>;
    fn get_task(&self, id: i64) -> Result<Option<Task>>;
    fn list_tasks(&self, repo: &str) -> Result<Vec<Task>>;
    fn update_task_status(&self, id: i64, status: TaskStatus) -> Result<()>;
    /// Inserts a task keyed on `(external_source, external_id)`, or updates
    /// the existing row in place if that pair already exists — the
    /// Linear-sync pull path calling this repeatedly must never duplicate
    /// an already-synced issue.
    fn upsert_external_task(&self, task: NewTask) -> Result<i64>;
    /// Idempotent: re-linking the same `(task_id, node_id, relation)` is a
    /// no-op, not a duplicate row.
    fn link_task(&self, task_id: i64, node_id: i64, relation: &str) -> Result<()>;
    fn task_links(&self, task_id: i64) -> Result<Vec<TaskLink>>;

    /// BM25-ranked full-text search over `name`/`content` (SQLite FTS5 /
    /// Postgres `tsvector`, adapter-specific). Kept automatically in sync
    /// with `nodes` at the database level (triggers), not by any Rust
    /// call site — see each adapter's migration for why. The returned
    /// score is **adapter-internal and ordering-only**: SQLite's `bm25()`
    /// is lower-is-better, Postgres's `ts_rank` is higher-is-better: do not
    /// compare scores across adapters or against `search_similar`'s cosine
    /// distance — the result `Vec` is already sorted best-first, which is
    /// the only thing `agentops-retrieval`'s fusion actually relies on.
    fn search_lexical(&self, repo: &str, query: &str, top_k: usize, kind: Option<NodeKind>) -> Result<Vec<(Node, f32)>>;
    /// Cheap exact/substring name match, case-insensitive — an exact
    /// `name` match ranks first, then substring matches shortest-name
    /// first. Same "best-first, don't compare the score across sources"
    /// contract as `search_lexical`.
    fn search_exact(&self, repo: &str, query: &str, top_k: usize, kind: Option<NodeKind>) -> Result<Vec<(Node, f32)>>;

    // -- Batch primitives (Postgres pool/batching plan, Phase 2) --
    //
    // Every method below has a correct, loop-based default impl, so
    // `SqliteGraphStore` inherits working (if unbatched) behavior for free
    // -- only `PostgresGraphStore` overrides these with real multi-row SQL
    // (`UNNEST`-based), since it's the adapter that actually pays a network
    // round trip per call. `agentops-mcp::scan::persist` is the intended
    // caller: one call per phase (files, symbols, edges, ...) instead of one
    // per row, cutting a full scan from O(files×symbols) round trips to
    // O(files+symbols) against Postgres, with no behavior change against
    // SQLite.

    /// Batched `find_node`. `keys` are `(kind, path, name, container)`
    /// tuples; a key with no matching node is simply absent from the
    /// returned map (never an error) -- same "missing means not found, not
    /// found means missing" contract `find_node` itself has, just batched.
    ///
    /// **Correctness note for any real (non-default) override**: `find_node`
    /// compares `NULL` path/name/container with `IS`, not `=`, so two
    /// distinct nodes that both happen to have no `name` (e.g. two `File`
    /// nodes) never spuriously match each other. A naive `UNNEST` + `=` join
    /// does *not* preserve that -- any override must replicate the `IS`
    /// semantics exactly (`PostgresGraphStore`'s does, via `COALESCE`-based
    /// equality against the same natural-key shape `idx_nodes_natural_key`
    /// uses).
    fn find_nodes_batch(&self, repo: &str, keys: &[(NodeKind, Option<&str>, Option<&str>, Option<&str>)]) -> Result<HashMap<NaturalKey, Node>> {
        let mut out = HashMap::new();
        for &(kind, path, name, container) in keys {
            if let Some(node) = self.find_node(repo, kind, path, name, container)? {
                out.insert((kind, path.map(str::to_string), name.map(str::to_string), container.map(str::to_string)), node);
            }
        }
        Ok(out)
    }

    /// Batched natural-key upsert -- the batch analog of the free function
    /// `upsert_node`, not a batched version of `add_node` alone. Returns ids
    /// in the same order as `nodes`, so a caller can `zip` the two slices
    /// back together. Tolerates `nodes` containing two (or more) entries
    /// sharing the same natural key -- last-wins, matching what a
    /// sequential loop of `upsert_node` calls would do. This is not just a
    /// hypothetical edge case: confirmed live in production that
    /// `agentops-mcp::scan::persist`'s scanner can legitimately emit two
    /// `Symbol` entries with the same `(path, name, container)` for one
    /// file, and a naive `ON CONFLICT`-based override that assumes
    /// otherwise fails outright ("ON CONFLICT DO UPDATE command cannot
    /// affect row a second time" -- Postgres refuses to let one `INSERT`
    /// statement affect the same row twice via its conflict target). Any
    /// override must dedupe its own insert batch before executing (see
    /// `PostgresGraphStore`'s implementation for the pattern: dedupe the
    /// arrays fed to `INSERT`, but resolve returned ids against the *full*,
    /// non-deduped input so every original position still gets one back).
    ///
    /// The default impl duplicates (rather than calls) the free function
    /// `upsert_node`'s exact `find_node`-then-`update_node`/`add_node` logic
    /// -- it can't just delegate to that free function per item, since
    /// `upsert_node` takes `&dyn GraphStore` and coercing `&Self` to that
    /// inside a default method body would require a `Self: Sized` bound,
    /// which would exclude this method from `dyn GraphStore`'s vtable --
    /// and every real caller in this codebase holds its store as
    /// `Box<dyn GraphStore>`/`&dyn GraphStore`, never a concrete generic
    /// `S: GraphStore`. Calling `self.find_node(...)`/`self.add_node(...)`
    /// directly (ordinary trait-method calls, not a free-function argument
    /// needing unsizing) keeps this dyn-compatible like every other method
    /// here.
    fn upsert_nodes_batch(&self, nodes: &[NewNode]) -> Result<Vec<i64>> {
        nodes
            .iter()
            .map(|n| match self.find_node(&n.repo, n.kind, n.path.as_deref(), n.name.as_deref(), n.container.as_deref())? {
                Some(existing) => {
                    self.update_node(&n.repo, existing.id, n.start_line, n.end_line, n.content.clone())?;
                    Ok(existing.id)
                }
                None => self.add_node(n.clone()),
            })
            .collect()
    }

    /// Batched `add_edge`. Unlike `upsert_nodes_batch`, plain inserts here
    /// have no natural-key conflict to resolve (an edge's identity is just
    /// its own new row), so there's no same-batch-duplicate precondition to
    /// document.
    fn add_edges_batch(&self, repo: &str, edges: &[(i64, i64, EdgeRelation)]) -> Result<()> {
        for &(src_id, dst_id, relation) in edges {
            self.add_edge(repo, src_id, dst_id, relation)?;
        }
        Ok(())
    }

    /// Batched `set_embedding`.
    fn set_embeddings_batch(&self, repo: &str, embeddings: &[(i64, Vec<f32>)]) -> Result<()> {
        for (node_id, embedding) in embeddings {
            self.set_embedding(repo, *node_id, embedding)?;
        }
        Ok(())
    }

    /// Batched `snapshot_node_version`. `repo` is accepted for symmetry with
    /// every other batch method and so a real override can scope its
    /// `UPDATE ... WHERE repo = $n` clause -- `snapshot_node_version` itself
    /// takes no `repo` (see its own doc comment), so the default loop simply
    /// ignores this parameter when delegating.
    fn snapshot_node_versions_batch(&self, repo: &str, versions: &[(i64, Option<&str>, Option<i64>, Option<i64>)]) -> Result<()> {
        let _ = repo;
        for &(node_id, content, start_line, end_line) in versions {
            self.snapshot_node_version(node_id, content, start_line, end_line)?;
        }
        Ok(())
    }

    /// Batched `edges_from` -- one lookup per `src_id`, merged into a map
    /// keyed by `src_id` so a caller doesn't need to remember which input
    /// order produced which output (unlike `upsert_nodes_batch`'s
    /// order-preserving contract, order doesn't matter here since every
    /// result is already tagged with its own `src_id`).
    fn edges_from_batch(&self, repo: &str, src_ids: &[i64]) -> Result<HashMap<i64, Vec<Edge>>> {
        let mut out = HashMap::new();
        for &src_id in src_ids {
            out.insert(src_id, self.edges_from(repo, src_id)?);
        }
        Ok(out)
    }

    /// Batched `reinforce_edge` -- same `bump_confirmed_at` contract as the
    /// single-edge method (see its doc comment), applied uniformly across
    /// every id in one call (`persist()`'s automatic every-scan rematch
    /// never mixes `true`/`false` within the same batch).
    fn reinforce_edges_batch(&self, repo: &str, edge_ids: &[i64], bump_confirmed_at: bool) -> Result<()> {
        for &edge_id in edge_ids {
            self.reinforce_edge(repo, edge_id, bump_confirmed_at)?;
        }
        Ok(())
    }

    /// Batched `delete_edge`.
    fn delete_edges_batch(&self, repo: &str, edge_ids: &[i64]) -> Result<()> {
        for &edge_id in edge_ids {
            self.delete_edge(repo, edge_id)?;
        }
        Ok(())
    }
}

/// `(kind, path, name, container)` -- the natural-key identity portion not
/// already fixed by `repo` (`find_node`'s own first, always-scalar
/// parameter). Owned strings, since `find_nodes_batch`'s returned map
/// outlives the borrowed `&str` keys callers pass in.
pub type NaturalKey = (NodeKind, Option<String>, Option<String>, Option<String>);

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
        scored.push((node.id, note_score(store, repo, node)?));
    }
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    Ok(scored.into_iter().take(5).map(|(id, _)| id).collect())
}

/// A note's (Gotcha/Decision) overall relevance score: the sum of
/// `effective_weight` across its own outgoing `Affects` edges, damped by
/// `prominence_rank_multiplier` if it's been curated down. Shared by
/// `rank_notes_by_weight` (above) and `agentops-docgen`'s full-list
/// ordering, which previously each summed this independently -- one
/// implementation instead of two crates hand-kept in sync.
pub fn note_score(store: &dyn GraphStore, repo: &str, node: &Node) -> Result<f64> {
    let raw: f64 = store.edges_from(repo, node.id)?.iter().filter(|e| e.relation == EdgeRelation::Affects).map(|e| effective_weight(e.weight, age_days(&e.updated_at))).sum();
    Ok(raw * prominence_rank_multiplier(node.prominence))
}

/// Which direction(s) to follow edges in `bounded_neighborhood`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraversalDirection {
    Outgoing,
    Incoming,
    Both,
}

/// Result of `bounded_neighborhood`: every visited node paired with its BFS
/// depth from the nearest seed (`0` for a seed itself), every edge between
/// two visited nodes that matched the relation/direction filter, and
/// whether `cap` was hit before the frontier was exhausted.
#[derive(Debug)]
pub struct BoundedNeighborhood {
    pub nodes: Vec<(Node, u32)>,
    pub edges: Vec<Edge>,
    pub truncated: bool,
}

/// The traversal parameters for `bounded_neighborhood`, grouped into one
/// struct rather than passed positionally -- with `store`/`repo` this would
/// otherwise be an 8-argument function, and several of these (`direction`,
/// `max_depth`, `cap`) are the same `bool`/`u32`/`usize` shape, exactly the
/// kind of call site that's easy to transpose by accident with plain
/// positional args.
pub struct NeighborhoodQuery<'a> {
    pub seed_ids: &'a [i64],
    pub relations: &'a [EdgeRelation],
    pub direction: TraversalDirection,
    pub max_depth: u32,
    /// Empty = no filter.
    pub kind_filter: &'a [NodeKind],
    pub cap: usize,
}

/// BFS out from one or more seed nodes, following only `query.relations` in
/// `query.direction`, capped at `query.cap` total nodes (seeds included)
/// and `query.max_depth` hops. Extracted from
/// `agentops-api::subgraph::build_subgraph`'s original single-seed BFS so
/// there is exactly one bounded-traversal implementation shared by that
/// endpoint and `agentops-retrieval`'s Personalized PageRank (Initiative 0
/// of the CLS-inspired retrieval plan) — a second, separate
/// frontier-expansion loop in `agentops-retrieval` would duplicate real,
/// already-tested logic. Pays exactly one `edges_from`/`edges_to` call per
/// visited node regardless of caller — the same cost discipline
/// `build_subgraph`'s own `NODE_CAP` was already built to enforce against
/// `PostgresGraphStore`'s per-node blocking round trip, now shared instead
/// of being at risk of a second, uncapped caller reintroducing it.
///
/// `query.kind_filter` (empty = no filter) excludes a node *and* any edge
/// touching it from the result, and — like `build_subgraph`'s original
/// behavior — a filtered-out node is never expanded from either, since it
/// never enters `next_frontier`.
pub fn bounded_neighborhood(store: &dyn GraphStore, repo: &str, query: NeighborhoodQuery) -> Result<BoundedNeighborhood> {
    let NeighborhoodQuery { seed_ids, relations, direction, max_depth, kind_filter, cap } = query;
    use std::collections::HashSet;

    let mut visited_node_ids: HashSet<i64> = HashSet::new();
    let mut visited_edge_ids: HashSet<i64> = HashSet::new();
    let mut nodes_out: Vec<(Node, u32)> = Vec::new();
    let mut edges_out: Vec<Edge> = Vec::new();
    let mut frontier: Vec<i64> = Vec::new();
    let mut truncated = false;

    for &seed_id in seed_ids {
        if visited_node_ids.contains(&seed_id) {
            continue;
        }
        let Some(seed) = store.get_node(repo, seed_id)? else { continue };
        visited_node_ids.insert(seed_id);
        nodes_out.push((seed, 0));
        frontier.push(seed_id);
    }

    for hop in 1..=max_depth {
        if frontier.is_empty() || nodes_out.len() >= cap {
            break;
        }
        let mut next_frontier: Vec<i64> = Vec::new();

        for node_id in &frontier {
            let mut candidates: Vec<(Edge, i64)> = Vec::new();
            match direction {
                TraversalDirection::Outgoing => candidates.extend(store.edges_from(repo, *node_id)?.into_iter().map(|e| { let dst = e.dst_id; (e, dst) })),
                TraversalDirection::Incoming => candidates.extend(store.edges_to(repo, *node_id)?.into_iter().map(|e| { let src = e.src_id; (e, src) })),
                TraversalDirection::Both => {
                    candidates.extend(store.edges_from(repo, *node_id)?.into_iter().map(|e| { let dst = e.dst_id; (e, dst) }));
                    candidates.extend(store.edges_to(repo, *node_id)?.into_iter().map(|e| { let src = e.src_id; (e, src) }));
                }
            }

            for (edge, other_id) in candidates {
                if !relations.contains(&edge.relation) {
                    continue;
                }
                let Some(other) = store.get_node(repo, other_id)? else { continue };
                if !kind_filter.is_empty() && !kind_filter.contains(&other.kind) {
                    continue;
                }

                let is_new_node = !visited_node_ids.contains(&other_id);
                if is_new_node {
                    if nodes_out.len() >= cap {
                        truncated = true;
                        continue;
                    }
                    visited_node_ids.insert(other_id);
                    nodes_out.push((other, hop));
                    next_frontier.push(other_id);
                }
                if visited_edge_ids.insert(edge.id) {
                    edges_out.push(edge);
                }
            }
        }

        frontier = next_frontier;
    }

    Ok(BoundedNeighborhood { nodes: nodes_out, edges: edges_out, truncated })
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
    fn edge_relation_db_str_round_trips_for_every_variant() {
        // `from_db_str` has a catch-all `_ => DependsOn` fallback (for
        // forward-compat with unknown future strings) -- a missing arm for
        // a real variant would silently mislabel it as DependsOn instead of
        // failing loudly, so this test exists specifically to catch that
        // one-line omission class of bug.
        for relation in [EdgeRelation::DependsOn, EdgeRelation::Documents, EdgeRelation::Affects, EdgeRelation::References, EdgeRelation::Covers] {
            assert_eq!(EdgeRelation::from_db_str(relation.as_db_str()), relation);
        }
    }

    /// `NodeKind::from_db_str` has the identical catch-all-fallback shape as
    /// `EdgeRelation::from_db_str` above (`_ => Symbol`) but, until this
    /// test, had no equivalent round-trip guard -- a pre-existing gap
    /// (audit finding E, CLS-inspired retrieval plan), not one introduced
    /// by adding `DocSection`.
    #[test]
    fn node_kind_db_str_round_trips_for_every_variant() {
        for kind in [NodeKind::Symbol, NodeKind::File, NodeKind::Gotcha, NodeKind::Decision, NodeKind::Definition, NodeKind::Note, NodeKind::DocSection] {
            assert_eq!(NodeKind::from_db_str(kind.as_db_str()), kind);
        }
    }

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
    fn effective_edge_weight_uses_the_references_half_life_for_references_edges() {
        // At Affects' 30-day half-life, weight 4.0 would already be halved
        // to 2.0 (see the test above) -- References' longer 90-day
        // half-life must decay far less over the same age.
        let edge = Edge { id: 1, repo: "demo".into(), src_id: 1, dst_id: 2, relation: EdgeRelation::References, weight: 4.0, updated_at: String::new() };
        let decayed = effective_weight_with_half_life(edge.weight, AFFECTS_EDGE_HALF_LIFE_DAYS, REFERENCES_EDGE_HALF_LIFE_DAYS);
        assert!(decayed > 2.0, "References' longer half-life must decay less than Affects' over the same age, got {decayed}");
    }

    #[test]
    fn effective_edge_weight_matches_effective_weight_for_an_affects_edge() {
        let edge = Edge { id: 1, repo: "demo".into(), src_id: 1, dst_id: 2, relation: EdgeRelation::Affects, weight: 3.0, updated_at: format_unix_for_test(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()) };
        let via_dispatch = effective_edge_weight(&edge);
        let via_direct = effective_weight(edge.weight, age_days(&edge.updated_at));
        assert!((via_dispatch - via_direct).abs() < 1e-9, "an Affects edge must decay identically through either function: {via_dispatch} vs {via_direct}");
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

    fn test_symbol(store: &dyn GraphStore, repo: &str, name: &str) -> i64 {
        store
            .add_node(NewNode { kind: NodeKind::Symbol, repo: repo.into(), path: Some(format!("{name}.py")), name: Some(name.into()), container: None, start_line: Some(1), end_line: Some(2), content: Some(name.into()) })
            .unwrap()
    }

    /// The whole point of `upsert_nodes_batch`'s default body calling
    /// `self.find_node`/`self.add_node`/`self.update_node` directly (instead
    /// of the free function `upsert_node`, which needs `&dyn GraphStore`) is
    /// so this compiles and works through a boxed trait object, matching
    /// every real call site in this codebase (`scan.rs::persist` holds its
    /// store as `Box<dyn GraphStore>`, never a concrete generic).
    #[test]
    fn upsert_nodes_batch_is_callable_through_a_boxed_trait_object() {
        let store: Box<dyn GraphStore> = Box::new(SqliteGraphStore::open_in_memory().unwrap());
        let node = NewNode { kind: NodeKind::File, repo: "repo-a".into(), path: Some("a.py".into()), name: None, container: None, start_line: None, end_line: None, content: None };
        let ids = store.upsert_nodes_batch(&[node]).unwrap();
        assert_eq!(ids.len(), 1);
        assert!(store.get_node("repo-a", ids[0]).unwrap().is_some());
    }

    #[test]
    fn upsert_nodes_batch_inserts_new_then_updates_the_same_natural_key_in_place() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let first = NewNode { kind: NodeKind::File, repo: "repo-a".into(), path: Some("a.py".into()), name: None, container: None, start_line: None, end_line: None, content: Some("v1".into()) };
        let ids1 = store.upsert_nodes_batch(&[first]).unwrap();

        let second = NewNode { kind: NodeKind::File, repo: "repo-a".into(), path: Some("a.py".into()), name: None, container: None, start_line: None, end_line: None, content: Some("v2".into()) };
        let ids2 = store.upsert_nodes_batch(&[second]).unwrap();

        assert_eq!(ids1, ids2, "same natural key must preserve the node's id, not create a second row");
        let node = store.get_node("repo-a", ids2[0]).unwrap().unwrap();
        assert_eq!(node.content.as_deref(), Some("v2"), "the second upsert must have updated the existing row's content");
    }

    #[test]
    fn find_nodes_batch_returns_only_keys_that_actually_matched() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        store.add_node(NewNode { kind: NodeKind::File, repo: "repo-a".into(), path: Some("a.py".into()), name: None, container: None, start_line: None, end_line: None, content: None }).unwrap();

        let found = store.find_nodes_batch("repo-a", &[(NodeKind::File, Some("a.py"), None, None), (NodeKind::File, Some("missing.py"), None, None)]).unwrap();
        assert_eq!(found.len(), 1, "only the key that actually matched a node should appear in the result");
        assert!(found.contains_key(&(NodeKind::File, Some("a.py".to_string()), None, None)));
    }

    #[test]
    fn edges_from_batch_keys_results_by_their_own_src_id() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let a = test_symbol(&store, "repo-a", "a");
        let b = test_symbol(&store, "repo-a", "b");
        let c = test_symbol(&store, "repo-a", "c");
        store.add_edge("repo-a", a, b, EdgeRelation::DependsOn).unwrap();

        let result = store.edges_from_batch("repo-a", &[a, c]).unwrap();
        assert_eq!(result.get(&a).map(Vec::len), Some(1));
        assert_eq!(result.get(&c).map(Vec::len), Some(0), "a src_id with no edges must still appear, mapped to an empty Vec");
    }

    /// `bounded_neighborhood` accepts multiple seeds -- new behavior not
    /// exercised by `agentops-api::subgraph::build_subgraph`'s single-seed
    /// callers, since that's the whole reason it was extracted as a shared
    /// primitive for Personalized PageRank's multi-seed activation.
    #[test]
    fn bounded_neighborhood_starts_from_every_seed_in_disjoint_components() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let a = test_symbol(&store, "repo-a", "a");
        let b = test_symbol(&store, "repo-a", "b");
        let c = test_symbol(&store, "repo-a", "c");
        let d = test_symbol(&store, "repo-a", "d");
        store.add_edge("repo-a", a, b, EdgeRelation::DependsOn).unwrap();
        store.add_edge("repo-a", c, d, EdgeRelation::DependsOn).unwrap();

        let result = bounded_neighborhood(&store, "repo-a", NeighborhoodQuery { seed_ids: &[a, c], relations: &[EdgeRelation::DependsOn], direction: TraversalDirection::Outgoing, max_depth: 2, kind_filter: &[], cap: 150 }).unwrap();
        let ids: std::collections::HashSet<i64> = result.nodes.iter().map(|(n, _)| n.id).collect();
        assert_eq!(ids, std::collections::HashSet::from([a, b, c, d]), "both disjoint components must be reached, one per seed");
        assert!(!result.truncated);
    }

    #[test]
    fn bounded_neighborhood_deduplicates_a_seed_listed_twice() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let a = test_symbol(&store, "repo-a", "a");

        let result = bounded_neighborhood(&store, "repo-a", NeighborhoodQuery { seed_ids: &[a, a], relations: &[EdgeRelation::DependsOn], direction: TraversalDirection::Both, max_depth: 2, kind_filter: &[], cap: 150 }).unwrap();
        assert_eq!(result.nodes.len(), 1, "a duplicate seed id must not produce a duplicate node entry");
    }

    #[test]
    fn bounded_neighborhood_respects_cap_across_multiple_seeds() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let hub_a = test_symbol(&store, "repo-a", "hub_a");
        let hub_b = test_symbol(&store, "repo-a", "hub_b");
        for i in 0..10 {
            let leaf = test_symbol(&store, "repo-a", &format!("leaf_a{i}"));
            store.add_edge("repo-a", hub_a, leaf, EdgeRelation::DependsOn).unwrap();
        }
        for i in 0..10 {
            let leaf = test_symbol(&store, "repo-a", &format!("leaf_b{i}"));
            store.add_edge("repo-a", hub_b, leaf, EdgeRelation::DependsOn).unwrap();
        }

        let result = bounded_neighborhood(&store, "repo-a", NeighborhoodQuery { seed_ids: &[hub_a, hub_b], relations: &[EdgeRelation::DependsOn], direction: TraversalDirection::Outgoing, max_depth: 2, kind_filter: &[], cap: 5 }).unwrap();
        assert_eq!(result.nodes.len(), 5, "the shared cap must bound the total across every seed's expansion combined");
        assert!(result.truncated);
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

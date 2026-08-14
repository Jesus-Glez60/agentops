//! Domain entities for docbrain's neuron/synapse doc graph, and the
//! `DocbrainStore` port (trait only — the SQLite adapter lives in
//! `sqlite.rs`, kept deliberately separate per the hexagonal architecture
//! guide; `main`'s `DocbrainStore` was a bare struct with no trait at all,
//! worse than `agentops-graph`'s already-flagged port/adapter blur).
//!
//! `DocNode` (neuron) is a token-bounded chunk of documentation; `DocEdge`
//! (synapse) is a typed relation between two nodes (`Sequence`,
//! `ParentSection`, `CrossReference`) that lets retrieval walk to
//! directly-connected context instead of returning a flat top-k of isolated
//! chunks. See ~/.claude/plans/let-s-start-module-1-robust-hummingbird.md
//! for the full design rationale.
//!
//! Single-tenant for this pass — `main`'s `TenantContext`/`Visibility`
//! multi-org isolation is real, useful behavior but out of scope for "build
//! the whole system, figure out gatekeeping later" (see the vnext planning
//! session's explicit no-tiering-for-now decision).

mod sqlite;

pub use sqlite::{content_hash, SqliteDocbrainStore, EMBEDDING_DIM};

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Library {
    pub id: i64,
    pub slug: String,
    pub name: String,
    /// Fetched during discovery (npm/PyPI/GitHub metadata) and persisted —
    /// previously discarded at every call site despite `DiscoveredLibrary`
    /// already carrying it.
    pub description: Option<String>,
    pub github_repo: Option<String>,
    pub docs_url: Option<String>,
    pub doc_snapshots: i64,
    pub changelog_versions: i64,
    /// Distinct versions with at least one doc_snapshot, ascending.
    pub versions: Vec<String>,
    pub total_nodes: i64,
    /// Count of distinct repos with a `repo_library_versions` row for this
    /// library — computed, not stored, so both list and detail views get
    /// "used in N repos" without a second per-library query.
    pub used_in_count: i64,
    /// `MAX(doc_snapshots.scraped_at)` — `None` if never scraped.
    pub last_indexed_at: Option<String>,
}

/// One repo's declared dependency on a library, from manifest parsing
/// (`agentops_scanner::extract_declared_dependencies`), not import scanning.
/// `repo_identifier` is a plain path/name string -- no FK, since a repo's
/// own graph lives in an entirely separate SQLite/Postgres store from
/// docbrain's.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoLibraryUsage {
    pub repo_identifier: String,
    pub declared_version: String,
    pub updated_at: String,
}

/// What a neuron actually holds. `CodeExample` nodes are split out of their
/// surrounding prose (see `chunk.rs`'s `extract_code_blocks`) so a
/// planning-stage query can retrieve the explanatory text without also
/// paying tokens for code it doesn't need yet, and an implementation-stage
/// query can fetch exactly the code for a prose node it already found via
/// [`EdgeRelation::HasExample`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeKind {
    Prose,
    CodeExample,
}

impl NodeKind {
    fn as_db_str(&self) -> &'static str {
        match self {
            NodeKind::Prose => "prose",
            NodeKind::CodeExample => "code_example",
        }
    }

    fn from_db_str(s: &str) -> Self {
        match s {
            "code_example" => NodeKind::CodeExample,
            _ => NodeKind::Prose,
        }
    }
}

/// One neuron: a token-bounded chunk of documentation, not a whole page or
/// section. `content_hash` is the dedup key within `(library_id, version)` —
/// re-scraping only inserts nodes whose hash is new (see `chunk.rs` in
/// `docbrain-ingest` for how it's computed).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocNode {
    pub id: i64,
    pub library_id: i64,
    pub version: String,
    pub topic: String,
    pub content: String,
    pub content_hash: String,
    pub token_count: i64,
    pub kind: NodeKind,
}

/// Fields needed to upsert one node; `id`/`library_id` are assigned by the
/// store.
#[derive(Debug, Clone)]
pub struct NewDocNode<'a> {
    pub version: &'a str,
    pub topic: &'a str,
    pub content: &'a str,
    pub content_hash: &'a str,
    pub token_count: i64,
    pub embedding: Option<&'a [f32]>,
    pub kind: NodeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpsertOutcome {
    Inserted(i64),
    AlreadyExists(i64),
}

impl UpsertOutcome {
    pub fn node_id(&self) -> i64 {
        match self {
            UpsertOutcome::Inserted(id) | UpsertOutcome::AlreadyExists(id) => *id,
        }
    }
}

/// A synapse: a typed relation between two neurons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EdgeRelation {
    /// `from_node` is the chunk immediately before `to_node` on the same
    /// scraped page — lets a hit pull its neighbor for continuity.
    Sequence,
    /// `from_node` (a subsection) rolls up to `to_node` (its parent
    /// heading) — lets a hit pull the framing context above it.
    ParentSection,
    /// An explicit in-page link from `from_node` to `to_node`.
    CrossReference,
    /// `from_node` (prose) has a code example at `to_node` (`CodeExample`
    /// kind) — the planning/implementation split's actual wiring.
    HasExample,
}

impl EdgeRelation {
    fn as_db_str(&self) -> &'static str {
        match self {
            EdgeRelation::Sequence => "sequence",
            EdgeRelation::ParentSection => "parent_section",
            EdgeRelation::CrossReference => "cross_reference",
            EdgeRelation::HasExample => "has_example",
        }
    }

    fn from_db_str(s: &str) -> Self {
        match s {
            "parent_section" => EdgeRelation::ParentSection,
            "cross_reference" => EdgeRelation::CrossReference,
            "has_example" => EdgeRelation::HasExample,
            _ => EdgeRelation::Sequence,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocEdge {
    pub id: i64,
    pub from_node: i64,
    pub to_node: i64,
    pub relation: EdgeRelation,
}

/// A hit from `search_similar`, still attached to its distance so callers
/// can decide how much edge-walked context they can afford within a token
/// budget.
#[derive(Debug, Clone)]
pub struct SearchHit {
    pub node: DocNode,
    pub distance: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangelogEntry {
    pub id: i64,
    pub library_id: i64,
    pub from_version: String,
    pub to_version: String,
    pub entry: String,
    pub breaking: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobStatus {
    Running,
    Succeeded,
    Failed,
}

impl JobStatus {
    fn as_db_str(&self) -> &'static str {
        match self {
            JobStatus::Running => "running",
            JobStatus::Succeeded => "succeeded",
            JobStatus::Failed => "failed",
        }
    }

    fn from_db_str(s: &str) -> Self {
        match s {
            "succeeded" => JobStatus::Succeeded,
            "failed" => JobStatus::Failed,
            _ => JobStatus::Running,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: i64,
    pub job_type: String,
    pub slug: String,
    pub status: JobStatus,
    pub message: Option<String>,
}

/// The docbrain graph port. One adapter today (`SqliteDocbrainStore`); the
/// point of the trait is that a future adapter (a hosted/multi-tenant
/// backend, if gatekeeping is added later) can implement this without
/// touching any use-case or MCP-handler code.
pub trait DocbrainStore {
    fn add_library(&self, slug: &str, name: &str, description: Option<&str>, github_repo: Option<&str>, docs_url: Option<&str>) -> Result<i64>;
    fn get_library(&self, slug: &str) -> Result<Option<Library>>;
    fn list_libraries(&self) -> Result<Vec<Library>>;

    /// Upserts the declared version for `(repo_identifier, slug)` — keyed
    /// on that pair, so re-running `sync_docs` for the same repo updates
    /// the existing row rather than duplicating it. Errors if `slug` isn't
    /// a registered library (call after registration/discovery, not before).
    fn upsert_repo_library_version(&self, repo_identifier: &str, slug: &str, declared_version: &str) -> Result<()>;
    /// Every repo with a recorded declared version for `slug`, ordered by
    /// `repo_identifier`. Mismatch against the library's latest indexed
    /// version is derived by the caller at read time, not stored here.
    fn repos_using_library(&self, slug: &str) -> Result<Vec<RepoLibraryUsage>>;

    fn add_doc_snapshot(&self, slug: &str, version: &str) -> Result<i64>;

    /// Upserts one node keyed on `(library, version, content_hash)` — the
    /// dedup-on-rescrape fix. Returns whether it was newly inserted or
    /// already existed, so ingestion can skip embedding/edge-construction
    /// work for unchanged chunks.
    fn upsert_doc_node(&self, slug: &str, node: NewDocNode) -> Result<UpsertOutcome>;
    fn get_node(&self, node_id: i64) -> Result<Option<DocNode>>;
    fn get_doc_nodes(&self, slug: &str, version: &str) -> Result<Vec<DocNode>>;
    fn all_doc_nodes(&self, slug: &str) -> Result<Vec<DocNode>>;
    fn list_doc_versions(&self, slug: &str) -> Result<Vec<String>>;

    fn add_doc_edge(&self, from_node: i64, to_node: i64, relation: EdgeRelation) -> Result<()>;
    /// Every edge touching `node_id`, either direction.
    fn node_edges(&self, node_id: i64) -> Result<Vec<DocEdge>>;

    /// KNN search over node embeddings, optionally scoped to one library and
    /// excluding one `NodeKind` (docbrain-mcp's `search_docs` excludes
    /// `CodeExample` by default — the planning/implementation split).
    /// Returns at most `top_k` hits, nearest first.
    fn search_similar(&self, embedding: &[f32], top_k: usize, slug: Option<&str>, exclude_kind: Option<NodeKind>) -> Result<Vec<SearchHit>>;

    /// Idempotent on `(library, from_version, to_version)` — re-syncing
    /// never duplicates a row (the confirmed gap in `main`'s plain `INSERT`).
    fn add_changelog_entry(&self, slug: &str, from_version: &str, to_version: &str, entry: &str, breaking: bool) -> Result<i64>;
    fn get_changelog(&self, slug: &str, from_version: &str, to_version: &str) -> Result<Vec<ChangelogEntry>>;
    /// Every synced entry for `slug`, most recent first (`id DESC` — entries
    /// are inserted in the order `sync_changelogs` fetched them, newest
    /// release first). Unlike `get_changelog`, not scoped to one exact
    /// `(from_version, to_version)` pair — real GitHub release tags (what
    /// changelog entries are keyed on) and a library's coarser
    /// doc-snapshot versions (what `scrape_library` was told to fetch) are
    /// different granularities with no guaranteed exact match, so a UI
    /// showing "the changelog for this library" needs the full history,
    /// not a lookup that silently returns nothing for a mismatched pair.
    fn list_changelog(&self, slug: &str) -> Result<Vec<ChangelogEntry>>;

    fn create_job(&self, job_type: &str, slug: &str) -> Result<i64>;
    fn complete_job(&self, id: i64, status: JobStatus, message: &str) -> Result<()>;
    fn get_job(&self, id: i64) -> Result<Option<Job>>;
}

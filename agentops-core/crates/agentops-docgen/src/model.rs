//! Structured, JSON-serializable representation of a repo's documentation
//! page — the sectioned counterpart to `lib.rs`'s flat-Markdown
//! `render_onboarding_doc`. Produced by `sectioned::build_doc_page`,
//! consumed by `agentops-api`'s `/repos/{name}/docs` endpoint and the
//! frontend's three-pane Documentation Viewer.
//!
//! `Serialize`-only (no `Deserialize`) -- matching `agentops-api`'s
//! `RepoSummary` convention, since this type is only ever produced here and
//! read back as raw JSON text by callers, never reconstructed from JSON in
//! Rust (see `sectioned.rs`'s persistence note).

use agentops_graph::NodeKind;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct DocPage {
    pub repo: String,
    pub generated_at: String,
    pub node_count: i64,
    pub sections: Vec<DocSection>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DocSection {
    /// Stable slug used for the frontend nav href and TOC anchor (e.g.
    /// `"core-modules-auth"`) -- stable across regenerations as long as the
    /// underlying grouping doesn't change, so a bookmarked/shared link into
    /// a specific section keeps working.
    pub id: String,
    pub group: DocGroup,
    pub title: String,
    pub blocks: Vec<DocBlock>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DocGroup {
    Repository,
    CoreModules,
    Knowledge,
    Setup,
    // Deliberately no `ExecutionFlows` variant for v1 -- no signal in the
    // graph derives a call-chain "flow" yet; see the plan this shipped
    // against. Add it here (and a corresponding nav group in the frontend)
    // once flow-detection is real, rather than shipping an always-empty one.
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "block_type", rename_all = "snake_case")]
pub enum DocBlock {
    Prose {
        markdown: String,
    },
    SymbolTable {
        file: String,
        rows: Vec<SymbolRow>,
    },
    DependencyChips {
        deps: Vec<String>,
    },
    KnowledgeCallout {
        kind: NodeKind,
        node_id: i64,
        title: String,
        body: String,
        /// Human-readable "affects X()" / "applies to Y" attribution shown
        /// in the callout footer.
        affects: String,
        /// `(path, line)` of the affected symbol/file, when known -- used
        /// for the "src/auth/session.ts:45" deep link.
        source: Option<(String, i64)>,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct SymbolRow {
    pub name: String,
    pub one_liner: String,
    pub gotcha_count: i64,
    pub node_id: i64,
}

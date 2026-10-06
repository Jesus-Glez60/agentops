//! Structured, JSON-serializable representation of a repo's documentation
//! page — the sectioned counterpart to `lib.rs`'s flat-Markdown
//! `render_onboarding_doc`. Produced by `sectioned::build_doc_page`,
//! consumed by `agentops-api`'s `/repos/{name}/docs` endpoint and the
//! frontend's three-pane Documentation Viewer.
//!
//! Also `Deserialize` (added alongside the original `Serialize`-only
//! design): `agentops-mcp`'s blueprint-persistence tool needs to read back
//! an already-persisted page, merge in an agent-authored `Blueprint`
//! section, and re-save it. This doesn't reopen the crate-cycle the
//! original design avoided -- `GraphStore::save_doc_page`/`get_doc_page`
//! (in `agentops-graph`) still take/return an opaque JSON string, so
//! `agentops-graph` still never depends on this crate. The parse-back-into-
//! `DocPage` step happens entirely inside `agentops-mcp`, which already
//! depends on both `agentops-graph` and this crate.

use agentops_graph::NodeKind;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocPage {
    pub repo: String,
    pub generated_at: String,
    pub node_count: i64,
    pub sections: Vec<DocSection>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocGroup {
    Repository,
    CoreModules,
    Knowledge,
    Setup,
    /// Agent-authored planning docs (PRD, Architecture, Security Guide,
    /// etc., see the `project-blueprint` skill) -- never produced by
    /// `build_doc_page` itself, only merged in by `agentops-mcp`'s
    /// `persist_blueprint_doc` tool via `upsert_blueprint_section`. Because
    /// `build_doc_page` never emits this group, `persist_doc_page`'s
    /// preserve-on-rescan merge can always re-append a previous page's
    /// `Blueprint` sections without any risk of colliding with a freshly
    /// built section's id.
    Blueprint,
    // Deliberately no `ExecutionFlows` variant for v1 -- no signal in the
    // graph derives a call-chain "flow" yet; see the plan this shipped
    // against. Add it here (and a corresponding nav group in the frontend)
    // once flow-detection is real, rather than shipping an always-empty one.
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolRow {
    pub name: String,
    pub one_liner: String,
    pub gotcha_count: i64,
    pub node_id: i64,
}

impl DocSection {
    /// Flattens this section's blocks into plain searchable text, plus the
    /// distinct node ids it covers (from `SymbolTable` rows and
    /// `KnowledgeCallout`s) — used by `agentops-mcp::scan::persist` to index
    /// each section as its own searchable `NodeKind::DocSection` node
    /// (Initiative 2, CLS-inspired retrieval plan: docgen's already-compressed
    /// "gist" tier was previously write-only, invisible to search). The
    /// returned ids feed `EdgeRelation::Covers` edges, not `Documents` — see
    /// that variant's doc comment for why they must stay distinct.
    pub fn search_text_and_covered_ids(&self) -> (String, Vec<i64>) {
        let mut text = String::new();
        let mut ids = Vec::new();
        for block in &self.blocks {
            match block {
                DocBlock::Prose { markdown } => {
                    text.push_str(markdown);
                    text.push('\n');
                }
                DocBlock::SymbolTable { file, rows } => {
                    text.push_str(file);
                    text.push('\n');
                    for row in rows {
                        text.push_str(&row.name);
                        text.push(' ');
                        text.push_str(&row.one_liner);
                        text.push('\n');
                        ids.push(row.node_id);
                    }
                }
                DocBlock::DependencyChips { deps } => {
                    text.push_str(&deps.join(", "));
                    text.push('\n');
                }
                DocBlock::KnowledgeCallout { node_id, title, body, affects, .. } => {
                    text.push_str(title);
                    text.push(' ');
                    text.push_str(body);
                    text.push(' ');
                    text.push_str(affects);
                    text.push('\n');
                    ids.push(*node_id);
                }
            }
        }
        ids.sort_unstable();
        ids.dedup();
        (text, ids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn section(blocks: Vec<DocBlock>) -> DocSection {
        DocSection { id: "s".into(), group: DocGroup::CoreModules, title: "Section".into(), blocks }
    }

    #[test]
    fn flattens_every_block_kind_into_searchable_text() {
        let sec = section(vec![
            DocBlock::Prose { markdown: "some prose here".into() },
            DocBlock::SymbolTable { file: "auth.rs".into(), rows: vec![SymbolRow { name: "verify_token".into(), one_liner: "checks a token".into(), gotcha_count: 0, node_id: 1 }] },
            DocBlock::DependencyChips { deps: vec!["serde".into(), "tokio".into()] },
            DocBlock::KnowledgeCallout { kind: NodeKind::Gotcha, node_id: 2, title: "watch out".into(), body: "edge case here".into(), affects: "affects verify_token".into(), source: None },
        ]);
        let (text, ids) = sec.search_text_and_covered_ids();
        for expected in ["some prose here", "verify_token", "checks a token", "auth.rs", "serde", "tokio", "watch out", "edge case here"] {
            assert!(text.contains(expected), "missing {expected:?} in flattened text: {text:?}");
        }
        assert_eq!(ids, vec![1, 2]);
    }

    #[test]
    fn covered_ids_are_deduplicated_and_sorted() {
        let sec = section(vec![
            DocBlock::SymbolTable { file: "a.rs".into(), rows: vec![SymbolRow { name: "b".into(), one_liner: String::new(), gotcha_count: 0, node_id: 5 }] },
            DocBlock::KnowledgeCallout { kind: NodeKind::Gotcha, node_id: 5, title: String::new(), body: String::new(), affects: String::new(), source: None },
            DocBlock::KnowledgeCallout { kind: NodeKind::Gotcha, node_id: 1, title: String::new(), body: String::new(), affects: String::new(), source: None },
        ]);
        let (_, ids) = sec.search_text_and_covered_ids();
        assert_eq!(ids, vec![1, 5]);
    }

    #[test]
    fn a_prose_only_section_covers_no_ids() {
        let sec = section(vec![DocBlock::Prose { markdown: "just prose, no symbols".into() }]);
        let (_, ids) = sec.search_text_and_covered_ids();
        assert!(ids.is_empty());
    }

    /// `DocPage` round-trips through `Deserialize` (added for
    /// `agentops-mcp`'s blueprint-persistence tool, which needs to read
    /// back an already-persisted page) -- including the `Blueprint`
    /// variant, which `build_doc_page` itself never produces.
    #[test]
    fn doc_page_round_trips_through_serialize_deserialize() {
        let page = DocPage {
            repo: "my-repo".into(),
            generated_at: "2026-01-01T00:00:00Z".into(),
            node_count: 3,
            sections: vec![DocSection {
                id: "blueprint-prd".into(),
                group: DocGroup::Blueprint,
                title: "Product Requirements".into(),
                blocks: vec![
                    DocBlock::Prose { markdown: "plain-language PRD".into() },
                    DocBlock::KnowledgeCallout { kind: NodeKind::Note, node_id: 7, title: "Our PRD".into(), body: "Status: Accepted".into(), affects: String::new(), source: None },
                ],
            }],
        };
        let json = serde_json::to_string(&page).unwrap();
        let round_tripped: DocPage = serde_json::from_str(&json).unwrap();
        assert_eq!(round_tripped.repo, page.repo);
        assert_eq!(round_tripped.sections.len(), 1);
        assert_eq!(round_tripped.sections[0].group, DocGroup::Blueprint);
    }
}

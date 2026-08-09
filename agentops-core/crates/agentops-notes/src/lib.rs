//! Gotcha/decision node CRUD, edge-connected into the code graph via
//! `agentops-graph` — Codebrain-2: "every documented error, bug, or workaround
//! becomes its own node, edge-connected to the specific feature/component/method
//! it affects."

use agentops_graph::{EdgeRelation, GraphStore, NewNode, NodeKind};
use anyhow::Result;

mod vault;
pub use vault::{connect_many, ingest_vault, match_symbols, walk_vault, IngestSummary, NoteType, SymbolMatcher, VaultNote, WordBoundaryMatcher};

/// What a new gotcha/decision note should attach to, if anything.
pub enum AffectsTarget<'a> {
    /// A specific node id (fastest, exact — e.g. from a prior `docgen`/`graph`
    /// query result).
    NodeId(i64),
    /// A symbol name to resolve by best-effort match against indexed symbols.
    /// Returns the first match; ambiguous names resolve to whichever symbol
    /// was indexed first, which is a known Phase 1 simplification.
    SymbolName(&'a str),
    None,
}

/// Records a gotcha (bug/workaround) node, optionally edge-connected to the
/// symbol it affects.
pub fn add_gotcha(store: &dyn GraphStore, repo: &str, title: &str, text: &str, affects: AffectsTarget) -> Result<i64> {
    add_note(store, NodeKind::Gotcha, repo, title, text, affects)
}

/// Records a decision (architectural choice) node, optionally edge-connected to
/// the symbol/component it applies to.
pub fn add_decision(store: &dyn GraphStore, repo: &str, title: &str, text: &str, affects: AffectsTarget) -> Result<i64> {
    add_note(store, NodeKind::Decision, repo, title, text, affects)
}

fn add_note(
    store: &dyn GraphStore,
    kind: NodeKind,
    repo: &str,
    title: &str,
    text: &str,
    affects: AffectsTarget,
) -> Result<i64> {
    let note_id = store.add_node(NewNode {
        kind,
        repo: repo.to_string(),
        path: None,
        name: Some(title.to_string()),
        start_line: None,
        end_line: None,
        content: Some(text.to_string()),
    })?;

    let target_id = match affects {
        AffectsTarget::NodeId(id) => Some(id),
        AffectsTarget::SymbolName(name) => resolve_symbol_by_name(store, repo, name)?,
        AffectsTarget::None => None,
    };

    if let Some(target_id) = target_id {
        store.add_edge(note_id, target_id, EdgeRelation::Affects)?;
    }

    Ok(note_id)
}

/// Resolves `name` to a symbol id within `repo` only, first match wins on
/// an ambiguous name within that repo — the documented Phase 1
/// simplification `AffectsTarget::SymbolName` accepts. Fixed to filter by
/// `repo` (it previously didn't at all): without this, a same-named symbol
/// in a *different* repo's data could silently be matched, since
/// `nodes_by_kind` returns every repo's symbols. Still not fully
/// disambiguated (no `path` filter) — for a caller that already knows
/// exactly which symbol it means, prefer `AffectsTarget::NodeId` (or, for
/// the vault-ingestion/`explain_symbol` paths, `agentops_llm::find_symbol_by_name`
/// / `match_symbols`, both of which are repo *and* path-aware) instead of
/// widening this function further.
fn resolve_symbol_by_name(store: &dyn GraphStore, repo: &str, name: &str) -> Result<Option<i64>> {
    let symbols = store.nodes_by_kind(NodeKind::Symbol)?;
    Ok(symbols.into_iter().find(|s| s.repo == repo && s.name.as_deref() == Some(name)).map(|s| s.id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentops_graph::SqliteGraphStore;

    #[test]
    fn gotcha_resolves_and_connects_to_symbol_by_name() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let symbol_id = store
            .add_node(NewNode {
                kind: NodeKind::Symbol,
                repo: "demo".into(),
                path: Some("src/auth.rs".into()),
                name: Some("verify_token".into()),
                start_line: Some(1),
                end_line: Some(5),
                content: Some("fn verify_token() {}".into()),
            })
            .unwrap();

        let gotcha_id = add_gotcha(
            &store,
            "demo",
            "token-expiry-off-by-one",
            "Expiry check was off by one day.",
            AffectsTarget::SymbolName("verify_token"),
        )
        .unwrap();

        let edges = store.edges_from(gotcha_id).unwrap();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].dst_id, symbol_id);
        assert_eq!(edges[0].relation, EdgeRelation::Affects);
    }

    #[test]
    fn gotcha_with_unresolvable_name_is_still_recorded_without_an_edge() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let gotcha_id = add_gotcha(&store, "demo", "orphan", "no matching symbol", AffectsTarget::SymbolName("nope")).unwrap();
        assert!(store.edges_from(gotcha_id).unwrap().is_empty());
        assert!(store.get_node(gotcha_id).unwrap().is_some());
    }

    #[test]
    fn affects_symbol_name_never_matches_a_same_named_symbol_in_a_different_repo() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        store
            .add_node(NewNode {
                kind: NodeKind::Symbol,
                repo: "other-repo".into(),
                path: Some("src/auth.rs".into()),
                name: Some("verify_token".into()),
                start_line: Some(1),
                end_line: Some(5),
                content: Some("fn verify_token() {}".into()),
            })
            .unwrap();

        let gotcha_id = add_gotcha(&store, "demo", "wrong-repo-orphan", "should not connect", AffectsTarget::SymbolName("verify_token")).unwrap();

        assert!(store.edges_from(gotcha_id).unwrap().is_empty(), "a same-named symbol in a different repo must not be matched");
    }

    #[test]
    fn decision_records_without_requiring_a_target() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let id = add_decision(&store, "demo", "use-sqlite-for-light-tier", "Chose SQLite over Postgres for the light tier.", AffectsTarget::None).unwrap();
        let node = store.get_node(id).unwrap().unwrap();
        assert_eq!(node.kind, NodeKind::Decision);
    }
}

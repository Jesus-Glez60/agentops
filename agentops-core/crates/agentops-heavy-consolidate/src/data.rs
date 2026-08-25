//! Training-data generation from the graph's own curated knowledge
//! (Initiative 6, CLS-inspired retrieval plan): one `(prompt, target)`
//! instruction pair per `Gotcha`/`Decision` note's `Affects` edge to a
//! symbol, where the prompt is the symbol's code plus pattern-completed
//! context (reusing Initiative 4's `pattern_complete`, not reinventing
//! context assembly) and the target is the note's own human-curated text
//! -- real, already-reviewed data, never auto-generated filler.

use agentops_embeddings::Embedder;
use agentops_graph::{effective_edge_weight, prominence_rank_multiplier, EdgeRelation, GraphStore, Node, NodeKind};
use anyhow::Result;

/// How many pattern-completed related symbols get folded into each
/// example's prompt -- mirrors `agentops-llm::explain_symbol`'s own
/// `PATTERN_COMPLETE_K`, for the same "generous, since most get filtered
/// by having no notes anyway" reasoning.
const PATTERN_COMPLETE_K: usize = 5;

#[derive(Debug, Clone)]
pub struct TrainingExample {
    pub prompt: String,
    pub target: String,
    /// Salience weight -- `effective_edge_weight` (the note's own
    /// plasticity signal) damped by `prominence_rank_multiplier` (a
    /// curated-down gotcha still trains on, just with less influence,
    /// same convention curation uses everywhere else in this codebase).
    pub weight: f64,
}

/// ChatML-style formatting (Qwen2/Qwen2.5's native instruction format) --
/// hand-constructed rather than parsed from the tokenizer's own chat
/// template, to avoid a template-engine dependency for what's a fixed,
/// simple three-turn shape.
fn build_prompt(symbol: &Node, related_context: &[agentops_retrieval::PatternCompletionMatch]) -> String {
    let mut user_turn = format!(
        "You are documenting a codebase. Explain concisely what this symbol does and why it might exist.\n\nSymbol: {}\nFile: {}\n\n```\n{}\n```\n",
        symbol.name.as_deref().unwrap_or("<unnamed>"),
        symbol.path.as_deref().unwrap_or("<unknown>"),
        symbol.content.as_deref().unwrap_or(""),
    );

    let with_notes: Vec<&agentops_retrieval::PatternCompletionMatch> = related_context.iter().filter(|m| !m.notes.is_empty()).collect();
    if !with_notes.is_empty() {
        user_turn.push_str("\nPossibly related context from similar symbols elsewhere in this repo:\n");
        for m in with_notes {
            for (kind, title, text, _, _) in &m.notes {
                user_turn.push_str(&format!("- [{kind:?}] {title}: {text}\n"));
            }
        }
    }

    format!("<|im_start|>system\nYou are codebrain, this repository's own local assistant.<|im_end|>\n<|im_start|>user\n{user_turn}<|im_end|>\n<|im_start|>assistant\n")
}

/// Generates up to `max_examples` training examples from `repo`'s curated
/// `Gotcha`/`Decision` notes, highest-salience first.
pub fn generate_examples(store: &dyn GraphStore, embedder: &dyn Embedder, repo: &str, max_examples: usize) -> Result<Vec<TrainingExample>> {
    let mut examples = Vec::new();

    for kind in [NodeKind::Gotcha, NodeKind::Decision] {
        for note in store.nodes_by_kind(repo, kind)? {
            let Some(note_text) = &note.content else { continue };
            for edge in store.edges_from(repo, note.id)? {
                if edge.relation != EdgeRelation::Affects {
                    continue;
                }
                let Some(symbol) = store.get_node(repo, edge.dst_id)? else { continue };
                if symbol.kind != NodeKind::Symbol || symbol.content.is_none() {
                    continue;
                }

                // Pattern-completed context (Initiative 4 reuse) -- a
                // failure here (e.g. the symbol was never embedded) just
                // means an empty related-context section, not a dropped
                // example; the symbol's own code and this note's own text
                // are still a valid, complete training pair without it.
                let related = agentops_retrieval::pattern_complete(store, embedder, repo, symbol.id, PATTERN_COMPLETE_K).unwrap_or_default();

                let target = match &note.name {
                    Some(title) => format!("{title}: {note_text}"),
                    None => note_text.clone(),
                };
                let weight = effective_edge_weight(&edge) * prominence_rank_multiplier(note.prominence);

                examples.push(TrainingExample { prompt: build_prompt(&symbol, &related), target, weight });
            }
        }
    }

    examples.sort_by(|a, b| b.weight.partial_cmp(&a.weight).unwrap_or(std::cmp::Ordering::Equal));
    examples.truncate(max_examples);
    Ok(examples)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentops_embeddings::LocalEmbedder;
    use agentops_graph::{upsert_node, NewNode, NodeProminence, SqliteGraphStore};

    fn symbol(store: &dyn GraphStore, repo: &str, name: &str, content: &str) -> i64 {
        upsert_node(store, NewNode { kind: NodeKind::Symbol, repo: repo.into(), path: Some(format!("{name}.rs")), name: Some(name.into()), container: None, start_line: Some(1), end_line: Some(2), content: Some(content.into()) }).unwrap()
    }

    #[test]
    fn generate_examples_pairs_a_gotcha_with_the_symbol_it_affects() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let sym = symbol(&store, "demo", "verify_token", "fn verify_token() { /* impl */ }");
        let gotcha = upsert_node(&store, NewNode { kind: NodeKind::Gotcha, repo: "demo".into(), path: None, name: Some("Token bug".into()), container: None, start_line: None, end_line: None, content: Some("has a known workaround".into()) }).unwrap();
        store.add_edge("demo", gotcha, sym, EdgeRelation::Affects).unwrap();

        let examples = generate_examples(&store, &LocalEmbedder, "demo", 10).unwrap();
        assert_eq!(examples.len(), 1);
        assert!(examples[0].prompt.contains("verify_token"));
        assert!(examples[0].target.contains("Token bug"));
        assert!(examples[0].target.contains("has a known workaround"));
    }

    #[test]
    fn generate_examples_damps_weight_for_a_reduced_prominence_gotcha() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let sym_full = symbol(&store, "demo", "full_sym", "fn full_sym() {}");
        let sym_reduced = symbol(&store, "demo", "reduced_sym", "fn reduced_sym() {}");
        let full_gotcha = upsert_node(&store, NewNode { kind: NodeKind::Gotcha, repo: "demo".into(), path: None, name: Some("full".into()), container: None, start_line: None, end_line: None, content: Some("full note".into()) }).unwrap();
        let reduced_gotcha = upsert_node(&store, NewNode { kind: NodeKind::Gotcha, repo: "demo".into(), path: None, name: Some("reduced".into()), container: None, start_line: None, end_line: None, content: Some("reduced note".into()) }).unwrap();
        store.add_edge("demo", full_gotcha, sym_full, EdgeRelation::Affects).unwrap();
        store.add_edge("demo", reduced_gotcha, sym_reduced, EdgeRelation::Affects).unwrap();
        store.set_curation("demo", reduced_gotcha, NodeProminence::Reduced, Some("niche")).unwrap();

        let examples = generate_examples(&store, &LocalEmbedder, "demo", 10).unwrap();
        let full = examples.iter().find(|e| e.target.contains("full note")).unwrap();
        let reduced = examples.iter().find(|e| e.target.contains("reduced note")).unwrap();
        assert!(full.weight > reduced.weight, "a Reduced-prominence gotcha must train with less weight: {full:?} vs {reduced:?}");
    }

    #[test]
    fn generate_examples_ignores_decisions_and_gotchas_with_no_affects_edge() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        upsert_node(&store, NewNode { kind: NodeKind::Gotcha, repo: "demo".into(), path: None, name: Some("orphan".into()), container: None, start_line: None, end_line: None, content: Some("no symbol connected".into()) }).unwrap();

        let examples = generate_examples(&store, &LocalEmbedder, "demo", 10).unwrap();
        assert!(examples.is_empty());
    }

    #[test]
    fn generate_examples_respects_max_examples_keeping_highest_weight() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        for i in 0..5 {
            let sym = symbol(&store, "demo", &format!("sym{i}"), &format!("fn sym{i}() {{}}"));
            let gotcha = upsert_node(&store, NewNode { kind: NodeKind::Gotcha, repo: "demo".into(), path: None, name: Some(format!("g{i}")), container: None, start_line: None, end_line: None, content: Some(format!("note {i}")) }).unwrap();
            let edge_id = store.add_edge("demo", gotcha, sym, EdgeRelation::Affects).unwrap();
            for _ in 0..i {
                store.reinforce_edge("demo", edge_id, true).unwrap();
            }
        }

        let examples = generate_examples(&store, &LocalEmbedder, "demo", 2).unwrap();
        assert_eq!(examples.len(), 2);
        assert!(examples[0].target.contains("note 4"));
        assert!(examples[1].target.contains("note 3"));
    }
}

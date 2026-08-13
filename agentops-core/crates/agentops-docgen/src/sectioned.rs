//! Sectioned, structured counterpart to `lib.rs`'s flat-Markdown
//! `render_onboarding_doc` — produces a `DocPage` the frontend's
//! Documentation Viewer renders as a three-pane docs site instead of one
//! long Markdown blob.
//!
//! Deliberately still LLM-free (see this crate's doc comment on the
//! network-boundary philosophy `render_onboarding_doc` already
//! established): `module_labels` is plain data the caller computed
//! elsewhere (LLM-assisted via `agentops-llm::group_core_modules`, or
//! nothing at all) — this function never calls out itself, and falls back
//! to a directory-name heuristic internally when given none.

use std::collections::BTreeMap;
use std::path::PathBuf;

use agentops_graph::{EdgeRelation, GraphStore, ModuleLabel, NodeKind};

use crate::model::{DocBlock, DocGroup, DocPage, DocSection, SymbolRow};
use crate::{gotchas_affecting, sort_notes_by_weight};

/// Builds `repo_name`'s Documentation Viewer page. `ranked_paths` should be
/// `agentops_scanner::rank_files`'s output, highest-ranked first (same
/// contract as `render_onboarding_doc`). `module_labels`, if non-empty, is
/// used verbatim as the Core Modules grouping; if empty, falls back to
/// grouping `ranked_paths` by their top-level `src/`-relative subdirectory,
/// humanized (e.g. `src/auth/session.ts` -> "Auth").
pub fn build_doc_page(store: &dyn GraphStore, repo_name: &str, ranked_paths: &[PathBuf], module_labels: &[ModuleLabel]) -> anyhow::Result<DocPage> {
    let all_nodes = store.all_nodes(repo_name)?;
    let all_symbols = store.nodes_by_kind(repo_name, NodeKind::Symbol)?;
    let mut all_gotchas = store.nodes_by_kind(repo_name, NodeKind::Gotcha)?;
    let mut all_decisions = store.nodes_by_kind(repo_name, NodeKind::Decision)?;
    // `context`/`knowledge`-typed vault notes -- both classify to
    // `NodeKind::Note` (`agentops-notes::NoteType::node_kind`) and get
    // symbol-matched into `Affects` edges the exact same way gotchas and
    // decisions do, so they're rankable/renderable with the same machinery.
    let mut all_notes = store.nodes_by_kind(repo_name, NodeKind::Note)?;
    sort_notes_by_weight(store, repo_name, &mut all_gotchas)?;
    sort_notes_by_weight(store, repo_name, &mut all_decisions)?;
    sort_notes_by_weight(store, repo_name, &mut all_notes)?;

    let generated_at = store.latest_scan(repo_name)?.map(|s| s.started_at).unwrap_or_default();

    let mut sections = Vec::new();
    sections.push(overview_section(repo_name, ranked_paths.len(), all_symbols.len(), all_gotchas.len(), all_decisions.len(), all_notes.len()));
    sections.extend(core_module_sections(store, repo_name, ranked_paths, &all_symbols, module_labels)?);
    sections.extend(knowledge_sections(store, repo_name, &all_notes, "Notes", "notes")?);
    sections.extend(knowledge_sections(store, repo_name, &all_gotchas, "Known Gotchas", "known-gotchas")?);
    sections.extend(knowledge_sections(store, repo_name, &all_decisions, "Architectural Decisions", "architectural-decisions")?);

    Ok(DocPage { repo: repo_name.to_string(), generated_at, node_count: all_nodes.len() as i64, sections })
}

fn overview_section(repo_name: &str, file_count: usize, symbol_count: usize, gotcha_count: usize, decision_count: usize, note_count: usize) -> DocSection {
    let markdown = format!(
        "The core repository for `{repo_name}`. Documentation below is generated from the indexed code graph, ordered by centrality in the dependency graph.\n\n\
         - Files indexed: {file_count}\n- Symbols indexed: {symbol_count}\n- Notes recorded: {note_count}\n- Gotchas recorded: {gotcha_count}\n- Decisions recorded: {decision_count}"
    );
    DocSection { id: "overview".to_string(), group: DocGroup::Repository, title: "Overview".to_string(), blocks: vec![DocBlock::Prose { markdown }] }
}

/// Groups `ranked_paths` either by the caller-supplied LLM labels or, when
/// none were given, by top-level `src/`-relative subdirectory (humanized) —
/// files that don't resolve to any group are dropped from the Core Modules
/// section rather than fabricating a catch-all bucket.
fn core_module_sections(
    store: &dyn GraphStore,
    repo_name: &str,
    ranked_paths: &[PathBuf],
    all_symbols: &[agentops_graph::Node],
    module_labels: &[ModuleLabel],
) -> anyhow::Result<Vec<DocSection>> {
    let groups: Vec<(String, Vec<String>)> = if !module_labels.is_empty() {
        module_labels.iter().map(|m| (m.label.clone(), m.file_paths.clone())).collect()
    } else {
        heuristic_groups(ranked_paths)
    };

    let mut sections = Vec::with_capacity(groups.len());
    for (label, file_paths) in groups {
        if file_paths.is_empty() {
            continue;
        }
        let id = slugify(&format!("core-modules-{label}"));
        let mut blocks = vec![DocBlock::Prose { markdown: format!("Files in this module, ranked by centrality within `{repo_name}`:") }];
        for path_str in &file_paths {
            let symbols_in_file: Vec<_> = all_symbols.iter().filter(|s| s.path.as_deref() == Some(path_str.as_str())).collect();
            if symbols_in_file.is_empty() {
                continue;
            }
            let mut rows = Vec::with_capacity(symbols_in_file.len());
            for symbol in symbols_in_file {
                let name = symbol.name.clone().unwrap_or_else(|| "<anonymous>".to_string());
                let one_liner = documenting_summary(store, repo_name, symbol.id)?.unwrap_or_default();
                let gotcha_count = gotchas_affecting(store, repo_name, symbol.id)?.len() as i64;
                rows.push(SymbolRow { name, one_liner, gotcha_count, node_id: symbol.id });
            }
            blocks.push(DocBlock::SymbolTable { file: path_str.clone(), rows });
        }
        sections.push(DocSection { id, group: DocGroup::CoreModules, title: label, blocks });
    }
    Ok(sections)
}

/// The first line of the `Definition` node `explain_symbol` (in
/// `agentops-llm`, opt-in, never run automatically) attached this symbol
/// via a `Documents` edge, if one exists -- real data when present, `None`
/// otherwise (no LLM call happens here; this is a pure read).
fn documenting_summary(store: &dyn GraphStore, repo_name: &str, symbol_id: i64) -> anyhow::Result<Option<String>> {
    for edge in store.edges_to(repo_name, symbol_id)? {
        if edge.relation != EdgeRelation::Documents {
            continue;
        }
        if let Some(def) = store.get_node(repo_name, edge.src_id)? {
            if let Some(content) = def.content {
                return Ok(content.lines().next().map(str::to_string));
            }
        }
    }
    Ok(None)
}

fn heuristic_groups(ranked_paths: &[PathBuf]) -> Vec<(String, Vec<String>)> {
    let mut by_label: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for path in ranked_paths {
        let Some(label) = top_level_module_label(path) else { continue };
        by_label.entry(label).or_default().push(path.to_string_lossy().into_owned());
    }
    by_label.into_iter().collect()
}

/// `src/auth/session.ts` -> `Some("Auth")`; a file directly under `src/`
/// with no subdirectory, or with no `src/` prefix at all, returns `None` —
/// dropped from Core Modules rather than guessed at, same "under-connect
/// rather than guess wrong" reasoning `rank_files` already uses for
/// dependency edges.
fn top_level_module_label(path: &std::path::Path) -> Option<String> {
    let components: Vec<_> = path.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    let src_idx = components.iter().position(|c| c == "src")?;
    let subdir = components.get(src_idx + 1)?;
    if src_idx + 2 >= components.len() {
        // The component right after `src/` is the filename itself, not a
        // subdirectory -- nothing to group by.
        return None;
    }
    Some(humanize(subdir))
}

fn humanize(dirname: &str) -> String {
    dirname.split(['_', '-']).map(|word| {
        let mut chars = word.chars();
        match chars.next() {
            Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            None => String::new(),
        }
    }).collect::<Vec<_>>().join(" ")
}

fn slugify(s: &str) -> String {
    s.to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { '-' }).collect::<String>().split('-').filter(|s| !s.is_empty()).collect::<Vec<_>>().join("-")
}

fn knowledge_sections(store: &dyn GraphStore, repo_name: &str, notes: &[agentops_graph::Node], title: &str, id: &str) -> anyhow::Result<Vec<DocSection>> {
    if notes.is_empty() {
        return Ok(Vec::new());
    }
    let mut blocks = Vec::with_capacity(notes.len());
    for note in notes {
        let mut edges: Vec<_> = store.edges_from(repo_name, note.id)?.into_iter().filter(|e| e.relation == EdgeRelation::Affects).collect();
        edges.sort_by(|a, b| b.weight.partial_cmp(&a.weight).unwrap_or(std::cmp::Ordering::Equal));

        let mut affects = String::new();
        let mut source = None;
        if let Some(edge) = edges.first() {
            if let Some(target) = store.get_node(repo_name, edge.dst_id)? {
                let target_name = target.name.clone().unwrap_or_else(|| "<unknown>".to_string());
                affects = format!("affects {target_name}()");
                if let (Some(path), Some(line)) = (target.path.clone(), target.start_line) {
                    source = Some((path, line));
                }
            }
        }

        blocks.push(DocBlock::KnowledgeCallout {
            kind: note.kind,
            node_id: note.id,
            title: note.name.clone().unwrap_or_else(|| "(untitled)".to_string()),
            body: note.content.clone().unwrap_or_default(),
            affects,
            source,
        });
    }
    Ok(vec![DocSection { id: id.to_string(), group: DocGroup::Knowledge, title: title.to_string(), blocks }])
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentops_graph::{NewNode, SqliteGraphStore};

    fn node(kind: NodeKind, path: Option<&str>, name: Option<&str>, start_line: Option<i64>, end_line: Option<i64>, content: Option<&str>) -> NewNode {
        NewNode {
            kind,
            repo: "demo".into(),
            path: path.map(String::from),
            name: name.map(String::from),
            container: None,
            start_line,
            end_line,
            content: content.map(String::from),
        }
    }

    #[test]
    fn builds_overview_and_falls_back_to_directory_heuristic_grouping() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let file_path = "src/auth/session.ts";
        store.add_node(node(NodeKind::File, Some(file_path), None, None, None, None)).unwrap();
        store.add_node(node(NodeKind::Symbol, Some(file_path), Some("refreshSession"), Some(10), Some(25), Some("fn refreshSession() {}"))).unwrap();

        let page = build_doc_page(&store, "demo", &[PathBuf::from(file_path)], &[]).unwrap();

        assert_eq!(page.repo, "demo");
        let overview = page.sections.iter().find(|s| s.id == "overview").unwrap();
        assert_eq!(overview.group, DocGroup::Repository);

        let auth_section = page.sections.iter().find(|s| s.title == "Auth").unwrap();
        assert_eq!(auth_section.group, DocGroup::CoreModules);
        let has_symbol_table = auth_section.blocks.iter().any(|b| matches!(b, DocBlock::SymbolTable { file, rows } if file == file_path && rows.iter().any(|r| r.name == "refreshSession")));
        assert!(has_symbol_table, "{auth_section:?}");
    }

    #[test]
    fn llm_module_labels_override_the_directory_heuristic() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let file_path = "src/auth/session.ts";
        store.add_node(node(NodeKind::File, Some(file_path), None, None, None, None)).unwrap();
        store.add_node(node(NodeKind::Symbol, Some(file_path), Some("refreshSession"), Some(1), Some(2), None)).unwrap();

        let labels = vec![ModuleLabel { label: "Authentication".to_string(), file_paths: vec![file_path.to_string()] }];
        let page = build_doc_page(&store, "demo", &[PathBuf::from(file_path)], &labels).unwrap();

        assert!(page.sections.iter().any(|s| s.title == "Authentication"));
        assert!(!page.sections.iter().any(|s| s.title == "Auth"));
    }

    #[test]
    fn gotchas_and_decisions_render_as_knowledge_callouts_with_affects_attribution() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let symbol_id = store.add_node(node(NodeKind::Symbol, Some("src/auth.rs"), Some("verify_token"), Some(1), Some(2), None)).unwrap();
        let gotcha_id = store.add_node(node(NodeKind::Gotcha, None, Some("token-expiry"), None, None, Some("Expiry check was off by one."))).unwrap();
        store.add_edge("demo", gotcha_id, symbol_id, EdgeRelation::Affects).unwrap();

        let page = build_doc_page(&store, "demo", &[], &[]).unwrap();

        let gotchas_section = page.sections.iter().find(|s| s.id == "known-gotchas").expect("gotchas section present");
        let DocBlock::KnowledgeCallout { title, affects, kind, .. } = &gotchas_section.blocks[0] else { panic!("expected a callout block") };
        assert_eq!(title, "token-expiry");
        assert_eq!(affects, "affects verify_token()");
        assert_eq!(*kind, NodeKind::Gotcha);
    }

    #[test]
    fn a_repo_with_no_gotchas_or_decisions_omits_those_sections_entirely() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let page = build_doc_page(&store, "demo", &[], &[]).unwrap();
        assert!(!page.sections.iter().any(|s| s.id == "known-gotchas" || s.id == "architectural-decisions"));
    }

    /// `NodeKind::Note` covers both `context` and `knowledge`-typed vault
    /// notes (`agentops-notes::NoteType::node_kind`) -- neither is gotcha-
    /// or decision-shaped, but both should render the same way (a
    /// `KnowledgeCallout` block in a "Notes" section) since they're
    /// symbol-matched via the same `Affects` edges.
    #[test]
    fn context_and_knowledge_notes_render_as_a_notes_section() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let symbol_id = store.add_node(node(NodeKind::Symbol, Some("src/lib.rs"), Some("build_doc_page"), Some(1), Some(2), None)).unwrap();
        let note_id =
            store.add_node(node(NodeKind::Note, None, Some("architecture-overview"), None, None, Some("This repo's docgen crate stays LLM-free by design."))).unwrap();
        store.add_edge("demo", note_id, symbol_id, EdgeRelation::Affects).unwrap();

        let page = build_doc_page(&store, "demo", &[], &[]).unwrap();

        let notes_section = page.sections.iter().find(|s| s.id == "notes").expect("a notes section must be present");
        assert_eq!(notes_section.group, DocGroup::Knowledge);
        let DocBlock::KnowledgeCallout { title, .. } = &notes_section.blocks[0] else { panic!("expected a callout block") };
        assert_eq!(title, "architecture-overview");
    }

    #[test]
    fn a_repo_with_no_notes_at_all_omits_the_notes_section() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let page = build_doc_page(&store, "demo", &[], &[]).unwrap();
        assert!(!page.sections.iter().any(|s| s.id == "notes"));
    }
}

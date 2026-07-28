use std::collections::HashMap;
use std::path::{Path, PathBuf};

use petgraph::algo::page_rank::page_rank;
use petgraph::graph::DiGraph;

use crate::types::ScannedFile;

const DAMPING_FACTOR: f64 = 0.85;
const ITERATIONS: usize = 20;

/// Ranks scanned files by PageRank over their (best-effort resolved) dependency
/// graph — mirroring Aider's repo-map approach: files referenced by more other
/// files rank higher, so a token-budgeted view of the repo surfaces the most
/// load-bearing files first instead of an arbitrary or alphabetical order.
///
/// Only relative-path-style imports (`./foo`, `../bar/baz`) are resolved against
/// the scanned file set; external package imports and unresolvable module paths
/// simply don't contribute an edge. This under-connects the graph rather than
/// guessing wrong, which is the safer failure mode for a ranking signal.
pub fn rank_files(files: &[ScannedFile]) -> Vec<(PathBuf, f64)> {
    if files.is_empty() {
        return Vec::new();
    }

    let mut graph = DiGraph::<PathBuf, ()>::new();
    let mut index_of = HashMap::new();

    for f in files {
        let idx = graph.add_node(f.path.clone());
        index_of.insert(f.path.clone(), idx);
    }

    for f in files {
        let src_idx = index_of[&f.path];
        for dep in &f.deps {
            if let Some(target) = resolve_relative_dep(&f.path, dep, files) {
                if let Some(&dst_idx) = index_of.get(&target) {
                    graph.add_edge(src_idx, dst_idx, ());
                }
            }
        }
    }

    let ranks = page_rank(&graph, DAMPING_FACTOR, ITERATIONS);

    let mut ranked: Vec<(PathBuf, f64)> = graph
        .node_indices()
        .map(|idx| (graph[idx].clone(), ranks[idx.index()]))
        .collect();

    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    ranked
}

/// Best-effort: resolves `./foo`/`../bar` style relative imports against the
/// scanned file set by trying the source file's known extension first, then
/// falling back to an `index` file, matching common JS/TS module resolution.
fn resolve_relative_dep(from: &Path, dep: &str, files: &[ScannedFile]) -> Option<PathBuf> {
    if !dep.starts_with('.') {
        return None;
    }

    let base = from.parent()?.join(dep);
    let candidates = [
        base.clone(),
        base.with_extension("ts"),
        base.with_extension("tsx"),
        base.with_extension("js"),
        base.with_extension("jsx"),
        base.with_extension("py"),
        base.with_extension("go"),
        base.join("index.ts"),
        base.join("index.js"),
    ];

    for candidate in candidates {
        let normalized = normalize(&candidate);
        if files.iter().any(|f| normalize(&f.path) == normalized) {
            return Some(files.iter().find(|f| normalize(&f.path) == normalized).unwrap().path.clone());
        }
    }

    None
}

/// Lexical path normalization (collapses `a/../b` -> `b`) without touching the
/// filesystem, since these paths may not exist under every candidate extension.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Language;

    fn file(path: &str, deps: &[&str]) -> ScannedFile {
        ScannedFile {
            path: PathBuf::from(path),
            language: Language::TypeScript,
            symbols: vec![],
            deps: deps.iter().map(|s| s.to_string()).collect(),
            chunks: vec![],
            used_tree_sitter: false,
        }
    }

    #[test]
    fn a_file_imported_by_many_others_ranks_highest() {
        // utils.ts is imported by both a.ts and b.ts; c.ts imports nothing.
        let files = vec![
            file("src/utils.ts", &[]),
            file("src/a.ts", &["./utils"]),
            file("src/b.ts", &["./utils"]),
            file("src/c.ts", &[]),
        ];

        let ranked = rank_files(&files);
        let top = &ranked[0].0;
        assert_eq!(top, &PathBuf::from("src/utils.ts"), "ranked: {ranked:?}");
    }

    #[test]
    fn unresolvable_deps_do_not_panic_or_add_edges() {
        let files = vec![file("src/a.ts", &["react", "lodash", "./missing"])];
        let ranked = rank_files(&files);
        assert_eq!(ranked.len(), 1);
    }
}

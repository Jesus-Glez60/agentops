use std::collections::HashMap;
use std::path::{Path, PathBuf};

use petgraph::algo::page_rank::page_rank;
use petgraph::graph::DiGraph;

use crate::types::{Language, ScannedFile, Symbol};

const DAMPING_FACTOR: f64 = 0.85;
const ITERATIONS: usize = 20;

/// Ranks scanned files by PageRank over their (best-effort resolved)
/// dependency graph — files referenced by more other files rank higher, so
/// a token-budgeted view of the repo (`agentops-docgen`'s consumer) surfaces
/// the most load-bearing files first instead of an arbitrary order.
///
/// Resolution is best-effort across three forms: relative imports (`./foo`,
/// `../bar/baz`), Rust module paths (`crate::`/`super::`/`self::`/bare `mod
/// foo;` names), and TypeScript path aliases (`@/foo`, read from the
/// nearest `tsconfig.json`). External package imports and anything that
/// still doesn't resolve simply don't contribute an edge — under-connecting
/// the graph is the safer failure mode for a ranking signal than guessing
/// wrong. `repo_root` is required to locate `tsconfig.json` files on disk.
pub fn rank_files(repo_root: &Path, files: &[ScannedFile]) -> Vec<(PathBuf, f64)> {
    if files.is_empty() {
        return Vec::new();
    }

    let mut graph = DiGraph::<PathBuf, ()>::new();
    let mut index_of = HashMap::new();

    for f in files {
        let idx = graph.add_node(f.path.clone());
        index_of.insert(f.path.clone(), idx);
    }

    for (from, to) in resolve_dependency_edges(repo_root, files) {
        graph.add_edge(index_of[&from], index_of[&to], ());
    }

    let ranks = page_rank(&graph, DAMPING_FACTOR, ITERATIONS);

    let mut ranked: Vec<(PathBuf, f64)> = graph.node_indices().map(|idx| (graph[idx].clone(), ranks[idx.index()])).collect();
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    ranked
}

/// Resolves every file's raw import strings against the scanned file set,
/// returning `(from, to)` path pairs for whatever could be resolved. `pub`
/// so a caller (`scan_and_persist`) can persist these as `DependsOn` edges
/// in the graph store, not just use them transiently for ranking.
///
/// Confirmed via live testing against this actual (Rust + Next.js) repo:
/// relative-import resolution alone produced **zero** edges across 88 real
/// files — this codebase's Rust code uses `use crate::`/`use super::`
/// module paths exclusively, and its TS code uses the `@/...` tsconfig
/// alias exclusively; neither starts with `.`. Both are now resolved too.
pub fn resolve_dependency_edges(repo_root: &Path, files: &[ScannedFile]) -> Vec<(PathBuf, PathBuf)> {
    let workspace_crates = build_workspace_crate_map(repo_root, files);
    let mut edges = Vec::new();
    for f in files {
        for dep in &f.deps {
            let target = resolve_relative_dep(&f.path, dep, files)
                .or_else(|| matches!(f.language, Language::Rust).then(|| resolve_rust_dep(&f.path, dep, files, &workspace_crates)).flatten())
                .or_else(|| matches!(f.language, Language::TypeScript | Language::JavaScript).then(|| resolve_ts_alias_dep(repo_root, &f.path, dep, files)).flatten());
            if let Some(target) = target {
                edges.push((f.path.clone(), target));
            }
        }
    }
    edges
}

/// Maps each in-workspace Rust crate's `[package] name` (as it appears in a
/// `use` path, i.e. hyphens normalized to underscores) to that crate's `src`
/// directory — lets `resolve_rust_dep` resolve `use other_crate::Thing`
/// across crate boundaries, not just within the importing file's own crate.
/// Built once per scan from the *scanned* Rust files' own crate roots
/// (`rust_crate_root`), not a filesystem walk, so it never looks outside
/// what was already scanned. A crate root whose `Cargo.toml` is missing or
/// unparseable contributes nothing, same best-effort spirit as the rest of
/// this module.
fn build_workspace_crate_map(repo_root: &Path, files: &[ScannedFile]) -> HashMap<String, PathBuf> {
    let mut crate_roots: Vec<PathBuf> = files.iter().filter(|f| matches!(f.language, Language::Rust)).filter_map(|f| rust_crate_root(&f.path)).collect();
    crate_roots.sort();
    crate_roots.dedup();

    crate_roots
        .into_iter()
        .filter_map(|root| {
            let content = std::fs::read_to_string(repo_root.join(&root).join("Cargo.toml")).ok()?;
            let table = content.parse::<toml::Table>().ok()?;
            let name = table.get("package")?.get("name")?.as_str()?;
            Some((name.replace('-', "_"), root.join("src")))
        })
        .collect()
}

/// Same-file symbol-to-symbol references, AST-precise: for each symbol
/// whose `references` set names another symbol defined in the same file,
/// returns `(from_index, to_index)` pairs into `symbols`. A name matching
/// more than one sibling (e.g. `new()` inside two different `impl` blocks)
/// produces an edge to all of them -- over-inclusive, not silent; there's
/// no way to disambiguate which one a bare identifier means without real
/// semantic analysis, which this pass deliberately doesn't attempt.
/// Symbols from the regex fallback (`references` always empty, since
/// there's no AST to walk there) naturally produce nothing here -- see
/// `agentops_notes::match_same_file_references` for that path instead.
pub fn resolve_same_file_symbol_references(symbols: &[Symbol]) -> Vec<(usize, usize)> {
    let mut indices_by_name: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, s) in symbols.iter().enumerate() {
        indices_by_name.entry(s.name.as_str()).or_default().push(i);
    }

    let mut pairs = Vec::new();
    for (from_idx, symbol) in symbols.iter().enumerate() {
        for referenced_name in &symbol.references {
            if let Some(target_indices) = indices_by_name.get(referenced_name.as_str()) {
                for &to_idx in target_indices {
                    if to_idx != from_idx {
                        pairs.push((from_idx, to_idx));
                    }
                }
            }
        }
    }
    pairs
}

/// Resolves a Rust `use crate::...`/`use super::...`/`use self::...`
/// dependency string, a bare `mod foo;` module name, or a `use
/// other_workspace_crate::...` path, against the scanned file set.
/// Best-effort: tries the longest module-path prefix first (the tail of a
/// `use` path is usually an imported item name, e.g. `GraphStore` in
/// `crate::graph::GraphStore`, not itself a file), falling back to shorter
/// prefixes; a first segment that isn't `crate`/`super`/`self` and isn't a
/// known in-workspace crate name (from `workspace_crates`) is a true
/// external crate (`std::...`, `serde::...`) and is deliberately never
/// guessed at.
fn resolve_rust_dep(from: &Path, dep: &str, files: &[ScannedFile], workspace_crates: &HashMap<String, PathBuf>) -> Option<PathBuf> {
    let segments: Vec<&str> = dep.split("::").collect();

    // Whether a total path-segment-match miss should still fall back to
    // `base_dir`'s own root file (`lib.rs`/`mod.rs`/`main.rs`) — true for
    // every branch that names an actual module *path*, where the tail is
    // commonly just an item re-exported at that module's root (e.g. `use
    // other_crate::PublicStruct;`, the overwhelmingly common shape for a
    // crate's public API). False for the bare `mod foo;` branch, where a
    // miss means the module file genuinely doesn't exist among the scanned
    // files — falling back there would misattribute the edge to the
    // declaring file's own module root instead of admitting no match.
    let (base_dir, module_segments, root_fallback): (PathBuf, &[&str], bool) = match segments.first() {
        Some(&"crate") => (rust_crate_root(from)?.join("src"), &segments[1..], true),
        Some(&"super") => {
            let mut dir = rust_parent_module_dir(from)?;
            let mut segs = &segments[1..];
            while segs.first() == Some(&"super") {
                dir = dir.parent()?.to_path_buf();
                segs = &segs[1..];
            }
            (dir, segs, true)
        }
        Some(&"self") => (rust_module_base_dir(from), &segments[1..], true),
        // A bare `mod foo;` module name has no `::` at all — resolve
        // relative to the declaring file's own directory (siblings).
        _ if segments.len() == 1 => (from.parent()?.to_path_buf(), &segments[..], false),
        Some(&first) if segments.len() > 1 && workspace_crates.contains_key(first) => (workspace_crates[first].clone(), &segments[1..], true),
        _ => return None,
    };

    for len in (1..=module_segments.len()).rev() {
        let rel = module_segments[..len].join("/");
        for candidate in [base_dir.join(format!("{rel}.rs")), base_dir.join(&rel).join("mod.rs")] {
            let normalized = normalize(&candidate);
            if let Some(found) = files.iter().find(|f| normalize(&f.path) == normalized) {
                return Some(found.path.clone());
            }
        }
    }

    if root_fallback && !module_segments.is_empty() {
        for candidate in [base_dir.join("lib.rs"), base_dir.join("mod.rs"), base_dir.join("main.rs")] {
            let normalized = normalize(&candidate);
            if let Some(found) = files.iter().find(|f| normalize(&f.path) == normalized) {
                return Some(found.path.clone());
            }
        }
    }
    None
}

/// A Rust file's crate root is the directory containing its `src/`
/// ancestor — found by looking for a `src` path component, since
/// `ScannedFile::path` is always relative to the repo root (a monorepo
/// workspace has many crates, each with its own `src/`, so this can't
/// simply be the repo root).
fn rust_crate_root(from: &Path) -> Option<PathBuf> {
    let components: Vec<_> = from.components().collect();
    let src_idx = components.iter().position(|c| c.as_os_str() == "src")?;
    Some(components[..src_idx].iter().collect())
}

/// `mod.rs`/`lib.rs`/`main.rs` own their containing directory directly
/// (submodules live as siblings inside it); a plain `foo.rs` doesn't own a
/// directory of its own for `self::` resolution unless a same-named `foo/`
/// also exists for its own submodules.
fn is_module_root_file(from: &Path) -> bool {
    matches!(from.file_stem().and_then(|s| s.to_str()), Some("mod" | "lib" | "main"))
}

fn rust_module_base_dir(from: &Path) -> PathBuf {
    if is_module_root_file(from) {
        from.parent().map(PathBuf::from).unwrap_or_default()
    } else {
        let stem = from.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        from.parent().map(|p| p.join(stem)).unwrap_or_default()
    }
}

/// The directory representing this file's *parent* module — one level up
/// from `rust_module_base_dir` conceptually, but `mod.rs`/`lib.rs`/`main.rs`
/// need an extra step up since they already own their containing directory
/// (e.g. `src/graph/mod.rs`'s parent module lives in `src/`, not
/// `src/graph/`; a plain `src/graph.rs`'s parent module lives in `src/`
/// too, one level up from `graph.rs` itself, not from a `graph/` dir it
/// doesn't own).
fn rust_parent_module_dir(from: &Path) -> Option<PathBuf> {
    if is_module_root_file(from) {
        from.parent()?.parent().map(PathBuf::from)
    } else {
        from.parent().map(PathBuf::from)
    }
}

/// Resolves a TypeScript/JavaScript path alias (`@/lib/utils`) — the
/// dominant non-relative import style in modern (Next.js/Vite) codebases —
/// against the nearest ancestor `tsconfig.json`'s `compilerOptions.paths`.
/// Relative imports are handled by `resolve_relative_dep`, not here.
fn resolve_ts_alias_dep(repo_root: &Path, from: &Path, dep: &str, files: &[ScannedFile]) -> Option<PathBuf> {
    if dep.starts_with('.') {
        return None;
    }

    let (config_dir, paths) = find_tsconfig_paths(repo_root, from)?;
    for (pattern, target) in &paths {
        let Some(matched) = match_alias_pattern(pattern, dep) else { continue };
        let resolved = target.replacen('*', &matched, 1);
        let base = config_dir.join(resolved);

        for candidate in [base.clone(), base.with_extension("ts"), base.with_extension("tsx"), base.with_extension("js"), base.with_extension("jsx"), base.join("index.ts"), base.join("index.tsx")] {
            let Ok(rel) = candidate.strip_prefix(repo_root) else { continue };
            let normalized = normalize(rel);
            if let Some(found) = files.iter().find(|f| normalize(&f.path) == normalized) {
                return Some(found.path.clone());
            }
        }
    }
    None
}

/// Walks up from `from`'s directory (within `repo_root`) looking for the
/// nearest `tsconfig.json` that actually declares `compilerOptions.paths`,
/// returning that config's own directory (aliases resolve relative to it,
/// not to `repo_root`) alongside its raw `(pattern, target)` pairs.
fn find_tsconfig_paths(repo_root: &Path, from: &Path) -> Option<(PathBuf, Vec<(String, String)>)> {
    let mut dir = repo_root.join(from.parent()?);
    loop {
        if let Ok(text) = std::fs::read_to_string(dir.join("tsconfig.json")) {
            let paths = parse_tsconfig_paths(&text);
            if !paths.is_empty() {
                return Some((dir, paths));
            }
        }
        if dir == repo_root {
            return None;
        }
        dir = dir.parent()?.to_path_buf();
    }
}

fn parse_tsconfig_paths(text: &str) -> Vec<(String, String)> {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(text) else { return Vec::new() };
    let Some(paths) = json.get("compilerOptions").and_then(|c| c.get("paths")).and_then(|p| p.as_object()) else { return Vec::new() };

    paths
        .iter()
        .filter_map(|(pattern, targets)| targets.as_array()?.first()?.as_str().map(|t| (pattern.clone(), t.to_string())))
        .collect()
}

/// Matches `dep` against a tsconfig `paths` pattern (`@/*`, or an exact
/// non-wildcard key), returning the substring the `*` captured (empty
/// string for an exact match).
fn match_alias_pattern(pattern: &str, dep: &str) -> Option<String> {
    match pattern.split_once('*') {
        Some((prefix, suffix)) => {
            if dep.starts_with(prefix) && dep.ends_with(suffix) && dep.len() >= prefix.len() + suffix.len() {
                Some(dep[prefix.len()..dep.len() - suffix.len()].to_string())
            } else {
                None
            }
        }
        None => (pattern == dep).then(String::new),
    }
}

/// Best-effort: resolves `./foo`/`../bar` style relative imports against
/// the scanned file set, trying the source file's known extension first,
/// then falling back to an `index` file, matching common JS/TS module
/// resolution.
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
        if let Some(found) = files.iter().find(|f| normalize(&f.path) == normalized) {
            return Some(found.path.clone());
        }
    }

    None
}

/// Lexical path normalization (collapses `a/../b` -> `b`) without touching
/// the filesystem, since these paths may not exist under every candidate
/// extension.
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
        ScannedFile { path: PathBuf::from(path), language: Language::TypeScript, symbols: vec![], deps: deps.iter().map(|s| s.to_string()).collect(), chunks: vec![], used_tree_sitter: false, macro_invocation_sites: vec![] }
    }

    fn rust_file(path: &str, deps: &[&str]) -> ScannedFile {
        ScannedFile { path: PathBuf::from(path), language: Language::Rust, symbols: vec![], deps: deps.iter().map(|s| s.to_string()).collect(), chunks: vec![], used_tree_sitter: false, macro_invocation_sites: vec![] }
    }

    fn symbol(name: &str, references: &[&str]) -> Symbol {
        Symbol { name: name.to_string(), container: None, kind: "function".to_string(), start_line: 1, end_line: 2, source: String::new(), references: references.iter().map(|s| s.to_string()).collect() }
    }

    #[test]
    fn resolve_same_file_symbol_references_finds_a_match() {
        let symbols = vec![symbol("a", &["b"]), symbol("b", &[])];
        let pairs = resolve_same_file_symbol_references(&symbols);
        assert_eq!(pairs, vec![(0, 1)]);
    }

    #[test]
    fn resolve_same_file_symbol_references_matches_all_same_named_siblings() {
        // Two `new` symbols (e.g. from different `impl` blocks) -- a bare
        // reference to `new` can't be disambiguated, so it must point at
        // both, over-inclusive rather than silently picking one.
        let symbols = vec![symbol("caller", &["new"]), symbol("new", &[]), symbol("new", &[])];
        let mut pairs = resolve_same_file_symbol_references(&symbols);
        pairs.sort();
        assert_eq!(pairs, vec![(0, 1), (0, 2)]);
    }

    #[test]
    fn resolve_same_file_symbol_references_produces_nothing_for_empty_references() {
        // Regex-fallback-extracted symbols always have empty `references`.
        let symbols = vec![symbol("a", &[]), symbol("b", &[])];
        assert!(resolve_same_file_symbol_references(&symbols).is_empty());
    }

    #[test]
    fn resolve_same_file_symbol_references_never_self_references() {
        let symbols = vec![symbol("factorial", &["factorial"])];
        assert!(resolve_same_file_symbol_references(&symbols).is_empty());
    }

    #[test]
    fn a_file_imported_by_many_others_ranks_highest() {
        let files = vec![file("src/utils.ts", &[]), file("src/a.ts", &["./utils"]), file("src/b.ts", &["./utils"]), file("src/c.ts", &[])];
        let ranked = rank_files(Path::new("."), &files);
        let top = &ranked[0].0;
        assert_eq!(top, &PathBuf::from("src/utils.ts"), "ranked: {ranked:?}");
    }

    #[test]
    fn resolve_dependency_edges_returns_the_same_edges_rank_files_uses_internally() {
        let files = vec![file("src/utils.ts", &[]), file("src/a.ts", &["./utils"]), file("src/b.ts", &["react", "./missing"])];
        let edges = resolve_dependency_edges(Path::new("."), &files);
        assert_eq!(edges, vec![(PathBuf::from("src/a.ts"), PathBuf::from("src/utils.ts"))], "external and unresolvable deps must not produce an edge");
    }

    #[test]
    fn unresolvable_deps_do_not_panic_or_add_edges() {
        let files = vec![file("src/a.ts", &["react", "lodash", "./missing"])];
        let ranked = rank_files(Path::new("."), &files);
        assert_eq!(ranked.len(), 1);
    }

    /// Regression test for a confirmed real gap found via live testing: a
    /// Rust-heavy repo produced **zero** dependency edges because
    /// `use crate::...` module paths (the dominant Rust import style) were
    /// never resolved, only literal `./relative` imports were.
    #[test]
    fn resolves_rust_use_crate_and_mod_paths() {
        let files = vec![
            rust_file("mycrate/src/lib.rs", &["mod graph;"]),
            rust_file("mycrate/src/graph.rs", &[]),
            rust_file("mycrate/src/tools.rs", &["crate::graph::GraphStore"]),
        ];
        let edges = resolve_dependency_edges(Path::new("."), &files);
        assert!(edges.contains(&(PathBuf::from("mycrate/src/tools.rs"), PathBuf::from("mycrate/src/graph.rs"))), "found: {edges:?}");
    }

    #[test]
    fn resolves_rust_super_path_to_an_item_at_the_parent_modules_root_file() {
        let files = vec![
            rust_file("mycrate/src/lib.rs", &[]),
            rust_file("mycrate/src/graph/mod.rs", &["super::lib_helper"]),
        ];
        // No sibling `lib_helper.rs` exists — `lib_helper` is the far more
        // common case of an item defined directly inside `lib.rs`, so the
        // root-file fallback should land there instead of giving up.
        let edges = resolve_dependency_edges(Path::new("."), &files);
        assert_eq!(edges, vec![(PathBuf::from("mycrate/src/graph/mod.rs"), PathBuf::from("mycrate/src/lib.rs"))], "found: {edges:?}");
    }

    #[test]
    fn super_path_still_produces_no_edge_when_no_root_file_exists_either() {
        let files = vec![rust_file("mycrate/src/graph/mod.rs", &["super::lib_helper"])];
        let edges = resolve_dependency_edges(Path::new("."), &files);
        assert!(edges.is_empty(), "found: {edges:?}");
    }

    #[test]
    fn external_rust_crates_are_never_resolved() {
        let files = vec![rust_file("mycrate/src/lib.rs", &["std::collections::HashMap", "serde::Deserialize"])];
        let edges = resolve_dependency_edges(Path::new("."), &files);
        assert!(edges.is_empty(), "external crates must never produce a guessed edge: {edges:?}");
    }

    /// Regression test for the other confirmed real gap: this codebase's
    /// actual `apps/web` uses `@/...` path aliases exclusively (zero
    /// literal relative imports), which only resolving `tsconfig.json`'s
    /// `compilerOptions.paths` can pick up.
    #[test]
    fn resolves_typescript_path_alias_from_tsconfig() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src/lib")).unwrap();
        std::fs::write(root.join("tsconfig.json"), r#"{"compilerOptions": {"paths": {"@/*": ["./src/*"]}}}"#).unwrap();
        std::fs::write(root.join("src/lib/utils.ts"), "export function cn() {}\n").unwrap();
        std::fs::write(root.join("src/app.ts"), "import { cn } from '@/lib/utils';\n").unwrap();

        let files = vec![
            ScannedFile { path: PathBuf::from("src/app.ts"), language: Language::TypeScript, symbols: vec![], deps: vec!["@/lib/utils".to_string()], chunks: vec![], used_tree_sitter: false, macro_invocation_sites: vec![] },
            ScannedFile { path: PathBuf::from("src/lib/utils.ts"), language: Language::TypeScript, symbols: vec![], deps: vec![], chunks: vec![], used_tree_sitter: false, macro_invocation_sites: vec![] },
        ];
        let edges = resolve_dependency_edges(root, &files);
        assert_eq!(edges, vec![(PathBuf::from("src/app.ts"), PathBuf::from("src/lib/utils.ts"))], "found: {edges:?}");
    }

    /// Regression test for a confirmed real gap: in a Cargo workspace, a
    /// `use other_crate::Thing` import across crate boundaries used to be
    /// silently dropped as if it were `std`/`serde` — every sibling crate
    /// rendered as a disconnected island in the knowledge graph even though
    /// they reference each other constantly. `crate-b`'s package name uses
    /// a hyphen (as Cargo requires) while its `use` path uses an
    /// underscore, matching real Rust conventions.
    #[test]
    fn resolves_use_paths_into_other_workspace_crates() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("crates/crate-a/src")).unwrap();
        std::fs::create_dir_all(root.join("crates/crate-b/src")).unwrap();
        std::fs::write(root.join("crates/crate-a/Cargo.toml"), "[package]\nname = \"crate-a\"\n").unwrap();
        std::fs::write(root.join("crates/crate-b/Cargo.toml"), "[package]\nname = \"crate-b\"\n").unwrap();

        // Same "longest module-path prefix, tail is the imported item
        // name" shape as `resolves_rust_use_crate_and_mod_paths`'s
        // `crate::graph::GraphStore` -- `thing` is the submodule file,
        // `Thing` the item defined inside it.
        let files = vec![
            rust_file("crates/crate-a/src/lib.rs", &["crate_b::thing::Thing"]),
            rust_file("crates/crate-b/src/lib.rs", &[]),
            rust_file("crates/crate-b/src/thing.rs", &[]),
        ];
        let edges = resolve_dependency_edges(root, &files);
        assert!(edges.contains(&(PathBuf::from("crates/crate-a/src/lib.rs"), PathBuf::from("crates/crate-b/src/thing.rs"))), "found: {edges:?}");
    }

    /// The actual dominant real-world shape, confirmed live against this
    /// repo's own workspace: `use other_crate::PublicItem;` where
    /// `PublicItem` is defined directly in the other crate's `lib.rs`, not
    /// in its own submodule file — a plain path-segment match alone (as in
    /// the `thing::Thing` test above) never finds this, only the root-file
    /// fallback does.
    #[test]
    fn resolves_use_path_to_an_item_defined_at_another_crates_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("crates/crate-a/src")).unwrap();
        std::fs::create_dir_all(root.join("crates/crate-b/src")).unwrap();
        std::fs::write(root.join("crates/crate-a/Cargo.toml"), "[package]\nname = \"crate-a\"\n").unwrap();
        std::fs::write(root.join("crates/crate-b/Cargo.toml"), "[package]\nname = \"crate-b\"\n").unwrap();

        let files = vec![rust_file("crates/crate-a/src/lib.rs", &["crate_b::PublicItem"]), rust_file("crates/crate-b/src/lib.rs", &[])];
        let edges = resolve_dependency_edges(root, &files);
        assert_eq!(edges, vec![(PathBuf::from("crates/crate-a/src/lib.rs"), PathBuf::from("crates/crate-b/src/lib.rs"))], "found: {edges:?}");
    }

    #[test]
    fn external_rust_crates_still_unresolved_alongside_a_real_workspace() {
        // Same setup as the cross-crate test, but the dep string names
        // something that isn't any workspace crate's package name — must
        // still not produce a guessed edge.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("crates/crate-a/src")).unwrap();
        std::fs::write(root.join("crates/crate-a/Cargo.toml"), "[package]\nname = \"crate-a\"\n").unwrap();

        let files = vec![rust_file("crates/crate-a/src/lib.rs", &["serde::Deserialize"])];
        let edges = resolve_dependency_edges(root, &files);
        assert!(edges.is_empty(), "found: {edges:?}");
    }
}

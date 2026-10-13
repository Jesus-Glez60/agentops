# Crates: indexing and repo registry

The crates that turn source files into symbols and edges, render docs from the graph, and remember which repos a machine has scanned. Read this before changing parsing, ranking or `repo-map.md` generation; the end-to-end pipeline is in [04-indexing-pipeline.md](../04-indexing-pipeline.md).

Index and field definitions: [02-crate-reference.md](../02-crate-reference.md).

## agentops-scanner

- **Path**: `agentops-core/crates/agentops-scanner`
- **Purpose**: Repo walking, tree-sitter symbol extraction, dependency extraction, chunking and PageRank file ranking. Extraction only: persisting into the graph is `agentops_mcp::scan_and_persist`'s job (module doc).
- **Entry points**:
  - `scan_repo(root) -> ScanReport`: walk, read, extract, redact. All returned text has passed `agentops_security::redact` (doc comment).
  - `walk_repo` (`src/walker.rs`), `watchable_dirs` (directories for the file watcher).
  - `extract_symbols`, `collect_macro_invocation_sites` (`src/ast_extract.rs`).
  - `extract_deps`, `resolve_dependency_edges`, `resolve_same_file_symbol_references` (`src/dep_extract.rs`).
  - `extract_declared_dependencies` (`src/manifest.rs`): declared versions from package manifests, which feed docbrain.
  - `chunk_file`, `rank_files` (PageRank via `petgraph`), `compress_symbol_source`.
  - Types: `ScanReport`, `ScannedFile`, `Symbol`, `Chunk`, `Language`.
- **Languages**: `Language` has `Python`, `TypeScript`, `JavaScript`, `Go`, `Rust`, `Cpp`, `CSharp`. The same seven are compiled in through `TSLP_LANGUAGES` in `.cargo/config.toml`.
- **Depends on**: `security`.
- **Used by**: `api`, `cli`, `heavy-api`, `llm`, `mcp`.
- **Invariants**:
  - Zero runtime network dependency, enforced by `deny.toml`. Grammars are fetched and compiled at build time; `tree-sitter-language-pack`'s `download` feature is disabled (root `Cargo.toml` comment).
  - Symbols with no tree-sitter parse fall back to regex extraction. Files where that also finds nothing are reported as `fallback_gap_files`.
  - `References` edges are same-file only, found by identifier matching; not a call graph. A cross-file LSP variant in `agentops-mcp` is unreachable (`findings.md` F25).
- **Known history**: gotchas 53927 (`ignore::WalkBuilder` honors `.gitignore` only inside a git repo), 53987 (dependency-edge gaps for Rust `crate::` and tsconfig aliases), 53982 and 54079 (symbol-identity collisions on `container`), 53926 (C# bare method re-parse), 54110 (Windows CRT mismatch for C++/C# grammars), 53925 (`tree_sitter_language_pack::Node` owns its tree).
- **Tests**: 68 (`src/ast_extract.rs` 22, `src/ranker.rs` 15, `src/compress.rs` 9, `src/dep_extract.rs` 8, `src/manifest.rs` 5, `src/walker.rs` 4, `src/chunker.rs` 3, `src/lib.rs` 2).

> Resolved in Phase 5: decision "Phase 5: C++/C# language coverage + file watcher, live-verified" records that regex fallbacks were added for C++ and C#, so the `scan_repo` comment's "all five languages" is only a stale count (seven now).

## agentops-docgen

- **Path**: `agentops-core/crates/agentops-docgen`
- **Purpose**: Renders onboarding and engineering documentation from a `GraphStore`, with no LLM call.
- **Entry points**:
  - `render_onboarding_doc(store, repo_name, ranked_paths) -> String`: the Markdown behind `repo-map.md`.
  - `write_to_file(content, path)`.
  - `build_doc_page(store, repo_name, ranked_paths, module_labels) -> DocPage` (`src/sectioned.rs`): the structured page the web Documentation Viewer shows.
  - Model types (`src/model.rs`): `DocPage`, `DocSection`, `DocBlock`, `DocGroup`, `SymbolRow`.
- **Depends on**: `graph`.
- **Used by**: `api`, `heavy-api`, `mcp` (`generate_docs` tool, `agentops docgen`).
- **Invariants**:
  - Ranking is input: callers pass `agentops_scanner::rank_files` output. This crate never re-ranks.
  - `DocPage` is persisted by the graph as opaque JSON text (`GraphStore::save_doc_page`), so `agentops-graph` never depends on this crate. Decision "Documentation Viewer backend: structured DocPage, LLM-free docgen, opaque-string persistence to avoid a crate cycle".
  - Every `Documents` edge it reads must come from a one-line `Definition` node. That is why doc sections use the separate `Covers` relation (doc comment on `EdgeRelation::Covers`).
- **Tests**: 15 (`src/sectioned.rs` 6, `src/lib.rs` 5, `src/model.rs` 4).

## agentops-manifest

- **Path**: `agentops-core/crates/agentops-manifest`
- **Purpose**: The local scan registry: a flat JSON file listing every repo `agentops install` has run against on this machine. Also the data-dir resolver and first-run bootstrap.
- **Entry points**:
  - `record_scan`, `list_scanned_repos`, `forget`, `forget_all_except` (and `_at` variants taking an explicit path).
  - `default_manifest_path()`: `<data dir>/manifest.json`.
  - `agentops_data_dir()`: `AGENTOPS_DATA_DIR`, else `~/.agentops`.
  - `bootstrap` module: `BootstrapConfig` with `validate()`/`to_env_file()`, shared by the `agentops init` wizard (classic install) and the web `/setup` page (PM2 install). `generate_master_key` re-exports the `agentops-security` generator for the wizard. Docker and Kubernetes skip this module (module doc).
- **Depends on**: `repo-access`, `security`.
- **Used by**: `api`, `cli`, `heavy-api`, `server`, `docbrain-mcp`.
- **Invariants**:
  - A flat file, deliberately not a database. It holds a small, rarely written list; graph content stays in each repo's own `.context/graph.db` (module doc).
  - `agentops forget` touches only the manifest, never the repo's graph (CLI doc comment).
- **Known history**: gotcha 53950, "AGENTOPS_DATA_DIR defaults to ~/.agentops regardless of deployment method -- data leaks across methods on the same host". `findings.md` F16 lists stores that bypass `agentops_data_dir`.
- **Tests**: 15 (`src/bootstrap.rs` 8, `src/lib.rs` 7).

//! `agentops` — single entry-point CLI for the vnext codebrain/docbrain
//! foundation. Every subcommand is a thin wrapper: parse args, call an
//! already-built library function, print the result — no business logic
//! lives here (see `agentops-mcp::{scan, notes, init}` for the shared
//! use cases this and the MCP tool table both call identically).
//!
//! Ruler/prompt-pack distribution and `sync-docs` are not here yet —
//! `agentops-ruler-bridge` is still a stub, and `sync-docs` is a separate
//! follow-up (see the plan).

use std::path::{Path, PathBuf};

use agentops_graph::{GraphStore, NodeKind};
use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(name = "agentops", version, about = "AgentOps codebrain/docbrain CLI")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Scan a repo into the graph and generate AGENTS.md.
    Install {
        #[arg(long, default_value = ".")]
        path: PathBuf,
        /// Persist an external notes location (e.g. a personal vault) into
        /// .agentops/config.json instead of the in-repo default.
        #[arg(long)]
        notes_path: Option<PathBuf>,
        /// Preview what would happen without writing anything.
        #[arg(long)]
        dry_run: bool,
        /// Embed each new/changed symbol so it's findable via `search`.
        /// Local, no API cost, but real CPU latency per symbol — off by
        /// default.
        #[arg(long)]
        with_embeddings: bool,
        /// Skip prompt-pack distribution via Ruler entirely.
        #[arg(long)]
        no_ruler: bool,
        /// Comma-separated agent ids to distribute the prompt pack to (e.g.
        /// claude,cursor). Passed straight through to `ruler apply --agents`.
        #[arg(long, value_delimiter = ',', default_value = "claude")]
        agents: Vec<String>,
    },
    /// Show what's currently scanned for a repo.
    Status {
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },
    /// List every repo `agentops install` has ever run against on this
    /// machine (from ~/.agentops/manifest.json).
    Repos,
    /// Remove a repo from ~/.agentops/manifest.json. Only touches the
    /// manifest — the repo's own .context/graph.db is untouched.
    Forget {
        #[arg(long)]
        path: PathBuf,
        /// Remove every *other* recorded repo instead, keeping only --path.
        #[arg(long)]
        all_except: bool,
    },
    /// Generate an onboarding/engineering doc (repo-map.md) from an
    /// already-scanned repo's graph.
    Docgen {
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },
    /// What changed in this repo's code/notes across scans. With no other
    /// flags, shows the most recent scan's full added/changed/removed diff.
    Changelog {
        #[arg(long, default_value = ".")]
        path: PathBuf,
        /// Show this specific scan's diff instead of the most recent one.
        #[arg(long)]
        since: Option<i64>,
        /// List this many recent scan summaries instead of one scan's full diff.
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Add a gotcha/decision/knowledge note, symbol-matched into the graph.
    /// Omit --kind to classify the text automatically (same heuristic the
    /// add_note MCP tool uses when its note_type argument is omitted).
    Note {
        #[arg(long, default_value = ".")]
        path: PathBuf,
        #[arg(long, value_enum)]
        kind: Option<NoteKindArg>,
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
        /// Embed the note so it's findable via `search`.
        #[arg(long)]
        with_embeddings: bool,
        title: String,
        text: String,
    },
    /// Recursively ingest a notes folder (a real vault or an unorganized
    /// one) into a repo's graph.
    IngestNotes {
        #[arg(long, default_value = ".")]
        path: PathBuf,
        #[arg(long)]
        notes: Option<PathBuf>,
        /// Print the note -> symbol match table without writing anything.
        #[arg(long)]
        dry_run: bool,
        /// Classify freeform notes via the Anthropic API instead of the
        /// no-network heuristic (requires AGENTOPS_ANTHROPIC_API_KEY).
        #[arg(long)]
        llm_classify: bool,
        /// Re-rank each note's cheap-matched candidates with one Anthropic
        /// API call per note (requires AGENTOPS_ANTHROPIC_API_KEY).
        #[arg(long)]
        llm_match: bool,
        #[arg(long, default_value_t = 4)]
        min_name_len: usize,
        /// Embed every ingested note so it's findable via `search`.
        #[arg(long)]
        with_embeddings: bool,
    },
    /// Dense-vector search over embedded symbols/gotchas/decisions/notes —
    /// requires having scanned/added notes with --with-embeddings first.
    Search {
        #[arg(long, default_value = ".")]
        path: PathBuf,
        #[arg(long, default_value_t = 10)]
        top_k: usize,
        #[arg(long, value_enum)]
        kind: Option<SearchKindArg>,
        query: String,
    },
    /// Generate an LLM explanation of a symbol (requires
    /// AGENTOPS_ANTHROPIC_API_KEY) and record it as a Definition node.
    Explain {
        #[arg(long, default_value = ".")]
        path: PathBuf,
        #[arg(long)]
        symbol: String,
        /// File path relative to the repo root, or a `Container::name`
        /// qualifier, to disambiguate `symbol` if it isn't unique.
        #[arg(long)]
        file: Option<PathBuf>,
    },
    /// Run the stdio MCP server.
    Serve {
        #[arg(long, value_enum, default_value_t = AccessModeArg::Advisor)]
        access_mode: AccessModeArg,
    },
    /// Run the REST API server.
    ServeApi {
        #[arg(long, value_enum, default_value_t = AccessModeArg::Advisor)]
        access_mode: AccessModeArg,
        #[arg(long, default_value = "127.0.0.1:8420")]
        addr: String,
    },
    /// Run the docbrain stdio MCP server (library docs/changelogs).
    DocbrainServe {
        #[arg(long)]
        db: Option<PathBuf>,
    },
    /// Run the docbrain REST API server.
    DocbrainServeApi {
        #[arg(long, default_value = "127.0.0.1:8421")]
        addr: String,
        #[arg(long)]
        db: Option<PathBuf>,
    },
    /// Scan a repo's dependencies and auto-register any docbrain doesn't
    /// already know about (registry metadata, then GitHub search, then an
    /// interactive prompt).
    SyncDocs {
        #[arg(long, default_value = ".")]
        path: PathBuf,
        #[arg(long)]
        db: Option<PathBuf>,
        /// Skip the interactive fallback prompt for unresolved dependencies.
        #[arg(long)]
        no_interactive: bool,
    },
    /// Generate a new API key for agentops-api/docbrain-api's optional
    /// auth. Prints the raw key once and its hash (what the server needs).
    ApiKey {
        #[command(subcommand)]
        action: ApiKeyAction,
    },
}

#[derive(Subcommand)]
enum ApiKeyAction {
    Generate,
}

#[derive(Clone, Copy, ValueEnum, Debug)]
enum AccessModeArg {
    Advisor,
    Full,
}

impl From<AccessModeArg> for agentops_mcp::AccessMode {
    fn from(mode: AccessModeArg) -> Self {
        match mode {
            AccessModeArg::Advisor => agentops_mcp::AccessMode::Advisor,
            AccessModeArg::Full => agentops_mcp::AccessMode::Full,
        }
    }
}

#[derive(Clone, Copy, ValueEnum, Debug)]
enum NoteKindArg {
    Gotcha,
    Decision,
    Knowledge,
}

#[derive(Clone, Copy, ValueEnum, Debug)]
enum SearchKindArg {
    Symbol,
    File,
    Gotcha,
    Decision,
    Note,
    Definition,
}

impl From<SearchKindArg> for NodeKind {
    fn from(kind: SearchKindArg) -> Self {
        match kind {
            SearchKindArg::Symbol => NodeKind::Symbol,
            SearchKindArg::File => NodeKind::File,
            SearchKindArg::Gotcha => NodeKind::Gotcha,
            SearchKindArg::Decision => NodeKind::Decision,
            SearchKindArg::Note => NodeKind::Note,
            SearchKindArg::Definition => NodeKind::Definition,
        }
    }
}

/// Deliberately *not* `#[tokio::main]` — every subcommand except
/// `serve-api`/`docbrain-serve-api` is synchronous, and `PostgresGraphStore`
/// (an optional `GraphStore` backend, see `agentops-graph-pg`) owns its
/// *own* internal Tokio runtime and calls `block_on` on it per query. An
/// ambient runtime wrapping the whole `main` would make every one of those
/// calls a nested `block_on` — confirmed via live testing against a real
/// Postgres backend to panic outright ("Cannot start a runtime from within
/// a runtime"), not a hypothetical concern. The two subcommands that are
/// genuinely async build their own runtime right where they're used
/// instead, the same pattern `PostgresGraphStore` itself already relies on.
fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Install { path, notes_path, dry_run, with_embeddings, no_ruler, agents } => {
            install(&path, notes_path.as_deref(), dry_run, with_embeddings, no_ruler, &agents)
        }
        Command::Status { path } => status(&path),
        Command::Repos => repos(),
        Command::Forget { path, all_except } => forget(&path, all_except),
        Command::Docgen { path } => docgen(&path),
        Command::Changelog { path, since, limit } => changelog(&path, since, limit),
        Command::Note { path, kind, tags, with_embeddings, title, text } => note(&path, kind, &tags, with_embeddings, &title, &text),
        Command::IngestNotes { path, notes, dry_run, llm_classify, llm_match, min_name_len, with_embeddings } => {
            ingest_notes(&path, notes.as_deref(), dry_run, llm_classify, llm_match, min_name_len, with_embeddings)
        }
        Command::Search { path, top_k, kind, query } => search(&path, top_k, kind, &query),
        Command::Explain { path, symbol, file } => explain(&path, &symbol, file.as_deref()),
        Command::Serve { access_mode } => agentops_mcp::run_stdio(access_mode.into()),
        Command::ServeApi { access_mode, addr } => tokio::runtime::Runtime::new()?.block_on(agentops_api::run(&addr, access_mode.into())),
        Command::DocbrainServe { db } => docbrain_mcp::run_stdio(&db.unwrap_or_else(docbrain_mcp::default_db_path)),
        Command::DocbrainServeApi { addr, db } => {
            tokio::runtime::Runtime::new()?.block_on(docbrain_api::run(&addr, &db.unwrap_or_else(docbrain_mcp::default_db_path)))
        }
        Command::SyncDocs { path, db, no_interactive } => sync_docs(&path, db.as_deref(), no_interactive),
        Command::ApiKey { action: ApiKeyAction::Generate } => api_key_generate(),
    }
}

fn install(path: &Path, notes_path: Option<&Path>, dry_run: bool, with_embeddings: bool, no_ruler: bool, agents: &[String]) -> Result<()> {
    println!("Scanning {}...", path.display());
    let report = agentops_scanner::scan_repo(path).context("scanning repo")?;
    println!("Found {} files. {} secret(s) redacted.", report.files.len(), report.redacted_count);
    if !report.fallback_gap_files.is_empty() {
        println!("WARNING: no symbols extracted (parse failed, regex fallback also found none) for {} file(s): {:?}", report.fallback_gap_files.len(), report.fallback_gap_files);
    }

    if dry_run {
        let ranked = agentops_scanner::rank_files(path, &report.files);
        println!("--dry-run: would write to {} and AGENTS.md", agentops_mcp::describe_backend(path));
        println!("Top-ranked files: {:?}", ranked.iter().take(5.min(ranked.len())).map(|(p, _)| p).collect::<Vec<_>>());
        return Ok(());
    }

    let summary = agentops_mcp::persist(path, &report, with_embeddings).context("persisting scan to graph store")?;
    if summary.pruned_files > 0 || summary.pruned_symbols > 0 {
        println!("Pruned {} stale file node(s) and {} stale symbol node(s) from prior scans.", summary.pruned_files, summary.pruned_symbols);
    }
    println!(
        "Wrote {} file node(s), {} symbol node(s), {} dependency edge(s) to {}",
        summary.files,
        summary.symbols,
        summary.dependency_edges,
        agentops_mcp::describe_backend(path)
    );

    let init_result = agentops_mcp::init_agents_md(path, notes_path).context("writing AGENTS.md")?;
    println!("Wrote {} (NOTES_PATH: {})", init_result.agents_md_path.display(), init_result.notes_path.display());

    if let Err(e) = agentops_manifest::record_scan(path) {
        println!("WARNING: failed to record this repo in ~/.agentops/manifest.json: {e}");
    }

    if !no_ruler {
        let agents_md_content = std::fs::read_to_string(&init_result.agents_md_path).context("re-reading AGENTS.md for Ruler distribution")?;
        if let Err(e) = agentops_ruler_bridge::build_ruler_dir(path, &agents_md_content) {
            println!("WARNING: failed to build .ruler/ (skipping prompt-pack distribution): {e}");
        } else {
            let agent_ids: Vec<&str> = agents.iter().map(String::as_str).collect();
            match agentops_ruler_bridge::apply(path, &agent_ids, false) {
                Ok(output) => println!("Distributed prompt pack via Ruler to {}:\n{output}", agent_ids.join(", ")),
                Err(e) => println!("WARNING: `ruler apply` failed (prompt pack still built at .ruler/, scan/AGENTS.md unaffected): {e}"),
            }
        }
    }

    Ok(())
}

fn repos() -> Result<()> {
    let repos = agentops_manifest::list_scanned_repos()?;
    if repos.is_empty() {
        println!("No repos recorded yet — run `agentops install` in one first.");
        return Ok(());
    }
    for entry in &repos {
        println!("{}  (last scanned unix {})", entry.path, entry.last_scanned_at);
    }
    Ok(())
}

fn forget(path: &Path, all_except: bool) -> Result<()> {
    if all_except {
        let removed = agentops_manifest::forget_all_except(path)?;
        println!("Removed {removed} other repo(s) from ~/.agentops/manifest.json, kept {}.", path.display());
    } else {
        let removed = agentops_manifest::forget(path)?;
        if removed {
            println!("Removed {} from ~/.agentops/manifest.json.", path.display());
        } else {
            println!("{} was not recorded in ~/.agentops/manifest.json — nothing to remove.", path.display());
        }
    }
    Ok(())
}

fn open_store(path: &Path) -> Result<(Box<dyn GraphStore>, String)> {
    let store = agentops_mcp::open_store(path)?;
    let repo = agentops_mcp::repo_name(path);
    // "Has this repo ever been scanned" needs a backend-agnostic check now
    // that AGENTOPS_DATABASE_URL can select Postgres — a SQLite file's
    // existence (the old check) is meaningless there, since no such file
    // is ever created for that backend, which would make every command
    // using this helper always report "not scanned" under Postgres.
    if store.latest_scan(&repo)?.is_none() {
        anyhow::bail!("no scans recorded for this repo yet — run `agentops install --path {}` first", path.display());
    }
    Ok((store, repo))
}

fn status(path: &Path) -> Result<()> {
    let (store, repo) = open_store(path)?;
    println!("Graph store: {}", agentops_mcp::describe_backend(path));
    println!("  files:       {}", store.nodes_by_kind(&repo, NodeKind::File)?.len());
    println!("  symbols:     {}", store.nodes_by_kind(&repo, NodeKind::Symbol)?.len());
    println!("  gotchas:     {}", store.nodes_by_kind(&repo, NodeKind::Gotcha)?.len());
    println!("  decisions:   {}", store.nodes_by_kind(&repo, NodeKind::Decision)?.len());
    println!("  notes:       {}", store.nodes_by_kind(&repo, NodeKind::Note)?.len());
    println!("  definitions: {}", store.nodes_by_kind(&repo, NodeKind::Definition)?.len());

    // A repo scanned before this feature existed, or with zero
    // gotchas/decisions ever recorded, genuinely has no row yet — omit the
    // section rather than printing an empty one.
    if let Some(state) = store.get_repo_state(&repo)? {
        let names = |ids: &[i64]| -> Result<Vec<String>> {
            let mut out = Vec::with_capacity(ids.len());
            for &id in ids {
                if let Some(node) = store.get_node(&repo, id)? {
                    out.push(node.name.unwrap_or_else(|| "(untitled)".to_string()));
                }
            }
            Ok(out)
        };
        if !state.top_gotcha_ids.is_empty() {
            println!("  top gotchas:   {}", names(&state.top_gotcha_ids)?.join(", "));
        }
        if !state.top_decision_ids.is_empty() {
            println!("  top decisions: {}", names(&state.top_decision_ids)?.join(", "));
        }
    }
    Ok(())
}

fn docgen(path: &Path) -> Result<()> {
    let out_path = agentops_mcp::generate_docs(path)?;
    println!("Wrote {}", out_path.display());
    Ok(())
}

fn print_scan_summary(scan: &agentops_graph::ScanHistory) {
    println!("Scan #{} ({})", scan.id, scan.started_at);
    println!(
        "  Files: +{} ~{} -{}   Symbols: +{} ~{} -{}   Notes added: {}",
        scan.files_added, scan.files_changed, scan.files_removed, scan.symbols_added, scan.symbols_changed, scan.symbols_removed, scan.notes_added
    );
}

fn changelog(path: &Path, since: Option<i64>, limit: Option<usize>) -> Result<()> {
    let (store, repo) = open_store(path)?;
    let scans = store.list_scans(&repo)?; // already DESC-ordered

    // `--limit` with no `--since`: list recent scans' summaries, not one
    // scan's full diff — useful before drilling into a specific one.
    if since.is_none() {
        if let Some(limit) = limit {
            if scans.is_empty() {
                println!("No scans recorded yet for this repo.");
                return Ok(());
            }
            for scan in scans.iter().take(limit) {
                print_scan_summary(scan);
            }
            return Ok(());
        }
    }

    let scan = match since {
        Some(id) => scans.into_iter().find(|s| s.id == id).ok_or_else(|| anyhow::anyhow!("no scan #{id} found for this repo"))?,
        None => match scans.into_iter().next() {
            Some(latest) => latest,
            None => {
                println!("No scans recorded yet for this repo.");
                return Ok(());
            }
        },
    };
    print_scan_summary(&scan);

    let entries = store.scan_entries(scan.id)?;
    if entries.is_empty() {
        println!("  (no changes)");
    }
    for e in &entries {
        let label = match (&e.name, &e.path) {
            (Some(name), Some(path)) => format!("{name} ({path})"),
            (Some(name), None) => name.clone(),
            (None, Some(path)) => path.clone(),
            (None, None) => "<unknown>".to_string(),
        };
        println!("  {:?} {:?} {}", e.change, e.kind, label);
    }
    Ok(())
}

fn note(path: &Path, kind: Option<NoteKindArg>, tags: &[String], with_embeddings: bool, title: &str, text: &str) -> Result<()> {
    let note_type = kind.map(|k| match k {
        NoteKindArg::Gotcha => agentops_notes::NoteType::Gotcha,
        NoteKindArg::Decision => agentops_notes::NoteType::Decision,
        NoteKindArg::Knowledge => agentops_notes::NoteType::Knowledge,
    });
    let result = agentops_mcp::add_note(path, title, text, note_type, tags, None, with_embeddings)?;
    let type_str = match result.note_type {
        agentops_notes::NoteType::Gotcha => "gotcha",
        agentops_notes::NoteType::Decision => "decision",
        agentops_notes::NoteType::Knowledge => "knowledge",
        agentops_notes::NoteType::Context => "context",
    };
    println!("Wrote {} ({type_str}, {} edge(s) to related symbols, {} reinforced).", result.file_path.display(), result.edges_written, result.edges_reinforced);
    Ok(())
}

fn ingest_notes(path: &Path, notes_dir: Option<&Path>, dry_run: bool, llm_classify: bool, llm_match: bool, min_name_len: usize, with_embeddings: bool) -> Result<()> {
    let resolved_notes_dir = agentops_notes::resolve_notes_path(path, notes_dir);

    let llm_config = if llm_classify || llm_match { Some(agentops_llm::AnthropicConfig::from_env()?) } else { None };

    let heuristic_classifier = agentops_notes::HeuristicClassifier;
    let llm_classifier = llm_config.as_ref().map(|config| agentops_llm::LlmAssistedClassifier { config });
    let classifier: &dyn agentops_notes::NoteClassifier = match (&llm_classifier, llm_classify) {
        (Some(c), true) => c,
        _ => &heuristic_classifier,
    };

    let cheap_matcher = agentops_notes::WordBoundaryMatcher { min_name_len };
    let llm_matcher = llm_config.as_ref().map(|config| agentops_llm::LlmAssistedMatcher { config, min_name_len });
    let matcher: &dyn agentops_notes::SymbolMatcher = match (&llm_matcher, llm_match) {
        (Some(m), true) => m,
        _ => &cheap_matcher,
    };

    if dry_run {
        let (store, repo) = open_store(path)?;
        let notes = agentops_notes::walk_vault(&resolved_notes_dir, classifier)?;
        println!("Found {} note(s) under {}", notes.len(), resolved_notes_dir.display());
        for note in &notes {
            let matched_ids = matcher.match_symbols(store.as_ref(), &repo, &note.body)?;
            let names: Vec<String> = matched_ids.iter().filter_map(|&id| store.get_node(&repo, id).ok().flatten().and_then(|n| n.name)).collect();
            println!("[{:?}] {} -> {}", note.note_type, note.title, if names.is_empty() { "(no match)".to_string() } else { names.join(", ") });
        }
        println!("\n(dry run — nothing written; drop --dry-run to ingest for real)");
        return Ok(());
    }

    let summary = agentops_mcp::ingest_notes_dir(path, notes_dir, classifier, matcher, with_embeddings)?;
    println!("Ingested {} of {} note(s) from {}, wrote {} edge(s), reinforced {}.", summary.notes_written, summary.notes_seen, resolved_notes_dir.display(), summary.edges_written, summary.edges_reinforced);
    Ok(())
}

fn search(path: &Path, top_k: usize, kind: Option<SearchKindArg>, query: &str) -> Result<()> {
    use agentops_embeddings::Embedder;

    let (store, repo) = open_store(path)?;
    let embedding = agentops_embeddings::LocalEmbedder.embed(query)?;
    let hits = store.search_similar(&repo, &embedding, top_k, kind.map(NodeKind::from))?;

    if hits.is_empty() {
        println!("No matches (nothing embedded yet, or nothing close enough — see install/note/ingest-notes's --with-embeddings flag).");
        return Ok(());
    }
    for (node, distance) in &hits {
        println!("{:?} {} (distance {distance:.4}){}", node.kind, node.name.as_deref().unwrap_or("(untitled)"), node.path.as_deref().map(|p| format!(" — {p}")).unwrap_or_default());
    }
    Ok(())
}

fn explain(path: &Path, symbol: &str, file: Option<&Path>) -> Result<()> {
    let (store, repo) = open_store(path)?;
    let symbol_id = agentops_llm::find_symbol_by_name(store.as_ref(), &repo, symbol, file)?;
    let config = agentops_llm::AnthropicConfig::from_env()?;
    let definition_id = agentops_llm::explain_symbol(store.as_ref(), &config, &repo, symbol_id)?;
    let definition = store.get_node(&repo, definition_id)?.context("definition node vanished immediately after being written")?;

    println!("Definition #{definition_id} for {symbol} (symbol #{symbol_id}):\n");
    println!("{}", definition.content.as_deref().unwrap_or(""));
    Ok(())
}

/// Docbrain's full 3-step discovery order: (1) registry metadata, (2)
/// GitHub search, (3) ask the user — implemented here rather than in
/// docbrain-ingest since step 3 is inherently an interactive CLI concern.
/// Single-tenant this pass (`docbrain-graph` dropped `TenantContext`), so
/// no `--org` flag or visibility plumbing.
fn sync_docs(path: &Path, db: Option<&Path>, no_interactive: bool) -> Result<()> {
    use docbrain_ingest::{classify_dependency, RegistryDiscovery};
    use std::collections::BTreeSet;

    let report = agentops_scanner::scan_repo(path).context("scanning repo for dependencies")?;

    let mut candidates: BTreeSet<(docbrain_ingest::Ecosystem, String)> = BTreeSet::new();
    for file in &report.files {
        let language = file.language.tree_sitter_name();
        for dep in &file.deps {
            if let Some(pair) = classify_dependency(language, dep) {
                candidates.insert(pair);
            }
        }
    }

    println!("Found {} distinct third-party dependencies across {} files.", candidates.len(), report.files.len());

    let store = docbrain_graph::SqliteDocbrainStore::open(&db.map(Path::to_path_buf).unwrap_or_else(docbrain_mcp::default_db_path)).context("opening docbrain store")?;
    let summary = sync_candidates(&store, &RegistryDiscovery, &candidates, no_interactive)?;

    println!(
        "{} already known, {} discovered (registry/GitHub), {} registered from user input, {} unresolved.",
        summary.already_known,
        summary.discovered,
        summary.asked,
        summary.unresolved.len()
    );
    if !summary.unresolved.is_empty() {
        println!("Not found via any discovery step, and not provided manually:");
        for name in &summary.unresolved {
            println!("  - {name}");
        }
    }

    Ok(())
}

#[derive(Debug, Default, PartialEq, Eq)]
struct SyncSummary {
    already_known: u32,
    discovered: u32,
    asked: u32,
    unresolved: Vec<String>,
}

/// Docbrain's full 3-step discovery order: (1) registry metadata, (2)
/// GitHub search, (3) ask the user. Takes `&dyn LibraryDiscovery` so this
/// core loop is unit-testable against a fake, without hitting the real
/// npm/PyPI/GitHub network — mirroring how `docbrain-ingest`'s own tests
/// already fake `Embedder`/`LibraryDiscovery`.
fn sync_candidates(
    store: &dyn docbrain_graph::DocbrainStore,
    discovery: &dyn docbrain_ingest::LibraryDiscovery,
    candidates: &std::collections::BTreeSet<(docbrain_ingest::Ecosystem, String)>,
    no_interactive: bool,
) -> Result<SyncSummary> {
    let mut summary = SyncSummary::default();

    for (ecosystem, name) in candidates {
        if store.get_library(name)?.is_some() {
            summary.already_known += 1;
            continue;
        }

        // Step 1: registry metadata. A network/parse error here is a soft
        // fail (warn, keep going) — one flaky lookup shouldn't abort a batch.
        let step1 = match discovery.discover(*ecosystem, name) {
            Ok(found) => found,
            Err(e) => {
                println!("  warning: registry lookup for {name} failed ({e}), trying GitHub search next");
                None
            }
        };

        if let Some(found) = step1 {
            store.add_library(name, name, found.repo_url.as_deref(), found.docs_url.as_deref())?;
            println!("  discovered (registry): {name} -> {}", found.docs_url.as_deref().unwrap_or(found.repo_url.as_deref().unwrap_or("(no URL published)")));
            summary.discovered += 1;
            continue;
        }

        // Step 2: GitHub search fallback. Same soft-fail treatment — a
        // rate-limited search is not the same as "ask the user," so it's
        // reported distinctly rather than silently falling through.
        let step2 = match discovery.search_github(name) {
            Ok(found) => found,
            Err(e) => {
                println!("  warning: GitHub search for {name} unavailable ({e})");
                None
            }
        };

        if let Some(found) = step2 {
            store.add_library(name, name, found.repo_url.as_deref(), found.docs_url.as_deref())?;
            println!("  discovered (GitHub search): {name} -> {}", found.docs_url.as_deref().unwrap_or(found.repo_url.as_deref().unwrap_or("(no URL published)")));
            summary.discovered += 1;
            continue;
        }

        // Step 3: ask the user, interactively, right now — rather than just
        // printing a list to act on later. `interact_text()` requires a real
        // TTY (dialoguer reads raw terminal input, not just redirected
        // stdin); in a non-interactive context (CI, piped input) it errors,
        // and `.unwrap_or_default()` treats that the same as "left blank" —
        // a safe no-op, not a crash.
        if !no_interactive {
            let prompt = format!("'{name}' not found via registry or GitHub search. Enter a docs URL (blank to skip)");
            let answer: String = dialoguer::Input::new().with_prompt(prompt).allow_empty(true).interact_text().unwrap_or_default();
            if !answer.trim().is_empty() {
                store.add_library(name, name, None, Some(answer.trim()))?;
                println!("  registered (user-provided): {name} -> {}", answer.trim());
                summary.asked += 1;
                continue;
            }
        }
        summary.unresolved.push(name.clone());
    }

    Ok(summary)
}

fn api_key_generate() -> Result<()> {
    let (raw, hash) = agentops_security::api_key::generate_api_key()?;
    println!("Raw key (give this to whoever authenticates with it — shown once, not stored anywhere):");
    println!("  {raw}");
    println!();
    println!("Hash (configure this on the server — AGENTOPS_API_KEY_HASH or DOCBRAIN_API_KEY_HASH):");
    println!("  {hash}");
    Ok(())
}

#[cfg(test)]
mod sync_docs_tests {
    use super::*;
    use docbrain_ingest::{DiscoveredLibrary, Ecosystem, LibraryDiscovery};

    /// A fake `LibraryDiscovery` — no real network calls in a unit test.
    /// `known` resolves via step 1 (registry), `github_only` resolves via
    /// step 2 (GitHub search) after step 1 misses, and anything else misses
    /// both steps entirely.
    struct FakeDiscovery {
        known: Vec<&'static str>,
        github_only: Vec<&'static str>,
    }

    impl LibraryDiscovery for FakeDiscovery {
        fn discover(&self, _ecosystem: Ecosystem, name: &str) -> Result<Option<DiscoveredLibrary>> {
            Ok(self.known.contains(&name).then(|| DiscoveredLibrary {
                name: name.to_string(),
                description: None,
                docs_url: Some(format!("https://registry.example/{name}")),
                repo_url: None,
            }))
        }

        fn search_github(&self, name: &str) -> Result<Option<DiscoveredLibrary>> {
            Ok(self.github_only.contains(&name).then(|| DiscoveredLibrary {
                name: name.to_string(),
                description: None,
                docs_url: None,
                repo_url: Some(format!("https://github.com/example/{name}")),
            }))
        }
    }

    #[test]
    fn discovery_order_prefers_registry_then_github_then_leaves_the_rest_unresolved() {
        let store = docbrain_graph::SqliteDocbrainStore::open_in_memory().unwrap();
        let discovery = FakeDiscovery { known: vec!["next"], github_only: vec!["some-tool"] };
        let candidates = std::collections::BTreeSet::from([
            (Ecosystem::Npm, "next".to_string()),
            (Ecosystem::Npm, "some-tool".to_string()),
            (Ecosystem::Npm, "totally-unknown".to_string()),
        ]);

        let summary = sync_candidates(&store, &discovery, &candidates, true).unwrap();

        assert_eq!(summary.already_known, 0);
        assert_eq!(summary.discovered, 2, "both the registry hit and the GitHub-only hit count as discovered");
        assert_eq!(summary.asked, 0, "no_interactive must skip the prompt entirely");
        assert_eq!(summary.unresolved, vec!["totally-unknown".to_string()]);

        use docbrain_graph::DocbrainStore;
        assert!(store.get_library("next").unwrap().is_some());
        assert!(store.get_library("some-tool").unwrap().is_some());
        assert!(store.get_library("totally-unknown").unwrap().is_none());
    }

    #[test]
    fn a_library_already_known_is_not_rediscovered() {
        let store = docbrain_graph::SqliteDocbrainStore::open_in_memory().unwrap();
        use docbrain_graph::DocbrainStore;
        store.add_library("next", "Next.js", None, Some("https://nextjs.org")).unwrap();

        let discovery = FakeDiscovery { known: vec!["next"], github_only: vec![] };
        let candidates = std::collections::BTreeSet::from([(Ecosystem::Npm, "next".to_string())]);

        let summary = sync_candidates(&store, &discovery, &candidates, true).unwrap();

        assert_eq!(summary.already_known, 1);
        assert_eq!(summary.discovered, 0, "already-known libraries must not be re-queried against discovery");
    }
}

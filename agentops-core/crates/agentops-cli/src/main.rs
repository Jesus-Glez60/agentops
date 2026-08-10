//! `agentops` — single entry-point CLI for the vnext codebrain/docbrain
//! foundation. Every subcommand is a thin wrapper: parse args, call an
//! already-built library function, print the result — no business logic
//! lives here (see `agentops-mcp::{scan, notes, init}` for the shared
//! use cases this and the MCP tool table both call identically).
//!
//! `docgen`, Ruler/prompt-pack distribution, and `sync-docs` are not here
//! yet — `agentops-docgen`/`agentops-ruler-bridge` are still stubs, and
//! `sync-docs` is a separate follow-up (see the plan).

use std::path::{Path, PathBuf};

use agentops_graph::{GraphStore, NodeKind, SqliteGraphStore};
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
    },
    /// Show what's currently scanned for a repo.
    Status {
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

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Install { path, notes_path, dry_run } => install(&path, notes_path.as_deref(), dry_run),
        Command::Status { path } => status(&path),
        Command::Changelog { path, since, limit } => changelog(&path, since, limit),
        Command::Note { path, kind, tags, title, text } => note(&path, kind, &tags, &title, &text),
        Command::IngestNotes { path, notes, dry_run, llm_classify, llm_match, min_name_len } => ingest_notes(&path, notes.as_deref(), dry_run, llm_classify, llm_match, min_name_len),
        Command::Explain { path, symbol, file } => explain(&path, &symbol, file.as_deref()),
        Command::Serve { access_mode } => agentops_mcp::run_stdio(access_mode.into()),
        Command::ServeApi { access_mode, addr } => agentops_api::run(&addr, access_mode.into()).await,
        Command::DocbrainServe { db } => docbrain_mcp::run_stdio(&db.unwrap_or_else(docbrain_mcp::default_db_path)),
        Command::DocbrainServeApi { addr, db } => docbrain_api::run(&addr, &db.unwrap_or_else(docbrain_mcp::default_db_path)).await,
        Command::ApiKey { action: ApiKeyAction::Generate } => api_key_generate(),
    }
}

fn install(path: &Path, notes_path: Option<&Path>, dry_run: bool) -> Result<()> {
    println!("Scanning {}...", path.display());
    let report = agentops_scanner::scan_repo(path).context("scanning repo")?;
    println!("Found {} files. {} secret(s) redacted.", report.files.len(), report.redacted_count);
    if !report.fallback_gap_files.is_empty() {
        println!("WARNING: no symbols extracted (parse failed, regex fallback also found none) for {} file(s): {:?}", report.fallback_gap_files.len(), report.fallback_gap_files);
    }

    if dry_run {
        let ranked = agentops_scanner::rank_files(path, &report.files);
        println!("--dry-run: would write {} and AGENTS.md", agentops_mcp::graph_db_path(path).display());
        println!("Top-ranked files: {:?}", ranked.iter().take(5.min(ranked.len())).map(|(p, _)| p).collect::<Vec<_>>());
        return Ok(());
    }

    let summary = agentops_mcp::persist(path, &report).context("persisting scan to graph store")?;
    if summary.pruned_files > 0 || summary.pruned_symbols > 0 {
        println!("Pruned {} stale file node(s) and {} stale symbol node(s) from prior scans.", summary.pruned_files, summary.pruned_symbols);
    }
    println!(
        "Wrote {} file node(s), {} symbol node(s), {} dependency edge(s) to {}",
        summary.files,
        summary.symbols,
        summary.dependency_edges,
        agentops_mcp::graph_db_path(path).display()
    );

    let init_result = agentops_mcp::init_agents_md(path, notes_path).context("writing AGENTS.md")?;
    println!("Wrote {} (NOTES_PATH: {})", init_result.agents_md_path.display(), init_result.notes_path.display());

    Ok(())
}

fn open_store(path: &Path) -> Result<(SqliteGraphStore, String)> {
    let db_path = agentops_mcp::graph_db_path(path);
    if !db_path.exists() {
        anyhow::bail!("no graph store at {} — run `agentops install --path {}` first", db_path.display(), path.display());
    }
    let store = SqliteGraphStore::open(&db_path)?;
    Ok((store, agentops_mcp::repo_name(path)))
}

fn status(path: &Path) -> Result<()> {
    let (store, repo) = open_store(path)?;
    println!("Graph store: {}", agentops_mcp::graph_db_path(path).display());
    println!("  files:       {}", store.nodes_by_kind(&repo, NodeKind::File)?.len());
    println!("  symbols:     {}", store.nodes_by_kind(&repo, NodeKind::Symbol)?.len());
    println!("  gotchas:     {}", store.nodes_by_kind(&repo, NodeKind::Gotcha)?.len());
    println!("  decisions:   {}", store.nodes_by_kind(&repo, NodeKind::Decision)?.len());
    println!("  notes:       {}", store.nodes_by_kind(&repo, NodeKind::Note)?.len());
    println!("  definitions: {}", store.nodes_by_kind(&repo, NodeKind::Definition)?.len());
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

fn note(path: &Path, kind: Option<NoteKindArg>, tags: &[String], title: &str, text: &str) -> Result<()> {
    let note_type = kind.map(|k| match k {
        NoteKindArg::Gotcha => agentops_notes::NoteType::Gotcha,
        NoteKindArg::Decision => agentops_notes::NoteType::Decision,
        NoteKindArg::Knowledge => agentops_notes::NoteType::Knowledge,
    });
    let result = agentops_mcp::add_note(path, title, text, note_type, tags, None)?;
    let type_str = match result.note_type {
        agentops_notes::NoteType::Gotcha => "gotcha",
        agentops_notes::NoteType::Decision => "decision",
        agentops_notes::NoteType::Knowledge => "knowledge",
        agentops_notes::NoteType::Context => "context",
    };
    println!("Wrote {} ({type_str}, {} edge(s) to related symbols).", result.file_path.display(), result.edges_written);
    Ok(())
}

fn ingest_notes(path: &Path, notes_dir: Option<&Path>, dry_run: bool, llm_classify: bool, llm_match: bool, min_name_len: usize) -> Result<()> {
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
            let matched_ids = matcher.match_symbols(&store, &repo, &note.body)?;
            let names: Vec<String> = matched_ids.iter().filter_map(|&id| store.get_node(&repo, id).ok().flatten().and_then(|n| n.name)).collect();
            println!("[{:?}] {} -> {}", note.note_type, note.title, if names.is_empty() { "(no match)".to_string() } else { names.join(", ") });
        }
        println!("\n(dry run — nothing written; drop --dry-run to ingest for real)");
        return Ok(());
    }

    let summary = agentops_mcp::ingest_notes_dir(path, notes_dir, classifier, matcher)?;
    println!("Ingested {} of {} note(s) from {}, wrote {} edge(s).", summary.notes_written, summary.notes_seen, resolved_notes_dir.display(), summary.edges_written);
    Ok(())
}

fn explain(path: &Path, symbol: &str, file: Option<&Path>) -> Result<()> {
    let (store, repo) = open_store(path)?;
    let symbol_id = agentops_llm::find_symbol_by_name(&store, &repo, symbol, file)?;
    let config = agentops_llm::AnthropicConfig::from_env()?;
    let definition_id = agentops_llm::explain_symbol(&store, &config, &repo, symbol_id)?;
    let definition = store.get_node(&repo, definition_id)?.context("definition node vanished immediately after being written")?;

    println!("Definition #{definition_id} for {symbol} (symbol #{symbol_id}):\n");
    println!("{}", definition.content.as_deref().unwrap_or(""));
    Ok(())
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

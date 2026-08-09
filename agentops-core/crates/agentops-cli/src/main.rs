//! agentops — single entry-point CLI for the agentops light tier.
//!
//! Phase 1: real scan -> graph -> AGENTS.md/repo-map.md pipeline for a single
//! local repo. Ruler-based prompt distribution, MCP/API servers, and the heavy
//! tier are still future work (see the plan).

use std::path::{Path, PathBuf};

use agentops_graph::{GraphStore, NodeKind, SqliteGraphStore};
use agentops_notes::AffectsTarget;
use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(name = "agentops", version, about = "Codebrain/Docbrain light-tier CLI")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Scan a repo into the neuron graph and generate AGENTS.md.
    Install {
        /// Repo to scan. Defaults to the current directory.
        #[arg(long, default_value = ".")]
        path: PathBuf,
        /// Preview what would happen without writing anything.
        #[arg(long)]
        dry_run: bool,
        #[arg(long, value_enum, default_value_t = AccessMode::Advisor)]
        access_mode: AccessMode,
        /// Skip Ruler-based prompt-pack distribution entirely (no Node required).
        #[arg(long)]
        no_ruler: bool,
        /// Comma-separated Ruler agent identifiers (e.g. claude,cursor). Empty = all.
        #[arg(long, value_delimiter = ',', default_value = "claude")]
        agents: Vec<String>,
    },
    /// Generate an onboarding/engineering doc from an already-scanned repo.
    Docgen {
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },
    /// Add a gotcha/decision note, edge-connected to a symbol in the graph.
    Note {
        #[arg(long, default_value = ".")]
        path: PathBuf,
        #[arg(long, value_enum, default_value = "gotcha")]
        kind: NoteKind,
        /// Symbol name this note affects, if any.
        #[arg(long)]
        affects: Option<String>,
        title: String,
        text: String,
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
        limit: Option<i64>,
    },
    /// Generate an LLM explanation of a symbol (Anthropic API — requires
    /// AGENTOPS_ANTHROPIC_API_KEY) and record it as a Definition node
    /// connected to the symbol. On-demand only, never runs during a scan.
    Explain {
        #[arg(long, default_value = ".")]
        path: PathBuf,
        /// Symbol name to explain.
        #[arg(long)]
        symbol: String,
        /// File path relative to the repo root, to disambiguate `symbol` if
        /// the name isn't unique in the repo.
        #[arg(long)]
        file: Option<PathBuf>,
    },
    /// Recursively ingest a markdown notes/vault folder, symbol-matching
    /// each note into the given repo's graph (Gotcha/Decision/Note nodes,
    /// Affects-connected to matched symbols). Formalizes what a one-off
    /// script did by hand: real, repeatable notes ingestion.
    IngestNotes {
        /// Repo to attach ingested notes to.
        #[arg(long, default_value = ".")]
        path: PathBuf,
        /// Directory to recursively walk for *.md notes.
        #[arg(long)]
        notes: PathBuf,
        /// Print the note -> symbol match table without writing anything.
        #[arg(long)]
        dry_run: bool,
        /// Re-rank each note's cheap-matched candidates with one Anthropic
        /// API call per note (requires AGENTOPS_ANTHROPIC_API_KEY) instead
        /// of trusting the word-boundary match as-is.
        #[arg(long)]
        llm_match: bool,
        /// Minimum symbol-name length to consider as a match candidate —
        /// the false-positive guard for short, generic names.
        #[arg(long, default_value_t = 4)]
        min_name_len: usize,
    },
    /// Run the stdio MCP server. AccessMode gates which tools are registered
    /// at all — Advisor mode's tools/list never includes scan_repo/add_note/
    /// generate_docs, not just a prompt asking the model to avoid them.
    Serve {
        #[arg(long, value_enum, default_value_t = AccessMode::Advisor)]
        access_mode: AccessMode,
    },
    /// Run the REST API server (same tool logic and AccessMode enforcement as `serve`).
    ServeApi {
        #[arg(long, value_enum, default_value_t = AccessMode::Advisor)]
        access_mode: AccessMode,
        #[arg(long, default_value = "127.0.0.1:8420")]
        addr: String,
    },
    /// Run the docbrain stdio MCP server (library docs/changelogs, Docbrain-1..4).
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
    /// Docbrain-3: scan a repo's dependencies and auto-discover docs for any
    /// that aren't already known to docbrain, via the registry (npm/PyPI).
    /// Reports anything the registry doesn't have, for the web-search/
    /// ask-the-user fallback steps.
    SyncDocs {
        #[arg(long, default_value = ".")]
        path: PathBuf,
        /// Register newly-discovered libraries as private to this org instead
        /// of public.
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        db: Option<PathBuf>,
        /// Skip the interactive ask-user fallback (discovery-order step 3) —
        /// just report what neither the registry nor GitHub search found.
        #[arg(long)]
        no_interactive: bool,
    },
    /// Generate a new API key for agentops-api/docbrain-api's optional
    /// AGENTOPS_API_KEY_HASH/DOCBRAIN_API_KEY_HASH auth. Prints the raw key
    /// once (hand it to the caller who'll use it) and the hash (what you
    /// actually configure on the server) — closes the "no CLI tooling for
    /// this yet" gap noted in SECURITY.md.
    ApiKey {
        #[command(subcommand)]
        action: ApiKeyAction,
    },
}

#[derive(Subcommand)]
enum ApiKeyAction {
    /// Generate a fresh API key and its hash.
    Generate,
}

#[derive(Clone, Copy, ValueEnum, Debug)]
enum AccessMode {
    /// Plans/reviews only — write-capable tools are never registered.
    Advisor,
    /// Full agent access, including code creation/modification.
    Full,
}

impl From<AccessMode> for agentops_mcp::AccessMode {
    fn from(mode: AccessMode) -> Self {
        match mode {
            AccessMode::Advisor => agentops_mcp::AccessMode::Advisor,
            AccessMode::Full => agentops_mcp::AccessMode::Full,
        }
    }
}

#[derive(Clone, Copy, ValueEnum, Debug)]
enum NoteKind {
    Gotcha,
    Decision,
}

fn graph_db_path(repo: &Path) -> PathBuf {
    repo.join(".context").join("graph.db")
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Install { path, dry_run, access_mode, no_ruler, agents } => {
            install(&path, dry_run, access_mode, no_ruler, &agents)
        }
        Command::Docgen { path } => docgen(&path),
        Command::Note { path, kind, affects, title, text } => note(&path, kind, affects.as_deref(), &title, &text),
        Command::Status { path } => status(&path),
        Command::Changelog { path, since, limit } => changelog(&path, since, limit),
        Command::Explain { path, symbol, file } => explain(&path, &symbol, file.as_deref()),
        Command::IngestNotes { path, notes, dry_run, llm_match, min_name_len } => ingest_notes(&path, &notes, dry_run, llm_match, min_name_len),
        Command::Serve { access_mode } => agentops_mcp::run_stdio(access_mode.into()),
        Command::ServeApi { access_mode, addr } => agentops_api::run(&addr, access_mode.into()).await,
        Command::DocbrainServe { db } => docbrain_mcp::run_stdio(&db.unwrap_or_else(default_docbrain_db_path)),
        Command::DocbrainServeApi { addr, db } => docbrain_api::run(&addr, &db.unwrap_or_else(default_docbrain_db_path)).await,
        Command::ApiKey { action: ApiKeyAction::Generate } => api_key_generate(),
        Command::SyncDocs { path, org, db, no_interactive } => {
            sync_docs(&path, org.as_deref(), &db.unwrap_or_else(default_docbrain_db_path), no_interactive)
        }
    }
}

fn default_docbrain_db_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".agentops").join("docbrain.db")
}

fn install(path: &Path, dry_run: bool, access_mode: AccessMode, no_ruler: bool, agents: &[String]) -> Result<()> {
    println!("Scanning {} (access mode: {access_mode:?})...", path.display());
    let report = agentops_scanner::scan_repo(path).context("scanning repo")?;

    println!(
        "Found {} files. {} secrets redacted.",
        report.files.len(),
        report.redacted_count
    );
    if !report.go_gap_files.is_empty() {
        println!(
            "WARNING: Go AST extraction unavailable for {} file(s), fell back to line chunks: {:?}",
            report.go_gap_files.len(),
            report.go_gap_files
        );
    }

    let ranked = agentops_scanner::rank_files(&report.files);

    if dry_run {
        println!("--dry-run: would write {} to {}", graph_db_path(path).display(), path.display());
        println!("--dry-run: would write AGENTS.md and .gitignore entries");
        println!("Top-ranked files: {:?}", ranked.iter().take(5.min(ranked.len())).map(|(p, _)| p).collect::<Vec<_>>());
        return Ok(());
    }

    let db_path = graph_db_path(path);

    // Persistence itself (upsert, prune, DependsOn edges) is shared with
    // agentops-mcp's scan_repo tool — the actual primary way an agent scans
    // a repo mid-session — rather than duplicated here. It used to be
    // duplicated, and the two copies drifted (see agentops-mcp::scan's doc
    // comment); one implementation is what keeps that from happening again.
    let summary = agentops_mcp::persist_scan(path, &report).context("persisting scan to graph store")?;
    if summary.pruned_files > 0 || summary.pruned_symbols > 0 {
        println!("Pruned {} stale file node(s) and {} stale symbol node(s) from prior scans.", summary.pruned_files, summary.pruned_symbols);
    }
    if summary.dependency_edges > 0 {
        println!("Wrote {} DependsOn edge(s).", summary.dependency_edges);
    }

    let opts = agentops_agents_md::GenerateOptions {
        claude_code_installed: !no_ruler && agents.iter().any(|a| a == "claude"),
        repo_map_path: Some("repo-map.md".to_string()),
    };
    let agents_md = agentops_agents_md::generate(path, &opts);
    std::fs::write(path.join("AGENTS.md"), &agents_md).context("writing AGENTS.md")?;

    ensure_gitignore_entries(path)?;

    if let Err(e) = agentops_manifest::record_scan(path) {
        println!("WARNING: could not record this scan in ~/.agentops/manifest.json ({e}) — the dashboard's repo overview won't list it, but the scan itself is unaffected.");
    }

    println!("Wrote {} nodes ({} files, {} symbols) to {}", summary.files + summary.symbols, summary.files, summary.symbols, db_path.display());
    println!("Wrote {}", path.join("AGENTS.md").display());

    if no_ruler {
        println!("Skipping Ruler prompt-pack distribution (--no-ruler).");
    } else {
        println!("Distributing prompt pack via Ruler {}...", agentops_ruler_bridge::RULER_VERSION);
        agentops_ruler_bridge::build_ruler_dir(path, &agents_md).context("building .ruler/ directory")?;
        let agent_refs: Vec<&str> = agents.iter().map(|s| s.as_str()).collect();
        match agentops_ruler_bridge::apply(path, &agent_refs, false) {
            Ok(output) => print!("{output}"),
            Err(e) => println!(
                "WARNING: Ruler prompt distribution failed, continuing without it ({e}). \
                 Scan results and AGENTS.md above are still valid — rerun with --no-ruler to skip this step."
            ),
        }
    }

    println!("Run `agentops docgen --path {}` to generate the onboarding doc.", path.display());

    Ok(())
}

/// Docbrain-3's full 3-step discovery order: (1) registry metadata, (2)
/// GitHub search, (3) ask the user — implemented here rather than in
/// docbrain-ingest since step 3 is inherently an interactive CLI concern.
fn sync_docs(path: &Path, org: Option<&str>, db_path: &Path, no_interactive: bool) -> Result<()> {
    use docbrain_graph::{DocbrainStore, TenantContext, Visibility};
    use docbrain_ingest::{classify_dependency, discover, search_github};
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

    let tenant = match org {
        Some(id) => TenantContext::org(id),
        None => TenantContext::public(),
    };
    let visibility = match &tenant {
        TenantContext::Org(id) => Visibility::Private(id.clone()),
        TenantContext::Public => Visibility::Public,
    };
    let store = DocbrainStore::open(db_path).context("opening docbrain store")?;

    let (mut already_known, mut discovered, mut asked, mut unresolved) = (0u32, 0u32, 0u32, Vec::new());

    for (ecosystem, name) in &candidates {
        if store.get_library(&tenant, name)?.is_some() {
            already_known += 1;
            continue;
        }

        // Step 1: registry metadata. A network/parse error here is a soft
        // fail (warn, keep going) — one flaky lookup shouldn't abort a batch.
        let step1 = match discover(*ecosystem, name) {
            Ok(found) => found,
            Err(e) => {
                println!("  warning: registry lookup for {name} failed ({e}), trying GitHub search next");
                None
            }
        };

        if let Some(found) = step1 {
            store.add_library(&tenant, name, name, found.repo_url.as_deref(), found.docs_url.as_deref(), visibility.clone())?;
            println!("  discovered (registry): {name} -> {}", found.docs_url.as_deref().unwrap_or(found.repo_url.as_deref().unwrap_or("(no URL published)")));
            discovered += 1;
            continue;
        }

        // Step 2: GitHub search fallback. Same soft-fail treatment — a
        // rate-limited search is not the same as "ask the user," so it's
        // reported distinctly rather than silently falling through.
        let step2 = match search_github(name) {
            Ok(found) => found,
            Err(e) => {
                println!("  warning: GitHub search for {name} unavailable ({e})");
                None
            }
        };

        if let Some(found) = step2 {
            store.add_library(&tenant, name, name, found.repo_url.as_deref(), found.docs_url.as_deref(), visibility.clone())?;
            println!("  discovered (GitHub search): {name} -> {}", found.docs_url.as_deref().unwrap_or(found.repo_url.as_deref().unwrap_or("(no URL published)")));
            discovered += 1;
            continue;
        }

        // Step 3: ask the user, interactively, right now — rather than just
        // printing a list to act on later. `interact_text()` requires a real
        // TTY (dialoguer reads raw terminal input, not just redirected
        // stdin); in a non-interactive context (CI, piped input, this
        // sandbox) it errors, and `.unwrap_or_default()` treats that the
        // same as "left blank" — a safe no-op, not a crash. Verified this
        // degrades gracefully via piped stdin; a real terminal session is
        // dialoguer's normal, well-established use case.
        if !no_interactive {
            let prompt = format!("'{name}' not found via registry or GitHub search. Enter a docs URL (blank to skip)");
            let answer: String = dialoguer::Input::new().with_prompt(prompt).allow_empty(true).interact_text().unwrap_or_default();
            if !answer.trim().is_empty() {
                store.add_library(&tenant, name, name, None, Some(answer.trim()), visibility.clone())?;
                println!("  registered (user-provided): {name} -> {}", answer.trim());
                asked += 1;
                continue;
            }
        }

        unresolved.push(name.clone());
    }

    println!(
        "{already_known} already known, {discovered} discovered (registry/GitHub), {asked} registered from user input, {} unresolved.",
        unresolved.len()
    );
    if !unresolved.is_empty() {
        println!("Not found via any discovery step, and not provided manually:");
        for name in &unresolved {
            println!("  - {name}");
        }
    }

    Ok(())
}

fn docgen(path: &Path) -> Result<()> {
    let db_path = graph_db_path(path);
    if !db_path.exists() {
        anyhow::bail!("no graph store at {} — run `agentops install --path {}` first", db_path.display(), path.display());
    }

    let repo_name = path
        .canonicalize()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| path.display().to_string());

    // Re-scan (read-only w.r.t. the graph store) purely to recompute the
    // PageRank file ordering — see the crate-boundary note in agentops-docgen.
    let report = agentops_scanner::scan_repo(path).context("scanning repo for ranking")?;
    let ranked: Vec<PathBuf> = agentops_scanner::rank_files(&report.files).into_iter().map(|(p, _)| p).collect();

    let store = SqliteGraphStore::open(&db_path).context("opening graph store")?;
    let doc = agentops_docgen::render_onboarding_doc(&store, &repo_name, &ranked)?;

    let out_path = path.join("repo-map.md");
    agentops_docgen::write_to_file(&doc, &out_path)?;
    println!("Wrote {}", out_path.display());

    Ok(())
}

fn note(path: &Path, kind: NoteKind, affects: Option<&str>, title: &str, text: &str) -> Result<()> {
    let db_path = graph_db_path(path);
    if !db_path.exists() {
        anyhow::bail!("no graph store at {} — run `agentops install --path {}` first", db_path.display(), path.display());
    }
    let store = SqliteGraphStore::open(&db_path)?;

    let repo_name = path
        .canonicalize()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| path.display().to_string());

    let target = match affects {
        Some(name) => AffectsTarget::SymbolName(name),
        None => AffectsTarget::None,
    };

    let id = match kind {
        NoteKind::Gotcha => agentops_notes::add_gotcha(&store, &repo_name, title, text, target)?,
        NoteKind::Decision => agentops_notes::add_decision(&store, &repo_name, title, text, target)?,
    };

    let connected = !store.edges_from(id)?.is_empty();
    println!(
        "Recorded {kind:?} node #{id}{}",
        if connected { " (connected to a symbol)" } else if affects.is_some() { " (WARNING: symbol not found, recorded without a connection)" } else { "" }
    );

    Ok(())
}

fn status(path: &Path) -> Result<()> {
    let db_path = graph_db_path(path);
    if !db_path.exists() {
        println!("No graph store found at {}. Run `agentops install --path {}` first.", db_path.display(), path.display());
        return Ok(());
    }

    let store = SqliteGraphStore::open(&db_path)?;
    println!("Graph store: {}", db_path.display());
    println!("  files:     {}", store.nodes_by_kind(NodeKind::File)?.len());
    println!("  symbols:   {}", store.nodes_by_kind(NodeKind::Symbol)?.len());
    println!("  gotchas:   {}", store.nodes_by_kind(NodeKind::Gotcha)?.len());
    println!("  decisions: {}", store.nodes_by_kind(NodeKind::Decision)?.len());
    println!("  notes:       {}", store.nodes_by_kind(NodeKind::Note)?.len());
    println!("  definitions: {}", store.nodes_by_kind(NodeKind::Definition)?.len());
    Ok(())
}

fn print_scan_summary(scan: &agentops_graph::ScanHistoryRow) {
    println!(
        "Scan #{} ({} -> {}){}",
        scan.id,
        scan.started_at,
        scan.finished_at,
        scan.git_sha.as_deref().map(|s| format!(" @ {s}")).unwrap_or_default()
    );
    println!(
        "  Files: +{} ~{} -{}   Symbols: +{} ~{} -{}   Notes added: {}",
        scan.files_added, scan.files_changed, scan.files_removed, scan.symbols_added, scan.symbols_changed, scan.symbols_removed, scan.notes_added
    );
}

fn changelog(path: &Path, since: Option<i64>, limit: Option<i64>) -> Result<()> {
    let db_path = graph_db_path(path);
    if !db_path.exists() {
        println!("No graph store found at {}. Run `agentops install --path {}` first.", db_path.display(), path.display());
        return Ok(());
    }
    let store = SqliteGraphStore::open(&db_path)?;
    let repo_name = path
        .canonicalize()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| path.display().to_string());

    // `--limit` with neither `--since` nor a default: list recent scans'
    // summaries, not one scan's full diff -- useful before drilling in.
    if since.is_none() {
        if let Some(limit) = limit {
            let scans = store.list_scans(&repo_name, limit)?;
            if scans.is_empty() {
                println!("No scans recorded yet for this repo.");
                return Ok(());
            }
            for scan in &scans {
                print_scan_summary(scan);
            }
            return Ok(());
        }
    }

    let scan_id = match since {
        Some(id) => id,
        None => match store.latest_scan(&repo_name)? {
            Some(latest) => latest.id,
            None => {
                println!("No scans recorded yet for this repo.");
                return Ok(());
            }
        },
    };
    let Some(scan) = store.get_scan(scan_id)? else {
        anyhow::bail!("no scan #{scan_id} found for this repo");
    };
    print_scan_summary(&scan);

    let entries = store.scan_diff(scan_id)?;
    if entries.is_empty() {
        println!("  (no changes)");
    }
    for e in &entries {
        let label = match e.name.as_deref() {
            Some(name) => match e.path.as_deref() {
                Some(path) => format!("{name} ({path})"),
                None => name.to_string(),
            },
            None => e.path.as_deref().unwrap_or("<unknown>").to_string(),
        };
        println!("  {} {} {}", e.change, e.kind, label);
    }
    Ok(())
}

fn explain(path: &Path, symbol: &str, file: Option<&Path>) -> Result<()> {
    let db_path = graph_db_path(path);
    if !db_path.exists() {
        anyhow::bail!("no graph store at {} — run `agentops install --path {}` first", db_path.display(), path.display());
    }
    let store = SqliteGraphStore::open(&db_path)?;
    let repo_name = path
        .canonicalize()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| path.display().to_string());

    let symbol_id = agentops_llm::find_symbol_by_name(&store, &repo_name, symbol, file)?;
    let config = agentops_llm::AnthropicConfig::from_env()?;
    let definition_id = agentops_llm::explain_symbol(&store, &config, symbol_id)?;
    let definition = store.get_node(definition_id)?.context("definition node vanished immediately after being written")?;

    println!("Definition #{definition_id} for {symbol} (symbol #{symbol_id}):\n");
    println!("{}", definition.content.as_deref().unwrap_or(""));
    Ok(())
}

fn ingest_notes(path: &Path, notes_dir: &Path, dry_run: bool, llm_match: bool, min_name_len: usize) -> Result<()> {
    let db_path = graph_db_path(path);
    if !db_path.exists() {
        anyhow::bail!("no graph store at {} — run `agentops install --path {}` first", db_path.display(), path.display());
    }
    let store = SqliteGraphStore::open(&db_path)?;
    let repo_name = path
        .canonicalize()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| path.display().to_string());

    let notes = agentops_notes::walk_vault(notes_dir)?;
    println!("Found {} notes under {}", notes.len(), notes_dir.display());

    let cheap_matcher = agentops_notes::WordBoundaryMatcher { min_name_len };
    let llm_config = if llm_match { Some(agentops_llm::AnthropicConfig::from_env()?) } else { None };
    let llm_matcher = llm_config.as_ref().map(|config| agentops_llm::LlmAssistedMatcher { config, min_name_len });
    let matcher: &dyn agentops_notes::SymbolMatcher = match &llm_matcher {
        Some(m) => m,
        None => &cheap_matcher,
    };

    if dry_run {
        for note in &notes {
            let matched_ids = matcher.match_symbols(&store, &repo_name, &note.body)?;
            let names: Vec<String> = matched_ids.iter().filter_map(|&id| store.get_node(id).ok().flatten().and_then(|n| n.name)).collect();
            println!("[{:?}] {} -> {}", note.note_type.node_kind(), note.title, if names.is_empty() { "(no match)".to_string() } else { names.join(", ") });
        }
        println!("\n(dry run — nothing written; drop --dry-run to ingest for real)");
        return Ok(());
    }

    let summary = agentops_notes::ingest_vault(&store, &repo_name, &notes, matcher)?;
    println!("Ingested {} of {} notes, wrote {} Affects edge(s).", summary.notes_written, summary.notes_seen, summary.edges_written);
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

/// Adds `.context/` and `repo-map.md` to the target repo's `.gitignore` if not
/// already present — generated scan output is a structured map of the whole
/// codebase's internals and shouldn't be committed by default (SECURITY.md).
fn ensure_gitignore_entries(repo: &Path) -> Result<()> {
    let gitignore_path = repo.join(".gitignore");
    let existing = std::fs::read_to_string(&gitignore_path).unwrap_or_default();

    let needed = [".context/", "repo-map.md"];
    let missing: Vec<&str> = needed.into_iter().filter(|e| !existing.lines().any(|l| l.trim() == *e)).collect();

    if missing.is_empty() {
        return Ok(());
    }

    let mut updated = existing;
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str("\n# agentops generated scan output\n");
    for entry in missing {
        updated.push_str(entry);
        updated.push('\n');
    }

    std::fs::write(&gitignore_path, updated).context("updating .gitignore")
}

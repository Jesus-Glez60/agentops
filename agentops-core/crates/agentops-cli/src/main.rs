//! agentops — single entry-point CLI for the agentops light tier.
//!
//! Phase 1: real scan -> graph -> AGENTS.md/repo-map.md pipeline for a single
//! local repo. Ruler-based prompt distribution, MCP/API servers, and the heavy
//! tier are still future work (see the plan).

use std::path::{Path, PathBuf};

use agentops_graph::{GraphStore, NewNode, NodeKind, SqliteGraphStore};
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
        Command::Serve { access_mode } => agentops_mcp::run_stdio(access_mode.into()),
        Command::ServeApi { access_mode, addr } => agentops_api::run(&addr, access_mode.into()).await,
    }
}

fn install(path: &Path, dry_run: bool, access_mode: AccessMode, no_ruler: bool, agents: &[String]) -> Result<()> {
    let repo_name = path
        .canonicalize()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| path.display().to_string());

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
    let store = SqliteGraphStore::open(&db_path).context("opening graph store")?;

    let mut symbol_count = 0;
    for file in &report.files {
        let path_str = file.path.to_string_lossy().to_string();
        store.add_node(NewNode {
            kind: NodeKind::File,
            repo: repo_name.clone(),
            path: Some(path_str.clone()),
            name: None,
            start_line: None,
            end_line: None,
            content: None,
        })?;

        for symbol in &file.symbols {
            store.add_node(NewNode {
                kind: NodeKind::Symbol,
                repo: repo_name.clone(),
                path: Some(path_str.clone()),
                name: Some(symbol.name.clone()),
                start_line: Some(symbol.start_line as i64),
                end_line: Some(symbol.end_line as i64),
                content: Some(symbol.source.clone()),
            })?;
            symbol_count += 1;
        }
    }

    let opts = agentops_agents_md::GenerateOptions {
        claude_code_installed: !no_ruler && agents.iter().any(|a| a == "claude"),
        repo_map_path: Some("repo-map.md".to_string()),
    };
    let agents_md = agentops_agents_md::generate(path, &opts);
    std::fs::write(path.join("AGENTS.md"), &agents_md).context("writing AGENTS.md")?;

    ensure_gitignore_entries(path)?;

    println!(
        "Wrote {} nodes ({} files, {} symbols) to {}",
        report.files.len() + symbol_count,
        report.files.len(),
        symbol_count,
        db_path.display()
    );
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

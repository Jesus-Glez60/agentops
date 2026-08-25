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
        /// Fuse in lexical (keyword/BM25) and exact-name-match signals —
        /// unlike plain dense search, finds a node even if it was never
        /// embedded (--with-embeddings left off).
        #[arg(long)]
        hybrid: bool,
        /// With --hybrid, also spread activation from the fused top hits
        /// across Affects/References edges (Personalized PageRank) so a
        /// graph-connected result can outrank a purely textual/semantic
        /// match. Ignored without --hybrid.
        #[arg(long)]
        graph_expand: bool,
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
    /// Watches a repo and automatically re-scans it on file changes — the
    /// repo must have already been scanned once (`agentops install`
    /// first). Runs until interrupted (Ctrl+C).
    Watch {
        #[arg(long, default_value = ".")]
        path: PathBuf,
        #[arg(long)]
        with_embeddings: bool,
    },
    /// Run the stdio MCP server.
    Serve {
        #[arg(long, value_enum, default_value_t = AccessModeArg::Advisor)]
        access_mode: AccessModeArg,
    },
    /// Run the merged REST API server — agentops-api, docbrain-api, and
    /// agentops-heavy-api's routes on one port (see the `agentops-server`
    /// crate). Replaces the former separate `serve-api`/`docbrain-serve-api`
    /// split now that there's one open-core, no price gate.
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
    /// Manage native tasks (Module 7's hybrid task manager) — see
    /// `sync-linear` for two-way Linear sync.
    Task {
        #[command(subcommand)]
        action: TaskAction,
    },
    /// Interactive first-run setup wizard for a classic (non-Docker,
    /// non-PM2) terminal deployment — collects the same config a Docker
    /// `.env` or the PM2 `/setup` page would, writes `.env` to the current
    /// directory, and optionally starts the app. Infra config only: this
    /// never creates an account itself (see `BootstrapConfig`'s doc comment
    /// on why org/user setup stays a browser step through `/login`).
    Init {
        /// Skip all prompts and accept every default (generated master
        /// key, SQLite-only, default addr, `first-user-only` signup mode) —
        /// for scripted/CI use.
        #[arg(long)]
        yes: bool,
        /// Write `.env` here instead of the current directory.
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },
    /// Connect a coding tool (Claude Code, Cursor, Codex CLI, Gemini CLI, or
    /// any other Ruler-supported agent) to this repo — registers agentops's
    /// MCP server and distributes AGENTS.md/skills, without a full re-scan.
    /// Re-runnable any time you want to add another tool.
    Connect {
        #[arg(long, default_value = ".")]
        path: PathBuf,
        /// Comma-separated agent ids (e.g. claude,cursor,codex,gemini-cli) —
        /// skips the interactive multi-select prompt.
        #[arg(long, value_delimiter = ',')]
        agents: Vec<String>,
        #[arg(long, value_enum, default_value_t = AccessModeArg::Advisor)]
        access_mode: AccessModeArg,
        /// Skip all prompts — requires --agents to be set (and --api-key,
        /// if --remote is also set).
        #[arg(long)]
        yes: bool,
        /// Point your coding tool at a server-hosted agentops instance
        /// instead of registering a local stdio MCP server — for team
        /// members connecting their own machine to a shared deployment
        /// (e.g. https://agentops.example.com). Omit this for a solo
        /// self-hosted setup where the CLI and server are the same
        /// machine — that keeps today's local/stdio behavior unchanged.
        /// If omitted and the command is interactive, you're asked which
        /// case this is before anything else happens.
        #[arg(long)]
        remote: Option<String>,
        /// Personal API key for --remote (generate one from Settings ->
        /// API Keys in the web app). Prompted for interactively if omitted.
        #[arg(long)]
        api_key: Option<String>,
    },
}

#[derive(Subcommand)]
enum TaskAction {
    /// Creates a native task owned by this repo's graph.
    Create {
        #[arg(long, default_value = ".")]
        path: PathBuf,
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        priority: Option<String>,
        #[arg(long)]
        assignee: Option<String>,
        /// Correlates future scan/note/explain calls sharing this id into
        /// this task's `activity` view.
        #[arg(long)]
        session_id: Option<String>,
        title: String,
    },
    /// Lists every task recorded for a repo.
    List {
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },
    /// Moves a task to a new status.
    UpdateStatus {
        #[arg(long, default_value = ".")]
        path: PathBuf,
        id: i64,
        #[arg(value_enum)]
        status: TaskStatusArg,
    },
    /// Shortcut for `update-status <id> done`.
    Close {
        #[arg(long, default_value = ".")]
        path: PathBuf,
        id: i64,
    },
    /// The task's built-in final-audit view: every activity correlated
    /// under its session_id, oldest first.
    Activity {
        #[arg(long, default_value = ".")]
        path: PathBuf,
        id: i64,
    },
    /// Generates technical/non-technical/client-friendly summaries of a
    /// task's recorded activity (requires AGENTOPS_ANTHROPIC_API_KEY).
    /// Posts the technical + non-technical summaries as Linear comments if
    /// the task is Linear-synced (requires AGENTOPS_LINEAR_API_KEY); prints
    /// all three either way — client-friendly has no auto-post destination
    /// yet.
    Summarize {
        #[arg(long, default_value = ".")]
        path: PathBuf,
        id: i64,
    },
    /// Pulls issues from Linear into this repo's tasks (idempotent — safe
    /// to run repeatedly). Requires AGENTOPS_LINEAR_API_KEY. Poll-based:
    /// there's no webhook infrastructure to push updates automatically yet,
    /// so re-run this periodically to pick up changes made in Linear.
    SyncLinear {
        #[arg(long, default_value = ".")]
        path: PathBuf,
        #[arg(long, default_value_t = 50)]
        limit: u32,
        /// After pulling, also push this task's current local status back
        /// to its Linear issue (errors if it isn't Linear-synced).
        #[arg(long)]
        push: Option<i64>,
        /// Target this exact Linear workflow state name instead of
        /// inferring one from the task's TaskStatus — for teams with custom
        /// states (e.g. "Testing") beyond the generic 5. Only used with
        /// `--push`.
        #[arg(long)]
        status_name: Option<String>,
    },
}

#[derive(Clone, Copy, ValueEnum, Debug)]
enum TaskStatusArg {
    Todo,
    InProgress,
    InReview,
    Done,
    Cancelled,
}

impl From<TaskStatusArg> for agentops_graph::TaskStatus {
    fn from(status: TaskStatusArg) -> Self {
        match status {
            TaskStatusArg::Todo => agentops_graph::TaskStatus::Todo,
            TaskStatusArg::InProgress => agentops_graph::TaskStatus::InProgress,
            TaskStatusArg::InReview => agentops_graph::TaskStatus::InReview,
            TaskStatusArg::Done => agentops_graph::TaskStatus::Done,
            TaskStatusArg::Cancelled => agentops_graph::TaskStatus::Cancelled,
        }
    }
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
    // Only this binary loads `.env` (see the Cargo.toml comment on the
    // `dotenvy` dep) -- picks up whatever `agentops init` just wrote, or a
    // hand-written `.env` in the cwd. Missing file is fine (`.ok()`);
    // vars already set in the real environment always win, since dotenvy
    // never overwrites an existing var.
    dotenvy::dotenv().ok();

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
        Command::Search { path, top_k, kind, hybrid, graph_expand, query } => search(&path, top_k, kind, hybrid, graph_expand, &query),
        Command::Explain { path, symbol, file } => explain(&path, &symbol, file.as_deref()),
        Command::Watch { path, with_embeddings } => {
            open_store(&path).context("repo must be scanned at least once (run `agentops install` first) before watching it")?;
            agentops_mcp::watch_and_rescan(&path, with_embeddings)
        }
        Command::Serve { access_mode } => agentops_mcp::run_stdio(access_mode.into()),
        Command::ServeApi { access_mode, addr } => {
            // SAFETY: single-threaded at this point in `main`, before the
            // tokio runtime (and thus any concurrent access to the
            // environment) starts — `agentops_server::run` reads these same
            // two vars, so setting them here is how CLI flags get threaded
            // into a function whose contract is "reads env, no params".
            unsafe {
                std::env::set_var("AGENTOPS_ADDR", &addr);
                std::env::set_var("AGENTOPS_ACCESS_MODE", if matches!(access_mode, AccessModeArg::Full) { "full" } else { "advisor" });
            }
            tokio::runtime::Runtime::new()?.block_on(agentops_server::run())
        }
        Command::DocbrainServe { db } => docbrain_mcp::run_stdio(&db.unwrap_or_else(docbrain_mcp::default_db_path)),
        Command::SyncDocs { path, db, no_interactive } => sync_docs(&path, db.as_deref(), no_interactive),
        Command::ApiKey { action: ApiKeyAction::Generate } => api_key_generate(),
        Command::Task { action } => match action {
            TaskAction::Create { path, description, priority, assignee, session_id, title } => task_create(&path, &title, description.as_deref(), priority.as_deref(), assignee.as_deref(), session_id.as_deref()),
            TaskAction::List { path } => task_list(&path),
            TaskAction::UpdateStatus { path, id, status } => task_update_status(&path, id, status.into()),
            TaskAction::Close { path, id } => task_update_status(&path, id, agentops_graph::TaskStatus::Done),
            TaskAction::Activity { path, id } => task_activity(&path, id),
            TaskAction::Summarize { path, id } => task_summarize(&path, id),
            TaskAction::SyncLinear { path, limit, push, status_name } => task_sync_linear(&path, limit, push, status_name.as_deref()),
        },
        Command::Init { yes, path } => init(yes, &path),
        Command::Connect { path, agents, access_mode, yes, remote, api_key } => connect(&path, agents, access_mode, yes, remote, api_key),
    }
}

fn init(yes: bool, path: &Path) -> Result<()> {
    use agentops_manifest::BootstrapConfig;

    println!("agentops init — sets up a classic (non-Docker, non-PM2) deployment.\n");

    let config = if yes {
        BootstrapConfig { secrets_master_key: agentops_manifest::bootstrap::generate_master_key()?, signup_mode: Some("first-user-only".to_string()), ..Default::default() }
    } else {
        let generate = dialoguer::Confirm::new().with_prompt("Generate a new AGENTOPS_SECRETS_MASTER_KEY?").default(true).interact()?;
        let secrets_master_key = if generate {
            let key = agentops_manifest::bootstrap::generate_master_key()?;
            println!("  generated: {key}");
            key
        } else {
            dialoguer::Password::new().with_prompt("AGENTOPS_SECRETS_MASTER_KEY (64 hex chars)").interact()?
        };

        let addr: String = dialoguer::Input::new().with_prompt("Bind address").default("127.0.0.1:8420".to_string()).interact_text()?;

        let use_postgres = dialoguer::Confirm::new().with_prompt("Use Postgres for the code-graph store? (No = local SQLite per repo)").default(false).interact()?;
        let database_url = if use_postgres { Some(dialoguer::Input::<String>::new().with_prompt("Postgres connection string").interact_text()?) } else { None };

        let open_signup = dialoguer::Confirm::new().with_prompt("Allow open signup after the first account? (No = invite-only, recommended for self-host)").default(false).interact()?;

        BootstrapConfig {
            secrets_master_key,
            addr: Some(addr),
            database_url,
            signup_mode: Some(if open_signup { "open".to_string() } else { "first-user-only".to_string() }),
            ..Default::default()
        }
    };

    if let Err(errors) = config.validate() {
        for e in &errors {
            eprintln!("error: {e}");
        }
        anyhow::bail!("invalid configuration, see errors above");
    }

    let env_path = path.join(".env");
    std::fs::write(&env_path, config.to_env_file()).with_context(|| format!("writing {}", env_path.display()))?;
    println!("\nWrote {}", env_path.display());
    println!("Tip: from your project repo (not this directory), run `agentops connect` to hook up Claude Code, Cursor, Codex CLI, Gemini CLI, or another coding tool.");

    let start_now = yes || dialoguer::Confirm::new().with_prompt("Start AgentOps now?").default(true).interact().unwrap_or(false);
    if !start_now {
        println!("Run `agentops serve-api` (from {}) when you're ready.", path.display());
        return Ok(());
    }

    // `main()`'s `dotenvy::dotenv().ok()` already ran before this `.env`
    // existed, so it never picked up what we just wrote -- apply the same
    // values to this process's environment directly instead of relying on
    // a reload. SAFETY: single-threaded here, before the tokio runtime
    // starts, same pattern `Command::ServeApi` already uses.
    unsafe {
        std::env::set_var("AGENTOPS_SECRETS_MASTER_KEY", &config.secrets_master_key);
        if let Some(addr) = &config.addr {
            std::env::set_var("AGENTOPS_ADDR", addr);
        }
        if let Some(db_url) = &config.database_url {
            std::env::set_var("AGENTOPS_DATABASE_URL", db_url);
        }
        if let Some(mode) = &config.signup_mode {
            std::env::set_var("AGENTOPS_SIGNUP_MODE", mode);
        }
    }
    println!("Starting agentops-server...");
    start_web_frontend_if_present(path);
    tokio::runtime::Runtime::new()?.block_on(agentops_server::run())
}

/// Best-effort: the frontend's standalone build only exists once a release
/// artifact (`agentops-web-standalone.tar.gz`) has been downloaded and
/// extracted, or a developer has run `npm run build` locally — neither is
/// guaranteed at `agentops init` time, so this looks in the couple of
/// places it could be and silently skips otherwise rather than failing the
/// whole command over an optional piece.
fn start_web_frontend_if_present(path: &Path) {
    // Three layouts: `install.sh`'s AGENTOPS_INSTALL_DIR/web (independent
    // of cwd, since a user typically doesn't run `agentops init` from
    // inside ~/.agentops), a `web/` dir next to `.env` (--path pointed
    // straight at an install dir), and a source checkout's own build
    // output (developer running from the repo).
    // Matches install.sh's AGENTOPS_INSTALL_DIR override / $HOME/.agentops default.
    let install_dir = std::env::var("AGENTOPS_INSTALL_DIR").ok().map(PathBuf::from).or_else(|| std::env::var("HOME").ok().map(|home| PathBuf::from(home).join(".agentops")));
    let install_dir_web = install_dir.map(|d| d.join("web/server.js"));
    let candidates = [install_dir_web, Some(path.join("web/server.js")), Some(path.join("apps/web/.next/standalone/server.js"))];
    let Some(server_js) = candidates.into_iter().flatten().find(|p| p.exists()) else {
        println!("(no bundled frontend found next to .env — run the web app separately if you want the UI)");
        return;
    };
    println!("Starting web frontend from {}...", server_js.display());
    if let Err(e) = std::process::Command::new("node").arg(&server_js).spawn() {
        eprintln!("warning: failed to start web frontend ({e}) — run it separately");
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
        "Wrote {} file node(s), {} symbol node(s), {} dependency edge(s), {} reference edge(s) to {}",
        summary.files,
        summary.symbols,
        summary.dependency_edges,
        summary.reference_edges,
        agentops_mcp::describe_backend(path)
    );

    let init_result = agentops_mcp::init_agents_md(path, notes_path).context("writing AGENTS.md")?;
    println!("Wrote {} (NOTES_PATH: {})", init_result.agents_md_path.display(), init_result.notes_path.display());

    if let Err(e) = agentops_manifest::record_scan(path) {
        println!("WARNING: failed to record this repo in ~/.agentops/manifest.json: {e}");
    }

    if !no_ruler {
        let agents_md_content = std::fs::read_to_string(&init_result.agents_md_path).context("re-reading AGENTS.md for Ruler distribution")?;
        distribute_via_ruler(path, &agents_md_content, agents, "advisor");
    }

    Ok(())
}

/// Shared by `install()` and `connect()`: builds `.ruler/` (rules/skills) +
/// `.ruler/mcp.json` (MCP server registration) and runs `ruler apply` for
/// the given agent ids. Every failure here is a warning, not a hard error —
/// matches `install()`'s existing posture that a scan/AGENTS.md write should
/// never be undone by an optional downstream distribution step failing.
///
/// **Remote-mode durability, not just a one-time write**: if
/// `.context/agentops-remote.json` exists (written by `connect_remote` the
/// first time `--remote` was used for this repo), this fn skips writing
/// `.ruler/mcp.json`'s stdio entry entirely and, after `ruler apply` runs
/// for `AGENTS.md`/prompt-pack distribution, re-runs the native remote-MCP
/// writer as its own last step. Without this, any *later* call to this fn
/// from a plain `agentops install` re-scan (which also calls this,
/// unconditionally, for any reason) would silently revert a team member's
/// coding tool back to the local/stdio entry the next time `ruler apply`
/// regenerates their native config files — a real bug caught auditing this
/// feature's first draft, not a hypothetical.
fn distribute_via_ruler(path: &Path, agents_md_content: &str, agent_ids: &[String], access_mode: &str) {
    if let Err(e) = agentops_ruler_bridge::build_ruler_dir(path, agents_md_content) {
        println!("WARNING: failed to build .ruler/ (skipping prompt-pack and MCP distribution): {e}");
        return;
    }

    let remote_marker = read_remote_marker(path);
    if remote_marker.is_none() {
        if let Err(e) = agentops_ruler_bridge::write_mcp_config(path, access_mode) {
            println!("WARNING: failed to write .ruler/mcp.json (skipping MCP registration, prompt pack still distributed): {e}");
        }
    }

    let agent_ids_ref: Vec<&str> = agent_ids.iter().map(String::as_str).collect();
    match agentops_ruler_bridge::apply(path, &agent_ids_ref, false) {
        Ok(output) => println!("Distributed prompt pack + MCP config via Ruler to {}:\n{output}", agent_ids_ref.join(", ")),
        Err(e) => println!("WARNING: `ruler apply` failed (prompt pack still built at .ruler/, scan/AGENTS.md unaffected): {e}"),
    }

    if let Some(marker) = remote_marker {
        write_remote_mcp_entries(path, agent_ids, &marker.server_url);
    }
}

/// Where each named agent's MCP config actually lands, for the closing
/// summary — matches each vendor's own current docs (fetched directly while
/// planning this, not assumed): Claude Code's `.mcp.json`, Cursor's
/// `.cursor/mcp.json`, Codex CLI's `.codex/config.toml`, Gemini CLI's
/// `.gemini/settings.json`. Anything else Ruler supports but isn't in this
/// table just gets a generic "check its docs" line.
fn mcp_config_location(agent_id: &str) -> Option<&'static str> {
    match agent_id {
        "claude" => Some(".mcp.json"),
        "cursor" => Some(".cursor/mcp.json"),
        "codex" => Some(".codex/config.toml"),
        "gemini-cli" => Some(".gemini/settings.json"),
        _ => None,
    }
}

/// Shared by `connect()`'s local and remote flows -- the "which coding
/// tool(s)" prompt/flags are identical either way, only what happens next
/// differs.
fn select_agents(mut agents: Vec<String>, yes: bool) -> Result<Vec<String>> {
    if !agents.is_empty() {
        return Ok(agents);
    }
    if yes {
        anyhow::bail!("--yes requires --agents to also be set (nothing to connect non-interactively otherwise)");
    }
    let named = ["claude", "cursor", "codex", "gemini-cli"];
    let labels = ["Claude Code", "Cursor", "Codex CLI", "Gemini CLI"];
    let defaults = [true, true, false, false];
    let selected = dialoguer::MultiSelect::new().with_prompt("Which coding tool(s) do you want to connect? (space to toggle, enter to confirm)").items(&labels).defaults(&defaults).interact()?;
    agents.extend(selected.into_iter().map(|i| named[i].to_string()));

    while dialoguer::Confirm::new().with_prompt("Add another agent id (any Ruler-supported id not listed above)?").default(false).interact()? {
        let id: String = dialoguer::Input::new().with_prompt("Agent id").interact_text()?;
        if !id.trim().is_empty() {
            agents.push(id.trim().to_string());
        }
    }
    Ok(agents)
}

fn connect(path: &Path, agents: Vec<String>, access_mode: AccessModeArg, yes: bool, remote: Option<String>, api_key: Option<String>) -> Result<()> {
    // Whether to use local/stdio vs. remote/HTTP is not inferable from
    // anything about this invocation on its own (a solo dev can self-host
    // on their own separate personal server too) -- ask directly rather
    // than guessing, unless `--remote` already answers it or `--yes` opts
    // out of every prompt (defaulting to local, today's behavior, since a
    // non-interactive caller must opt into remote explicitly via the flag).
    let remote_url = match &remote {
        Some(url) => Some(url.trim().trim_end_matches('/').to_string()),
        None if yes => None,
        None => {
            let choice = dialoguer::Select::new()
                .with_prompt("Is agentops running on this machine, or on a separate server you'll connect to?")
                .items(&["This machine (local)", "A separate server (remote)"])
                .default(0)
                .interact()?;
            if choice == 1 {
                let url: String = dialoguer::Input::new().with_prompt("Server URL (e.g. http://192.168.1.10:3000 or https://agentops.example.com)").interact_text()?;
                Some(url.trim().trim_end_matches('/').to_string())
            } else {
                None
            }
        }
    };

    if let Some(server_url) = remote_url {
        let agents = select_agents(agents, yes)?;
        if agents.is_empty() {
            println!("No agents selected — nothing to do.");
            return Ok(());
        }
        return connect_remote(path, &server_url, api_key, &agents, yes);
    }

    let agents = select_agents(agents, yes)?;
    if agents.is_empty() {
        println!("No agents selected — nothing to do.");
        return Ok(());
    }

    agentops_ruler_bridge::preflight_check_npx()?;
    agentops_ruler_bridge::preflight_check_mcp_server_binary()?;

    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let already_scanned = agentops_manifest::list_scanned_repos()?.iter().any(|e| Path::new(&e.path) == canonical);
    if !already_scanned {
        println!("This repo hasn't been scanned yet — MCP tools need a populated graph (.context/graph.db) to return anything useful.");
        let run_install = yes || dialoguer::Confirm::new().with_prompt("Run a scan now (`agentops install`-equivalent)?").default(true).interact()?;
        if run_install {
            install(path, None, false, false, true, &[])?; // no_ruler: true -- this function handles Ruler distribution itself, right after
        } else {
            println!("Continuing without scanning — MCP tools will return little until you run `agentops install`.");
        }
    }

    let agents_md_path = path.join("AGENTS.md");
    let agents_md_content = if agents_md_path.exists() {
        std::fs::read_to_string(&agents_md_path).context("reading existing AGENTS.md")?
    } else {
        let init_result = agentops_mcp::init_agents_md(path, None).context("writing AGENTS.md")?;
        println!("Wrote {}", init_result.agents_md_path.display());
        std::fs::read_to_string(&init_result.agents_md_path)?
    };

    let access_mode_str = if matches!(access_mode, AccessModeArg::Full) { "full" } else { "advisor" };
    distribute_via_ruler(path, &agents_md_content, &agents, access_mode_str);

    println!("\nConnected: {}", agents.join(", "));
    for agent_id in &agents {
        match mcp_config_location(agent_id) {
            Some(loc) => println!("  {agent_id}: {loc} — restart {agent_id} to pick up the new MCP server."),
            None => println!("  {agent_id}: check its docs for where Ruler wrote its MCP config."),
        }
    }

    Ok(())
}

/// `.context/agentops-remote.json` -- persists a repo's "coding tools here
/// point at this remote server" choice across future `agentops install`/
/// `connect` runs that don't pass `--remote` again (e.g. re-scanning, or
/// adding another agent later). Lives in `.context/` alongside
/// `graph.db` -- a per-repo, not-meant-for-git-history directory this
/// codebase already uses for exactly this kind of local machine state,
/// deliberately not `.ruler/` (whose own files are Ruler-managed/
/// overwritten-on-every-`apply`, the opposite of what a marker needs).
#[derive(serde::Serialize, serde::Deserialize)]
struct RemoteMcpMarker {
    server_url: String,
    connection_id: String,
}

fn remote_marker_path(path: &Path) -> PathBuf {
    path.join(".context").join("agentops-remote.json")
}

fn read_remote_marker(path: &Path) -> Option<RemoteMcpMarker> {
    let content = std::fs::read_to_string(remote_marker_path(path)).ok()?;
    serde_json::from_str(&content).ok()
}

fn write_remote_marker(path: &Path, server_url: &str, connection_id: &str) -> Result<()> {
    let marker_path = remote_marker_path(path);
    if let Some(parent) = marker_path.parent() {
        std::fs::create_dir_all(parent).context("creating .context/")?;
    }
    let marker = RemoteMcpMarker { server_url: server_url.to_string(), connection_id: connection_id.to_string() };
    std::fs::write(&marker_path, serde_json::to_string_pretty(&marker)?).with_context(|| format!("writing {}", marker_path.display()))
}

#[derive(serde::Deserialize)]
struct RepoConnectionSummary {
    id: String,
    repo_url: String,
}

/// Auto-matches this checkout's `git remote get-url origin` against the
/// caller's tenant's connected repos (`GET /repos`) so the common case
/// needs no manual picking; falls back to an interactive list when there's
/// no match (a fresh clone under a different remote URL, a repo connected
/// via GitHub App rather than SSH, etc.).
/// `None` covers both "not a git repo" and "no `origin` remote configured"
/// -- indistinguishable from here, and both mean the same thing to the
/// caller (nothing to auto-match against).
fn git_remote_url(path: &Path) -> Option<String> {
    std::process::Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(path)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

fn resolve_connection_id(path: &Path, server_url: &str, api_key: &str, yes: bool) -> Result<String> {
    let mut response = ureq::get(format!("{server_url}/repos"))
        .header("Authorization", &format!("Bearer {api_key}"))
        .config()
        .http_status_as_error(false)
        .build()
        .call()
        .context("calling GET /repos")?;
    let status = response.status();
    if status.as_u16() == 401 {
        anyhow::bail!("the server rejected this API key (401) — generate a fresh one from Settings -> API Keys in the web app");
    }
    if !status.is_success() {
        let body = response.body_mut().read_to_string().unwrap_or_default();
        anyhow::bail!("GET /repos returned {status}: {body}");
    }
    let body: serde_json::Value = response.body_mut().read_json().context("parsing GET /repos response")?;
    let connections: Vec<RepoConnectionSummary> = body.get("connections").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default();

    if connections.is_empty() {
        anyhow::bail!("no repos are connected to your organization yet — connect one from the web app first (Repositories -> Connect a repository), then run this again");
    }

    let git_remote = git_remote_url(path);

    if let Some(remote) = &git_remote {
        if let Some(matched) = connections.iter().find(|c| &c.repo_url == remote) {
            println!("Matched this checkout's git remote ({remote}) to the connected repo {:?}.", matched.id);
            return Ok(matched.id.clone());
        }
    }

    if yes {
        anyhow::bail!("couldn't auto-match this checkout's git remote to a connected repo, and --yes was set — run again without --yes to pick one interactively");
    }

    // "No remote at all" and "remote present but unmatched" are different
    // situations needing different advice -- a repo with no remote is
    // never going to auto-match no matter which directory you're in
    // (that's the local-only case the web app's own /repositories/connect/local
    // page gives the same advice for), while an unmatched remote often
    // just means the command was run from the wrong directory, which the
    // "check a different path" prompt below can actually fix.
    match &git_remote {
        None => println!("This checkout has no git remote at all. If it's meant to stay local-only, run `agentops install`/`agentops connect` (without --remote) instead — otherwise pick the right connected repo below, or point at a different local path."),
        Some(remote) => println!("This checkout's git remote ({remote}) doesn't match any of your connected repos."),
    }

    if dialoguer::Confirm::new().with_prompt("Check a different local path instead?").default(false).interact()? {
        let path_str: String = dialoguer::Input::new().with_prompt("Path to check").interact_text()?;
        let other_path = Path::new(&path_str);
        match git_remote_url(other_path) {
            Some(remote) => match connections.iter().find(|c| c.repo_url == remote) {
                Some(matched) => {
                    println!("Matched {}'s git remote ({remote}) to the connected repo {:?}.", other_path.display(), matched.id);
                    return Ok(matched.id.clone());
                }
                None => println!("That path's remote ({remote}) doesn't match any connected repo either."),
            },
            None => println!("No git remote found at that path either."),
        }
    }

    let labels: Vec<String> = connections.iter().map(|c| format!("{} ({})", c.repo_url, c.id)).collect();
    let selected = dialoguer::Select::new().with_prompt("Which connected repo is this?").items(&labels).interact()?;
    Ok(connections[selected].id.clone())
}

fn connect_remote(path: &Path, server_url: &str, api_key: Option<String>, agents: &[String], yes: bool) -> Result<()> {
    let api_key = match api_key {
        Some(k) => k,
        None if yes => anyhow::bail!("--remote requires --api-key when --yes is set (nothing to prompt for non-interactively)"),
        None => {
            println!("Generate a personal API key from Settings -> API Keys in the web app, then paste it below (or pass --api-key next time).");
            dialoguer::Password::new().with_prompt("API key").interact()?
        }
    };

    let connection_id = resolve_connection_id(path, server_url, &api_key, yes)?;

    let agents_md_path = path.join("AGENTS.md");
    let agents_md_content = if agents_md_path.exists() {
        std::fs::read_to_string(&agents_md_path).context("reading existing AGENTS.md")?
    } else {
        let init_result = agentops_mcp::init_agents_md(path, None).context("writing AGENTS.md")?;
        println!("Wrote {}", init_result.agents_md_path.display());
        std::fs::read_to_string(&init_result.agents_md_path)?
    };

    // Written before `distribute_via_ruler` runs -- that fn checks for
    // this marker to skip the stdio `.ruler/mcp.json` entry and re-apply
    // the remote native entries as its own last step, every time it runs
    // from here on (not just this one call). See its doc comment.
    write_remote_marker(path, server_url, &connection_id)?;
    distribute_via_ruler(path, &agents_md_content, agents, "advisor");

    println!("\nConnected to {server_url} (repo: {connection_id}).");
    println!("Export your API key locally before using your coding tool: export AGENTOPS_API_KEY={api_key}");
    for agent_id in agents {
        match mcp_config_location(agent_id) {
            Some(loc) => println!("  {agent_id}: {loc} — restart {agent_id} to pick up the new MCP server."),
            None => println!("  {agent_id}: not a recognized agent id for a remote entry — check its docs for how to register a Streamable HTTP MCP server manually."),
        }
    }
    Ok(())
}

/// Writes/refreshes the `"agentops"` entry directly in each target agent's
/// own native MCP config file, deliberately bypassing Ruler for this one
/// entry: each of the four vendors uses a genuinely different remote-MCP
/// schema (Claude Code needs an explicit `"type":"http"`; Cursor doesn't;
/// Codex CLI uses a separate `bearer_token_env_var` key, not string
/// interpolation; Gemini CLI's header env-var substitution isn't confirmed
/// by its own docs) -- verified against each vendor's own current docs
/// individually, matching this codebase's established "verify empirically,
/// don't assume" convention, rather than trusting a single generic entry
/// translated by a pinned, unverified-for-this-case Ruler version to get
/// all four right. Read-modify-write: any *other* MCP server already
/// configured in these files is preserved untouched. Any agent id outside
/// this table (Ruler-supported but not one of the four with a known
/// remote schema) is skipped with a note in `connect_remote`'s own
/// closing summary -- its prompt-pack distribution via Ruler still works,
/// just not an automatic remote MCP entry.
fn write_remote_mcp_entries(path: &Path, agent_ids: &[String], server_url: &str) {
    let url = format!("{server_url}/mcp");
    for agent_id in agent_ids {
        let Some(rel_path) = mcp_config_location(agent_id) else { continue };
        let config_path = path.join(rel_path);
        let result = if agent_id == "codex" { write_codex_remote_entry(&config_path, &url) } else { write_json_mcp_remote_entry(agent_id, &config_path, &url) };
        if let Err(e) = result {
            println!("WARNING: failed to write a remote MCP entry to {}: {e}", config_path.display());
        }
    }
}

/// Claude Code (`.mcp.json`), Cursor (`.cursor/mcp.json`), and Gemini CLI
/// (`.gemini/settings.json`) all use the same `{"mcpServers": {...}}` JSON
/// shape, differing only in the per-agent fields set here -- see this
/// module's `write_remote_mcp_entries` doc comment for the schema sources.
fn write_json_mcp_remote_entry(agent_id: &str, config_path: &Path, url: &str) -> Result<()> {
    let mut root: serde_json::Value = if config_path.exists() {
        serde_json::from_str(&std::fs::read_to_string(config_path).with_context(|| format!("reading {}", config_path.display()))?).unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };
    if !root.is_object() {
        root = serde_json::json!({});
    }
    let entry = match agent_id {
        "claude" => serde_json::json!({ "type": "http", "url": url, "headers": { "Authorization": "Bearer ${AGENTOPS_API_KEY}" } }),
        "gemini-cli" => serde_json::json!({ "httpUrl": url, "headers": { "Authorization": "Bearer ${AGENTOPS_API_KEY}" } }),
        _ => serde_json::json!({ "url": url, "headers": { "Authorization": "Bearer ${env:AGENTOPS_API_KEY}" } }), // cursor
    };
    let root_obj = root.as_object_mut().expect("just normalized to an object above");
    let servers = root_obj.entry("mcpServers").or_insert_with(|| serde_json::json!({}));
    if !servers.is_object() {
        *servers = serde_json::json!({});
    }
    servers.as_object_mut().expect("just normalized to an object above").insert("agentops".to_string(), entry);

    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(config_path, serde_json::to_string_pretty(&root)?).with_context(|| format!("writing {}", config_path.display()))
}

/// Codex CLI's `.codex/config.toml` reads the bearer token from a *named
/// env var* (`bearer_token_env_var`), not string interpolation inside
/// `url`/headers the way the other three do — see `write_remote_mcp_
/// entries`'s doc comment.
fn write_codex_remote_entry(config_path: &Path, url: &str) -> Result<()> {
    let mut root: toml::Value = if config_path.exists() {
        std::fs::read_to_string(config_path).with_context(|| format!("reading {}", config_path.display()))?.parse().unwrap_or_else(|_| toml::Value::Table(Default::default()))
    } else {
        toml::Value::Table(Default::default())
    };
    let table = root.as_table_mut().ok_or_else(|| anyhow::anyhow!("{} is not a TOML table at its root", config_path.display()))?;
    let mcp_servers = table.entry("mcp_servers".to_string()).or_insert_with(|| toml::Value::Table(Default::default()));
    let mcp_servers_table = mcp_servers.as_table_mut().ok_or_else(|| anyhow::anyhow!("mcp_servers is not a table in {}", config_path.display()))?;

    let mut entry = toml::value::Table::new();
    entry.insert("url".to_string(), toml::Value::String(url.to_string()));
    entry.insert("bearer_token_env_var".to_string(), toml::Value::String("AGENTOPS_API_KEY".to_string()));
    mcp_servers_table.insert("agentops".to_string(), toml::Value::Table(entry));

    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(config_path, toml::to_string_pretty(&root)?).with_context(|| format!("writing {}", config_path.display()))
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

fn task_create(path: &Path, title: &str, description: Option<&str>, priority: Option<&str>, assignee: Option<&str>, session_id: Option<&str>) -> Result<()> {
    let (store, repo) = open_store(path)?;
    let id = store.create_task(agentops_graph::NewTask {
        repo,
        title: title.to_string(),
        description: description.map(String::from),
        status: agentops_graph::TaskStatus::Todo,
        priority: priority.map(String::from),
        assignee: assignee.map(String::from),
        external_source: None,
        external_id: None,
        session_id: session_id.map(String::from),
    })?;
    println!("Created task {id}: {title}");
    Ok(())
}

fn task_list(path: &Path) -> Result<()> {
    let (store, repo) = open_store(path)?;
    let tasks = store.list_tasks(&repo)?;
    if tasks.is_empty() {
        println!("No tasks recorded.");
        return Ok(());
    }
    for t in &tasks {
        println!("[{}] task {}: {}{}", t.status.as_db_str(), t.id, t.title, t.external_source.as_deref().map(|s| format!(" (synced from {s})")).unwrap_or_default());
    }
    Ok(())
}

fn task_update_status(path: &Path, id: i64, status: agentops_graph::TaskStatus) -> Result<()> {
    let (store, _repo) = open_store(path)?;
    store.update_task_status(id, status)?;
    println!("Task {id} moved to {}", status.as_db_str());
    Ok(())
}

fn task_activity(path: &Path, id: i64) -> Result<()> {
    let (store, repo) = open_store(path)?;
    let task = store.get_task(id)?.ok_or_else(|| anyhow::anyhow!("task {id} not found"))?;
    let Some(session_id) = &task.session_id else {
        println!("Task {id} ({}) has no session_id — nothing to correlate.", task.title);
        return Ok(());
    };
    let events = store.session_events(&repo, session_id)?;
    if events.is_empty() {
        println!("Task {id} ({}) has session_id '{session_id}' but no activity recorded under it yet.", task.title);
        return Ok(());
    }
    for e in &events {
        println!("[{}] {}: {}", e.created_at, e.tool_name, e.description);
    }
    Ok(())
}

fn task_summarize(path: &Path, id: i64) -> Result<()> {
    let (store, repo) = open_store(path)?;
    let task = store.get_task(id)?.ok_or_else(|| anyhow::anyhow!("task {id} not found"))?;
    let Some(session_id) = &task.session_id else {
        anyhow::bail!("task {id} ({}) has no session_id — nothing to summarize", task.title);
    };
    let events = store.session_events(&repo, session_id)?;
    if events.is_empty() {
        anyhow::bail!("task {id} ({}) has session_id '{session_id}' but no activity recorded under it yet", task.title);
    }

    let config = agentops_llm::AnthropicConfig::from_env()?;
    let summaries = agentops_llm::summarize_task_activity(&config, &task.title, &events)?;

    println!("Technical:\n{}\n", summaries.technical);
    println!("Non-technical:\n{}\n", summaries.non_technical);
    println!("Client-friendly:\n{}\n", summaries.client_friendly);

    if task.external_source.as_deref() == Some("linear") {
        if let Some(external_id) = &task.external_id {
            match agentops_linear::LinearConfig::from_env() {
                Ok(linear_config) => {
                    agentops_linear::post_comment(&linear_config, external_id, &format!("**Technical summary**\n\n{}", summaries.technical))?;
                    agentops_linear::post_comment(&linear_config, external_id, &format!("**Non-technical summary**\n\n{}", summaries.non_technical))?;
                    println!("Posted technical + non-technical summaries as comments on {external_id}.");
                }
                Err(e) => println!("Skipping Linear comment post ({e:#})."),
            }
        }
    }

    Ok(())
}

fn task_sync_linear(path: &Path, limit: u32, push: Option<i64>, status_name: Option<&str>) -> Result<()> {
    let (store, repo) = open_store(path)?;
    let config = agentops_linear::LinearConfig::from_env()?;

    // Push *before* pulling — a real bug caught live-testing this against
    // the AgentOps testing Linear project: pulling first calls
    // `upsert_external_task`, which overwrites the very local status
    // change `--push` was about to send, so the push would silently send
    // whatever Linear already had (a no-op) instead of the local change.
    // Pushing first means the local change reaches Linear before anything
    // local gets overwritten; the pull that follows then correctly reflects
    // it back, along with everything else that changed upstream.
    if let Some(task_id) = push {
        agentops_linear::sync_push(store.as_ref(), &config, task_id, status_name)?;
        println!("Pushed task {task_id}'s status to Linear.");
    }

    let synced = agentops_linear::pull_issues(store.as_ref(), &config, &repo, limit)?;
    println!("Pulled {synced} issue(s) from Linear.");
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

fn search(path: &Path, top_k: usize, kind: Option<SearchKindArg>, hybrid: bool, graph_expand: bool, query: &str) -> Result<()> {
    use agentops_embeddings::Embedder;

    let (store, repo) = open_store(path)?;
    let kind = kind.map(NodeKind::from);

    if hybrid {
        let hits = agentops_retrieval::search_hybrid(store.as_ref(), &agentops_embeddings::LocalEmbedder, &repo, query, top_k, kind, graph_expand, None)?;
        if hits.is_empty() {
            println!("No matches.");
            return Ok(());
        }
        for h in &hits {
            let signals = [h.dense_rank.map(|_| "dense"), h.lexical_rank.map(|_| "lexical"), h.exact_rank.map(|_| "exact")].into_iter().flatten().collect::<Vec<_>>().join("+");
            let graph = h.graph_score.filter(|s| *s > 0.0).map(|s| format!(", graph {s:.4}")).unwrap_or_default();
            println!("{:?} {} (score {:.4}, signals: {signals}{graph}){}", h.node.kind, h.node.name.as_deref().unwrap_or("(untitled)"), h.fused_score, h.node.path.as_deref().map(|p| format!(" — {p}")).unwrap_or_default());
        }
        return Ok(());
    }

    let embedding = agentops_embeddings::LocalEmbedder.embed(query)?;
    let hits = store.search_similar(&repo, &embedding, top_k, kind)?;

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
/// GitHub search — both run via the shared `agentops_mcp::sync_docs` (also
/// used by the Linear webhook auto-kickoff dispatch path, which has no
/// terminal to prompt against) — then (3) ask the user, implemented here
/// since step 3 is inherently an interactive CLI concern. Single-tenant
/// this pass (`docbrain-graph` dropped `TenantContext`), so no `--org` flag
/// or visibility plumbing.
fn sync_docs(path: &Path, db: Option<&Path>, no_interactive: bool) -> Result<()> {
    let shared = agentops_mcp::sync_docs(path, db).context("running shared registry/GitHub discovery")?;

    println!("{} already known, {} discovered (registry/GitHub), {} unresolved after automatic discovery.", shared.already_known, shared.discovered, shared.unresolved.len());

    let mut asked = 0u32;
    let mut still_unresolved = Vec::new();
    if !shared.unresolved.is_empty() {
        let store = docbrain_graph::SqliteDocbrainStore::open(&db.map(Path::to_path_buf).unwrap_or_else(docbrain_mcp::default_db_path)).context("opening docbrain store")?;
        for name in &shared.unresolved {
            // Step 3: ask the user, interactively, right now — rather than
            // just printing a list to act on later. `interact_text()`
            // requires a real TTY (dialoguer reads raw terminal input, not
            // just redirected stdin); in a non-interactive context (CI,
            // piped input) it errors, and `.unwrap_or_default()` treats
            // that the same as "left blank" — a safe no-op, not a crash.
            if !no_interactive {
                let prompt = format!("'{name}' not found via registry or GitHub search. Enter a docs URL (blank to skip)");
                let answer: String = dialoguer::Input::new().with_prompt(prompt).allow_empty(true).interact_text().unwrap_or_default();
                if !answer.trim().is_empty() {
                    use docbrain_graph::DocbrainStore;
                    store.add_library(name, name, None, None, Some(answer.trim()))?;
                    println!("  registered (user-provided): {name} -> {}", answer.trim());
                    asked += 1;
                    continue;
                }
            }
            still_unresolved.push(name.clone());
        }
    }

    if asked > 0 {
        println!("{asked} registered from user input.");
    }
    if !still_unresolved.is_empty() {
        println!("Not found via any discovery step, and not provided manually:");
        for name in &still_unresolved {
            println!("  - {name}");
        }
    }

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


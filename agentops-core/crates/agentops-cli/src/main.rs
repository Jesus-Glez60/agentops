//! agentops — single entry-point CLI for the agentops light tier.
//!
//! Phase 1 skeleton. See /Users/jesusglez/.claude/plans/i-m-thinking-that-now-modular-sparrow.md
//! for the full command flow this is meant to grow into.

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(name = "agentops", version, about = "Codebrain/Docbrain light-tier CLI")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Install the prompt pack (via Ruler) and optionally scan repos.
    Install {
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        yes: bool,
        #[arg(long, value_enum, default_value_t = AccessMode::Advisor)]
        access_mode: AccessMode,
    },
    /// Generate onboarding/engineering documentation from an already-scanned repo.
    Docgen {
        #[arg(long)]
        repo: String,
    },
    /// Add a gotcha/decision note, edge-connected to a symbol in the graph.
    Note {
        #[arg(long)]
        affects: String,
        text: String,
    },
    /// Show what's currently installed/scanned.
    Status,
}

#[derive(Clone, Copy, ValueEnum, Debug)]
enum AccessMode {
    /// Plans/reviews only — write-capable tools are never registered.
    Advisor,
    /// Full agent access, including code creation/modification.
    Full,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Install { dry_run, yes, access_mode } => {
            println!(
                "install: dry_run={dry_run} yes={yes} access_mode={access_mode:?} — {}",
                agentops_scanner::placeholder()
            );
            println!("{}", agentops_ruler_bridge::placeholder());
            println!("{}", agentops_agents_md::placeholder());
        }
        Command::Docgen { repo } => {
            println!("docgen for '{repo}' — {}", agentops_docgen::placeholder());
        }
        Command::Note { affects, text } => {
            println!("note affecting '{affects}': {text} — {}", agentops_notes::placeholder());
        }
        Command::Status => {
            println!("{}", agentops_manifest::placeholder());
        }
    }

    Ok(())
}

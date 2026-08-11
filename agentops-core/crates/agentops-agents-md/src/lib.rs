//! Generates a spec-compliant `AGENTS.md` — the vendor-neutral, open
//! standard read by 20+ coding agents. Deliberately plain Markdown: no
//! `@include` directives (a Claude-Code-specific mechanism other agents
//! don't resolve).
//!
//! Emits a structured `NOTES_PATH` marker — the confirmed missing link in
//! `main`'s design: the installed prompt pack's write-back instructions
//! already expected `AGENTS.md` to declare where new notes go, but this
//! generator never wrote it, so read (ingestion) and write (agent-authored
//! notes) never actually agreed on a location. The caller resolves the
//! path (via `agentops_notes::resolve_notes_path`, so this crate doesn't
//! need a dependency on `agentops-notes` just to emit a string) and passes
//! it in through `GenerateOptions`.

use std::path::Path;

/// Best-effort stack detection (package.json/pyproject.toml/go.mod/
/// Cargo.toml/pubspec.yaml/*.tf).
fn detect_stack(root: &Path) -> String {
    if let Ok(pkg) = std::fs::read_to_string(root.join("package.json")) {
        let framework = ["next", "nuxt", "remix", "react", "vue", "svelte", "express", "fastify", "nestjs", "react-native"].iter().find(|f| pkg.contains(&format!("\"{f}\""))).copied();
        return match framework {
            Some(f) => format!("Node.js, TypeScript/JavaScript ({f})"),
            None => "Node.js, TypeScript/JavaScript".to_string(),
        };
    }

    if let Ok(pyproject) = std::fs::read_to_string(root.join("pyproject.toml")) {
        let framework = ["fastapi", "django", "flask", "dbt", "airflow"].iter().find(|f| pyproject.to_lowercase().contains(*f)).copied();
        return match framework {
            Some(f) => format!("Python ({f})"),
            None => "Python".to_string(),
        };
    }
    if root.join("requirements.txt").exists() || root.join("setup.py").exists() {
        return "Python".to_string();
    }

    if root.join("go.mod").exists() {
        return "Go".to_string();
    }
    if root.join("Cargo.toml").exists() {
        return "Rust".to_string();
    }
    if root.join("pubspec.yaml").exists() {
        return "Flutter, Dart".to_string();
    }
    if has_extension(root, "tf") {
        return "Terraform".to_string();
    }

    "Unknown".to_string()
}

fn has_extension(root: &Path, ext: &str) -> bool {
    std::fs::read_dir(root).into_iter().flatten().flatten().any(|e| e.path().extension().and_then(|e| e.to_str()) == Some(ext))
}

fn build_test_commands(stack: &str) -> (&'static str, &'static str) {
    if stack.starts_with("Node.js") {
        ("npm install", "npm test")
    } else if stack.starts_with("Python") {
        ("pip install -r requirements.txt", "pytest")
    } else if stack == "Go" {
        ("go build ./...", "go test ./...")
    } else if stack == "Rust" {
        ("cargo build", "cargo test")
    } else if stack == "Flutter, Dart" {
        ("flutter pub get", "flutter test")
    } else {
        ("<none detected>", "<none detected>")
    }
}

/// Options controlling optional sections.
#[derive(Default)]
pub struct GenerateOptions {
    pub claude_code_installed: bool,
    pub repo_map_path: Option<String>,
    /// The resolved notes location (see `agentops_notes::resolve_notes_path`)
    /// — emitted verbatim as a `NOTES_PATH:` marker so the installed
    /// write-back prompt instructions can read it dynamically instead of
    /// assuming a hardcoded path.
    pub notes_path: Option<String>,
}

/// Generates AGENTS.md content for the repo at `root`.
pub fn generate(root: &Path, opts: &GenerateOptions) -> String {
    let stack = detect_stack(root);
    let (build_cmd, test_cmd) = build_test_commands(&stack);
    let repo_name = root.file_name().and_then(|n| n.to_str()).unwrap_or("this project");

    let mut out = String::new();
    out.push_str(&format!("# {repo_name}\n\n"));
    out.push_str("## Overview\n\n");
    out.push_str(&format!("Stack: {stack} (auto-detected — correct this if wrong).\n\n"));

    out.push_str("## Build & test commands\n\n");
    out.push_str(&format!("```\n{build_cmd}\n{test_cmd}\n```\n\n"));

    out.push_str("## Code style\n\n");
    out.push_str("Follow existing conventions in the codebase. No project-specific style guide detected yet.\n\n");

    out.push_str("## Testing\n\n");
    out.push_str("Run the test command above before considering a change complete.\n\n");

    out.push_str("## Security\n\n");
    out.push_str(
        "Source content referenced by this project's context files has passed through a \
         secret-redaction gate (see SECURITY.md) — credential-shaped strings are replaced \
         with `[REDACTED:<rule>]` markers, not included verbatim.\n\n",
    );

    if let Some(notes_path) = &opts.notes_path {
        out.push_str("## Notes & Knowledge\n\n");
        out.push_str(&format!(
            "NOTES_PATH: {notes_path}\n\n\
             New gotchas, decisions, and other project knowledge learned while working here \
             should be recorded at the path above — via the `add_note` MCP tool if available, \
             or as a Markdown file with `title`/`type`/`tags` frontmatter directly in that \
             folder otherwise. This applies whether or not the folder already has anything in \
             it.\n\n"
        ));
    }

    out.push_str("## Commit / PR guidelines\n\n");
    out.push_str("No project-specific convention detected yet — follow the existing commit history's style.\n\n");
    out.push_str("Do not include a `Co-Authored-By` trailer (or similar AI-attribution trailer) in commit messages.\n\n");

    out.push_str("## Deployment\n\n");
    out.push_str("No deployment process detected yet.\n\n");

    if let Some(repo_map) = &opts.repo_map_path {
        out.push_str("## Repository map\n\n");
        out.push_str(&format!("See [{repo_map}]({repo_map}) for a ranked overview of this codebase's structure.\n\n"));
    }

    if opts.claude_code_installed {
        out.push_str("## Claude Code\n\n");
        out.push_str("This project has the Claude Code prompt pack installed. Useful commands: `/session`, `/plan`, `/audit-plan`, `/wrap`.\n\n");
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn detects_nextjs_stack_from_package_json() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("package.json"), r#"{"dependencies": {"next": "16.0.0"}}"#).unwrap();

        let content = generate(dir.path(), &GenerateOptions::default());
        assert!(content.contains("Node.js, TypeScript/JavaScript (next)"));
        assert!(content.contains("npm install"));
    }

    #[test]
    fn detects_fastapi_stack_from_pyproject() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("pyproject.toml"), "[project]\ndependencies = [\"fastapi\"]\n").unwrap();

        let content = generate(dir.path(), &GenerateOptions::default());
        assert!(content.contains("Python (fastapi)"));
        assert!(content.contains("pytest"));
    }

    #[test]
    fn falls_back_to_unknown_stack() {
        let dir = tempfile::tempdir().unwrap();
        let content = generate(dir.path(), &GenerateOptions::default());
        assert!(content.contains("Stack: Unknown"));
    }

    #[test]
    fn does_not_reference_claude_slash_commands_unless_opted_in() {
        let dir = tempfile::tempdir().unwrap();
        let content = generate(dir.path(), &GenerateOptions::default());
        assert!(!content.contains("/session"));

        let opts = GenerateOptions { claude_code_installed: true, ..Default::default() };
        let content_with_claude = generate(dir.path(), &opts);
        assert!(content_with_claude.contains("/session"));
    }

    #[test]
    fn never_emits_include_directives() {
        let dir = tempfile::tempdir().unwrap();
        let content = generate(dir.path(), &GenerateOptions::default());
        assert!(!content.contains("@include"), "AGENTS.md must be vendor-neutral, no Claude-specific includes");
    }

    /// The actual point of this pass: confirms the missing link from
    /// `main`'s design is closed — a resolved notes path is now really
    /// emitted into the generated file.
    #[test]
    fn emits_notes_path_when_given_one_and_omits_the_section_otherwise() {
        let dir = tempfile::tempdir().unwrap();

        let without = generate(dir.path(), &GenerateOptions::default());
        assert!(!without.contains("NOTES_PATH"));

        let opts = GenerateOptions { notes_path: Some("/repo/.agentops/notes".to_string()), ..Default::default() };
        let with = generate(dir.path(), &opts);
        assert!(with.contains("NOTES_PATH: /repo/.agentops/notes"));
        assert!(with.contains("add_note"));
    }

    /// Phase 6b, Extension 3 (corrected design): AgentOps doesn't create
    /// git commits itself, so trailer hygiene has to be an instruction the
    /// coding agent reading this file follows — not a git-rewrite step
    /// AgentOps would have to own.
    #[test]
    fn commit_guidelines_instruct_against_a_co_authored_by_trailer() {
        let dir = tempfile::tempdir().unwrap();
        let content = generate(dir.path(), &GenerateOptions::default());
        assert!(content.contains("Co-Authored-By"), "must instruct coding agents not to add this trailer");
    }
}

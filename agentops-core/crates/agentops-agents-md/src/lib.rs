//! Generates a spec-compliant `AGENTS.md` — the vendor-neutral, Linux-Foundation
//! Agentic-AI-Foundation-governed open standard (formalized Aug 2025 by
//! OpenAI/Google/Cursor/Factory) read by 20+ coding agents. Deliberately plain
//! Markdown: no `@include` directives (that's a Claude-Code-specific mechanism
//! Cursor/Codex/etc. don't resolve).

use std::path::Path;

/// Best-effort stack detection, mirroring `dev-agent-ops`'s `wire-repos.nu`
/// heuristics (package.json/pyproject.toml/go.mod/Cargo.toml/pubspec.yaml/*.tf),
/// ported to Rust.
fn detect_stack(root: &Path) -> String {
    if let Ok(pkg) = std::fs::read_to_string(root.join("package.json")) {
        let framework = ["next", "nuxt", "remix", "react", "vue", "svelte", "express", "fastify", "nestjs", "react-native"]
            .iter()
            .find(|f| pkg.contains(&format!("\"{f}\"")))
            .copied();
        return match framework {
            Some(f) => format!("Node.js, TypeScript/JavaScript ({f})"),
            None => "Node.js, TypeScript/JavaScript".to_string(),
        };
    }

    if let Ok(pyproject) = std::fs::read_to_string(root.join("pyproject.toml")) {
        let framework = ["fastapi", "django", "flask", "dbt", "airflow"]
            .iter()
            .find(|f| pyproject.to_lowercase().contains(*f))
            .copied();
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
    std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .any(|e| e.path().extension().and_then(|e| e.to_str()) == Some(ext))
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

/// Options controlling optional sections — e.g. whether to reference Claude
/// Code slash commands, which only makes sense if the Claude Code prompt pack
/// was actually installed for this project (see the plan's §Repo structure).
#[derive(Default)]
pub struct GenerateOptions {
    pub claude_code_installed: bool,
    pub repo_map_path: Option<String>,
}

/// Generates AGENTS.md content for the repo at `root`.
pub fn generate(root: &Path, opts: &GenerateOptions) -> String {
    let stack = detect_stack(root);
    let (build_cmd, test_cmd) = build_test_commands(&stack);
    let repo_name = root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("this project");

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

    out.push_str("## Commit / PR guidelines\n\n");
    out.push_str("No project-specific convention detected yet — follow the existing commit history's style.\n\n");

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

        let opts = GenerateOptions { claude_code_installed: true, repo_map_path: None };
        let content_with_claude = generate(dir.path(), &opts);
        assert!(content_with_claude.contains("/session"));
    }

    #[test]
    fn never_emits_include_directives() {
        let dir = tempfile::tempdir().unwrap();
        let content = generate(dir.path(), &GenerateOptions::default());
        assert!(!content.contains("@include"), "AGENTS.md must be vendor-neutral, no Claude-specific includes");
    }
}

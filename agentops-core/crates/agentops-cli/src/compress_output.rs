//! `agentops compress-output --kind <kind>` -- reads a command's combined
//! stdout+stderr from stdin, compresses it per `kind`, and writes the
//! compressed result to stdout. One stage of a `PreToolUse`-rewritten
//! command's pipeline (see `hook_capture::rewrite_command`) -- never
//! invoked directly by a human, and never itself talks to a `GraphStore` or
//! any other agentops state (a pure stdin -> stdout filter).
//!
//! Reuse-before-writing check: no test-output-parsing crate (`nextest`,
//! `cargo2junit`, `libtest-mimic`, ...) exists anywhere in this workspace,
//! and the actual requirement here -- "count passing/skipped lines, keep
//! failing ones (plus a bounded trailing context block) verbatim, always
//! keep the final summary line" -- doesn't need one. `regex` is available
//! workspace-wide but isn't used here either: every classification below is
//! a plain prefix/suffix/substring check, matching
//! `hook_capture::is_git_op`'s own stated preference for that over regex
//! wherever a substring check already suffices.

use std::io::Read;

use anyhow::{Context, Result};
use clap::ValueEnum;

/// `git diff`/`git log` are deliberately **not** members of this enum --
/// see `hook_capture::classify_for_compression`'s `CompressAction`: those
/// two are handled by appending a narrower flag directly to the rewritten
/// command (`-U1`, `--oneline`) rather than piping through this binary at
/// all, since git already produces the compact form itself given the right
/// flag -- the smallest possible implementation for those two cases.
#[derive(Clone, Copy, ValueEnum, Debug, PartialEq, Eq)]
pub enum CompressKind {
    TestRust,
    TestNode,
    TestPytest,
    TestGo,
    Grep,
}

impl CompressKind {
    /// The exact string passed on the rewritten command line
    /// (`--kind <as_flag()>`) and parsed back by clap's `ValueEnum` --
    /// spelled out explicitly rather than relied on from the derive macro,
    /// matching this codebase's existing `as_db_str`-style convention
    /// (`NodeKind`/`NodeProminence`/`EdgeRelation` in `agentops-graph`).
    pub fn as_flag(self) -> &'static str {
        match self {
            CompressKind::TestRust => "test-rust",
            CompressKind::TestNode => "test-node",
            CompressKind::TestPytest => "test-pytest",
            CompressKind::TestGo => "test-go",
            CompressKind::Grep => "grep",
        }
    }
}

pub fn run(kind: CompressKind) -> Result<()> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).context("reading command output from stdin")?;
    let compressed = match kind {
        CompressKind::TestRust | CompressKind::TestNode | CompressKind::TestPytest | CompressKind::TestGo => compress_test_output(&input, kind),
        CompressKind::Grep => compress_grep(&input),
    };
    print!("{compressed}");
    Ok(())
}

struct TestMarkers {
    is_pass: fn(&str) -> bool,
    is_fail: fn(&str) -> bool,
    is_summary: fn(&str) -> bool,
}

/// Only ever called for the four test-runner kinds -- `Grep` is dispatched
/// separately in `run` and never reaches this function, hence the
/// `unreachable!` arm below rather than a fifth, meaningless `TestMarkers`.
fn markers_for(kind: CompressKind) -> TestMarkers {
    match kind {
        CompressKind::TestRust => TestMarkers {
            is_pass: |l| {
                let t = l.trim();
                t.starts_with("test ") && (t.ends_with("... ok") || t.ends_with("... ignored"))
            },
            is_fail: |l| {
                let t = l.trim();
                t.starts_with("test ") && t.ends_with("... FAILED")
            },
            is_summary: |l| l.trim_start().starts_with("test result:"),
        },
        CompressKind::TestNode => TestMarkers {
            is_pass: |l| {
                let t = l.trim_start();
                t.starts_with('✓') || t.starts_with("PASS ")
            },
            is_fail: |l| {
                let t = l.trim_start();
                t.starts_with('✗') || t.starts_with('✕') || t.starts_with("FAIL ")
            },
            is_summary: |l| {
                let t = l.trim_start();
                t.starts_with("Tests:") || t.starts_with("Test Suites:")
            },
        },
        CompressKind::TestPytest => TestMarkers {
            is_pass: |l| l.trim_end().ends_with(" PASSED"),
            is_fail: |l| l.trim_end().ends_with(" FAILED") || l.trim_start().starts_with("FAILED "),
            is_summary: |l| {
                let t = l.trim();
                t.starts_with('=') && (t.contains("passed") || t.contains("failed") || t.contains("error"))
            },
        },
        CompressKind::TestGo => TestMarkers {
            is_pass: |l| l.trim_start().starts_with("--- PASS:"),
            is_fail: |l| l.trim_start().starts_with("--- FAIL:"),
            is_summary: |l| {
                let t = l.trim_start();
                t.starts_with("ok ") || t.starts_with("ok\t") || t.starts_with("FAIL\t") || t == "PASS" || t == "FAIL"
            },
        },
        CompressKind::Grep => unreachable!("compress_test_output is never called for CompressKind::Grep -- see run()'s own dispatch"),
    }
}

/// How many lines of a failing test's own trailing output (traceback,
/// assertion diff, panic message) are kept verbatim before the next
/// pass/fail/summary marker cuts it off.
const FAILURE_CONTEXT_LINES: usize = 20;

fn flush_collapsed(out: &mut Vec<String>, collapsed: &mut usize) {
    if *collapsed > 0 {
        out.push(format!("… {collapsed} passing/skipped test line(s) collapsed …"));
        *collapsed = 0;
    }
}

/// Collapses every passing/skipped test line into a running count, keeps
/// every failing test line plus a bounded trailing context block verbatim,
/// and always preserves the final summary line unmodified -- that line
/// typically carries the "N failed" signal a human/agent actually needs,
/// distinct from the process's real exit code (preserved separately, at the
/// shell level, by `hook_capture::rewrite_command`'s `PIPESTATUS` handling).
fn compress_test_output(text: &str, kind: CompressKind) -> String {
    let TestMarkers { is_pass, is_fail, is_summary } = markers_for(kind);
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<String> = Vec::new();
    let mut collapsed = 0usize;
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];
        if is_pass(line) {
            collapsed += 1;
            i += 1;
            continue;
        }
        if is_fail(line) {
            flush_collapsed(&mut out, &mut collapsed);
            out.push(line.to_string());
            i += 1;
            let mut kept = 0;
            while i < lines.len() && kept < FAILURE_CONTEXT_LINES {
                let next = lines[i];
                if is_pass(next) || is_fail(next) || is_summary(next) {
                    break;
                }
                out.push(next.to_string());
                kept += 1;
                i += 1;
            }
            continue;
        }
        if is_summary(line) {
            flush_collapsed(&mut out, &mut collapsed);
            out.push(line.to_string());
            i += 1;
            continue;
        }
        out.push(line.to_string());
        i += 1;
    }
    flush_collapsed(&mut out, &mut collapsed);
    out.join("\n")
}

/// Parses one grep/rg `path:line:content` match line. `None` for anything
/// else (a header, "Binary file ... matches", a warning) -- those pass
/// through unmodified rather than being misclassified as a match.
fn parse_grep_match(line: &str) -> Option<(&str, &str)> {
    let (path, rest) = line.split_once(':')?;
    let (line_no, content) = rest.split_once(':')?;
    if path.is_empty() || line_no.parse::<u64>().is_err() {
        return None;
    }
    Some((path, content))
}

const MAX_MATCHES_PER_FILE: usize = 10;

/// Groups `path:line:content` matches by file with a per-file cap, instead
/// of one line per match -- a `grep -rn` across a large tree can otherwise
/// dump hundreds of near-identical lines into context for what's really
/// "these N files matched."
fn compress_grep(text: &str) -> String {
    let mut order: Vec<&str> = Vec::new();
    let mut by_file: std::collections::HashMap<&str, Vec<&str>> = std::collections::HashMap::new();
    let mut passthrough: Vec<&str> = Vec::new();

    for line in text.lines() {
        match parse_grep_match(line) {
            Some((path, content)) => {
                by_file.entry(path).or_insert_with(|| {
                    order.push(path);
                    Vec::new()
                });
                by_file.get_mut(path).unwrap().push(content);
            }
            None => passthrough.push(line),
        }
    }

    if order.is_empty() {
        // Nothing looked like a match line at all -- return the input
        // unchanged rather than silently dropping content grep produced in
        // a shape this parser doesn't recognize.
        return text.to_string();
    }

    let mut out: Vec<String> = Vec::new();
    for path in &order {
        let matches = &by_file[path];
        out.push(format!("{path} ({} match{})", matches.len(), if matches.len() == 1 { "" } else { "es" }));
        for m in matches.iter().take(MAX_MATCHES_PER_FILE) {
            out.push(format!("  {m}"));
        }
        if matches.len() > MAX_MATCHES_PER_FILE {
            out.push(format!("  … {} more", matches.len() - MAX_MATCHES_PER_FILE));
        }
    }
    for line in passthrough {
        out.push(line.to_string());
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cargo_test_output_collapses_passes_and_keeps_a_failure_with_its_context() {
        let input = "\
running 3 tests
test foo::a ... ok
test foo::b ... FAILED
thread 'foo::b' panicked at 'assertion failed', src/foo.rs:10:5
test foo::c ... ok

failures:

test result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out";

        let out = compress_test_output(input, CompressKind::TestRust);
        assert!(out.contains("… 1 passing/skipped test line(s) collapsed …"), "{out}");
        assert!(out.contains("test foo::b ... FAILED"), "{out}");
        assert!(out.contains("panicked at 'assertion failed'"), "failure context must survive: {out}");
        assert!(out.contains("test result: FAILED. 2 passed; 1 failed"), "the summary line must always survive: {out}");
        // The second passing test (`foo::c`, after the failure) must still
        // be collapsed into its own count, not silently dropped -- the
        // marker for it surfaces at the next fail/summary boundary (here,
        // right before the summary line), since unclassified lines in
        // between (the blank line, "failures:") don't themselves flush it.
        assert_eq!(out.matches("passing/skipped test line(s) collapsed").count(), 2, "both the pre-failure and post-failure passing tests must each get their own collapse marker: {out}");
    }

    #[test]
    fn cargo_test_output_with_no_failures_collapses_everything_but_the_summary() {
        let input = "\
test a ... ok
test b ... ok
test c ... ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out";

        let out = compress_test_output(input, CompressKind::TestRust);
        assert!(out.contains("… 3 passing/skipped test line(s) collapsed …"), "{out}");
        assert!(!out.contains("test a ... ok"), "individual passing lines must not survive: {out}");
        assert!(out.contains("test result: ok. 3 passed"), "{out}");
    }

    #[test]
    fn pytest_verbose_output_flags_failed_and_keeps_the_summary() {
        let input = "\
tests/test_a.py::test_one PASSED
tests/test_b.py::test_two FAILED
E   assert 1 == 2
========================= 1 failed, 1 passed in 0.02s =========================";

        let out = compress_test_output(input, CompressKind::TestPytest);
        assert!(out.contains("… 1 passing/skipped test line(s) collapsed …"), "{out}");
        assert!(out.contains("tests/test_b.py::test_two FAILED"), "{out}");
        assert!(out.contains("assert 1 == 2"), "{out}");
        assert!(out.contains("1 failed, 1 passed in 0.02s"), "{out}");
    }

    #[test]
    fn go_test_output_flags_fail_and_keeps_the_package_summary() {
        let input = "\
--- PASS: TestFoo (0.00s)
--- FAIL: TestBar (0.00s)
    bar_test.go:12: expected 2, got 3
FAIL
FAIL\tmypackage\t0.004s";

        let out = compress_test_output(input, CompressKind::TestGo);
        assert!(out.contains("--- FAIL: TestBar"), "{out}");
        assert!(out.contains("expected 2, got 3"), "{out}");
        assert!(out.contains("FAIL\tmypackage\t0.004s"), "{out}");
    }

    #[test]
    fn grep_output_is_grouped_by_file_with_a_count() {
        let input = "\
src/a.rs:10:fn foo() {}
src/a.rs:20:fn bar() {}
src/b.rs:5:fn foo() {}";

        let out = compress_grep(input);
        assert!(out.contains("src/a.rs (2 matches)"), "{out}");
        assert!(out.contains("src/b.rs (1 match)"), "{out}");
        assert!(out.contains("fn foo() {}"));
    }

    #[test]
    fn grep_output_caps_matches_per_file() {
        let many = (0..15).map(|i| format!("src/a.rs:{i}:line {i}")).collect::<Vec<_>>().join("\n");
        let out = compress_grep(&many);
        assert!(out.contains("src/a.rs (15 matches)"), "{out}");
        assert!(out.contains("… 5 more"), "{out}");
    }

    #[test]
    fn grep_output_with_no_recognizable_matches_passes_through_unchanged() {
        let input = "grep: some/path: No such file or directory";
        assert_eq!(compress_grep(input), input);
    }
}

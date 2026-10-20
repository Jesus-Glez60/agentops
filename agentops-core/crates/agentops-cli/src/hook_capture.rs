//! `agentops hook-capture --event <name>` — the handler behind every Claude
//! Code hook `connect` installs (see `maybe_install_claude_hooks` in
//! `main.rs`). One binary, dispatched by `--event` rather than by sniffing
//! payload shape.
//!
//! The store-touching logic for each event lives in `agentops_mcp::hooks`,
//! which documents what each event does. This module only decides *where*
//! it runs:
//!
//! - **Local** (no `.context/agentops-remote.json`): open the repo's store
//!   and call `agentops_mcp::hooks::handle` directly. Errors propagate as
//!   before, except on `pretooluse`, which must never fail.
//! - **Remote** (marker present): forward the payload to the server's
//!   `POST /repos/{id}/hooks/{event}` with the marker's API key, which runs
//!   the same function against the server's store. The local graph is never
//!   opened. A network error, timeout or non-2xx response never fails the
//!   hook: it logs one stderr line and exits 0 with no context, so a session
//!   never stalls on recall.
//!
//! Client-only concerns stay here: reading stdin, `--compress` command
//! rewriting on `pretooluse`, and `sessionend`'s detached usage sync.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use agentops_mcp::hooks;
use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::Value;

use crate::remote_marker::{read_remote_marker, RemoteMcpMarker};

/// Per-HTTP-call timeout when forwarding a hook to the server. A prompt
/// hook must never feel slow; Claude Code's own hook timeout (10 s for the
/// prompt and subagent hooks `connect` installs) is the outer bound.
pub(crate) const REMOTE_HOOK_TIMEOUT: Duration = Duration::from_secs(3);

/// Entry point for the `hook-capture` subcommand. `compress` is only ever
/// read by the `pretooluse` branch (see `Connect`'s `--compress-commands`
/// doc comment in `main.rs` for why this is opt-in) — every other event
/// ignores it.
pub fn run(event: &str, compress: bool) -> Result<()> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).context("reading hook payload from stdin")?;
    if let Some(output) = process(event, compress, &input)? {
        println!("{output}");
    }
    Ok(())
}

/// Everything `run` does except the stdin read and the stdout write, so
/// tests can drive it with fixture payloads.
fn process(event: &str, compress: bool, input: &str) -> Result<Option<Value>> {
    match event {
        // Never fails the hook: a usage sync is bookkeeping.
        "sessionend" => {
            if let Err(e) = process_session_end(input) {
                eprintln!("agentops hook-capture --event sessionend: {e:#} (non-fatal)");
            }
            Ok(None)
        }
        // `PreToolUse` is the one event whose nonzero exit can actually
        // block the tool call it's reacting to (confirmed against Claude
        // Code's real hook docs). This handler must never surface an `Err`.
        "pretooluse" => match process_pre_tool_use(input, compress) {
            Ok(output) => Ok(output),
            Err(e) => {
                eprintln!("agentops hook-capture --event pretooluse: {e:#} (non-fatal, continuing)");
                Ok(None)
            }
        },
        event if hooks::STORE_EVENTS.contains(&event) => {
            let payload: Value = serde_json::from_str(input).with_context(|| format!("parsing {event} payload"))?;
            dispatch(event, &payload)
        }
        other => anyhow::bail!("unknown hook event '{other}'"),
    }
}

/// Runs `event`'s store logic locally or on the server (see the module doc
/// comment). Returns the hook's stdout JSON, if any.
fn dispatch(event: &str, payload: &Value) -> Result<Option<Value>> {
    let Some(cwd) = payload.get("cwd").and_then(|v| v.as_str()) else {
        return Ok(None);
    };
    let root = hook_repo_root(Path::new(cwd), std::env::var("CLAUDE_PROJECT_DIR").ok().as_deref());
    let repo_path = root.as_path();
    let mut payload = payload.clone();
    // Worked out here because the server never sees the repo root.
    if let Some(rel) = touched_rel_path(&payload, Path::new(cwd), repo_path) {
        payload[hooks::REL_PATH_FIELD] = Value::String(rel);
    }
    if !hooks::is_relevant(event, &payload) {
        return Ok(None);
    }
    // The `path` the agent should pass to agentops tools, echoed into the
    // context the hook injects: the connection id for a remote repo (the
    // hosted `/mcp` never resolves a local checkout path), else this repo.
    let marker = read_remote_marker(repo_path);
    payload["agentops_path"] = Value::String(marker.as_ref().map_or_else(|| repo_path.to_string_lossy().into_owned(), |m| m.connection_id.clone()));
    let payload = &payload;
    match marker {
        Some(marker) => match call_remote(&marker, event, payload) {
            Ok(output) => Ok(output),
            Err(e) => {
                eprintln!("agentops hook-capture --event {event}: {e:#} (remote, non-fatal)");
                Ok(None)
            }
        },
        None => {
            let store = agentops_mcp::open_store(repo_path)?;
            let repo = agentops_mcp::repo_name(repo_path);
            hooks::handle(store.as_ref(), &repo, event, payload)
        }
    }
}

/// The repo-relative path of the file a tool call touches: its
/// `file_path`/`notebook_path`, or the one file a Bash command only reads
/// (`work_recall::bash_read_path`). A relative path is taken from the
/// shell's `cwd`. `None` when there is no such file or it's outside `root`.
pub(crate) fn touched_rel_path(payload: &Value, cwd: &Path, root: &Path) -> Option<String> {
    let input = payload.get("tool_input")?;
    let raw = match input.get("file_path").or_else(|| input.get("notebook_path")).and_then(Value::as_str) {
        Some(path) => path.to_string(),
        None => agentops_retrieval::work_recall::bash_read_path(input.get("command").and_then(Value::as_str)?)?,
    };
    let path = cwd.join(raw);
    // macOS reaches the same directory as /tmp and /private/tmp, so compare
    // canonical forms when they exist (a deleted file has none).
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let path = std::fs::canonicalize(&path).unwrap_or(path);
    let rel = path.strip_prefix(&root).ok()?.to_string_lossy().into_owned();
    (!rel.is_empty()).then_some(rel)
}

/// The repo a hook belongs to. The payload's `cwd` follows the session's
/// shell, so after a `cd` into a subdirectory it would miss the remote
/// marker and open a stray local graph there (found 2026-10-16: five of
/// them, holding the evidence the plan gate was looking for on the
/// server). Claude Code sets `CLAUDE_PROJECT_DIR` to the root the session
/// started in; without it, the nearest ancestor with a `.git`.
fn hook_repo_root(cwd: &Path, project_dir: Option<&str>) -> PathBuf {
    if let Some(dir) = project_dir.filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    cwd.ancestors().find(|p| p.join(".git").exists()).unwrap_or(cwd).to_path_buf()
}

/// `POST {server_url}/repos/{connection_id}/hooks/{event}`, the same Bearer
/// auth `usage::push_usage_remote` uses. The payload goes up without its
/// local paths (`cwd`, `transcript_path`): the server resolves the repo
/// from the connection and never reads them.
pub(crate) fn call_remote(marker: &RemoteMcpMarker, event: &str, payload: &Value) -> Result<Option<Value>> {
    let url = format!("{}/repos/{}/hooks/{event}", marker.server_url.trim_end_matches('/'), marker.connection_id);
    let mut body = payload.clone();
    if let Some(map) = body.as_object_mut() {
        map.remove("cwd");
        map.remove("transcript_path");
    }
    let mut response = ureq::post(&url)
        .header("Authorization", &format!("Bearer {}", marker.api_key))
        .config()
        .http_status_as_error(false)
        .timeout_global(Some(REMOTE_HOOK_TIMEOUT))
        .build()
        .send_json(&body)
        .with_context(|| format!("calling POST {url}"))?;
    if !response.status().is_success() {
        anyhow::bail!("POST {url} returned {}", response.status());
    }
    let reply: Value = response.body_mut().read_json().with_context(|| format!("parsing POST {url} response"))?;
    Ok(reply.get("output").cloned().filter(|v| !v.is_null()))
}

// ---------------------------------------------------------------------
// PreToolUse
// ---------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct PreToolUsePayload {
    tool_name: String,
    #[serde(default)]
    tool_input: Value,
}

/// Two fully independent concerns, either/both/neither of which can fire on
/// the same call: the secret-exposure warning (store-side, via `dispatch`,
/// see `agentops_mcp::hooks`) and, when `compress` is set, a
/// command-output-compression rewrite (RTK-inspired -- see
/// `classify_for_compression`). The compression path sets `updatedInput`
/// only, never `permissionDecision`, so it changes *what* runs, never
/// *whether* it runs. A failure to reach the store never drops the rewrite.
fn process_pre_tool_use(input: &str, compress: bool) -> Result<Option<Value>> {
    let value: Value = serde_json::from_str(input).context("parsing PreToolUse payload")?;
    let payload: PreToolUsePayload = serde_json::from_value(value.clone()).context("parsing PreToolUse payload")?;
    let rewrite = if compress { detect_command_rewrite(&payload.tool_name, &payload.tool_input) } else { None };
    let warning = match dispatch("pretooluse", &value) {
        Ok(output) => output,
        Err(e) => {
            eprintln!("agentops hook-capture --event pretooluse: {e:#} (non-fatal, continuing)");
            None
        }
    };
    if warning.is_none() && rewrite.is_none() {
        return Ok(None);
    }
    let mut output = warning.unwrap_or_else(|| hooks::hook_output("PreToolUse", None, None));
    if let Some(new_command) = &rewrite {
        output["hookSpecificOutput"]["updatedInput"] = serde_json::json!({ "command": new_command });
    }
    Ok(Some(output))
}

// ---------------------------------------------------------------------
// PreToolUse: RTK-inspired command-output compression (opt-in via
// `--compress`, see `Connect`'s `--compress-commands` doc comment)
// ---------------------------------------------------------------------

/// How a matched command gets rewritten -- see each variant for why the two
/// strategies exist rather than one.
enum CompressAction {
    /// Append a flag directly to the command -- no pipe, so there's nothing
    /// to restore via `PIPESTATUS`: the original command still runs and
    /// still reports its own real exit code. Used for `git log --oneline`/
    /// `git diff -U1`, where git already produces the compact form itself
    /// given the right flag -- the smallest possible implementation for
    /// those two cases, no `compress-output` invocation needed at all.
    AppendFlag(&'static str),
    /// Pipe the command's combined stdout+stderr through
    /// `agentops compress-output --kind <kind>`.
    Pipe(crate::compress_output::CompressKind),
}

/// Only matches a **single, unchained** recognized command -- anything
/// already containing `&&`, `;`, `|`, or a subshell `(` is left completely
/// untouched, since rewriting only the visible head of a chained command
/// risks corrupting the rest of the pipeline (a script that pipes
/// `cargo test` into its own `tee`, for instance, must never be
/// double-piped by this hook). Deliberately a small, conservative allowlist
/// -- matches `is_git_op`'s own "plain substring check, not regex"
/// precedent -- meant to grow over time, not to be exhaustive from day one.
fn classify_for_compression(command: &str) -> Option<CompressAction> {
    use crate::compress_output::CompressKind;

    let trimmed = command.trim();
    if trimmed.contains("&&") || trimmed.contains(';') || trimmed.contains('|') || trimmed.contains('(') {
        return None;
    }
    if trimmed.starts_with("cargo test") {
        return Some(CompressAction::Pipe(CompressKind::TestRust));
    }
    if trimmed.starts_with("npm test") || trimmed.starts_with("npm run test") {
        return Some(CompressAction::Pipe(CompressKind::TestNode));
    }
    if trimmed.starts_with("pytest") || trimmed.starts_with("python -m pytest") || trimmed.starts_with("python3 -m pytest") {
        return Some(CompressAction::Pipe(CompressKind::TestPytest));
    }
    if trimmed.starts_with("go test") {
        return Some(CompressAction::Pipe(CompressKind::TestGo));
    }
    if trimmed.starts_with("git log") && !trimmed.contains("--oneline") && !trimmed.contains("--format") && !trimmed.contains("--stat") {
        return Some(CompressAction::AppendFlag("--oneline"));
    }
    if trimmed.starts_with("git diff") && !trimmed.contains("-U") && !trimmed.contains("--unified") {
        return Some(CompressAction::AppendFlag("-U1"));
    }
    if trimmed.starts_with("grep ") || trimmed.starts_with("rg ") {
        return Some(CompressAction::Pipe(CompressKind::Grep));
    }
    None
}

/// `AppendFlag` needs no exit-code handling (see the variant's own doc
/// comment). `Pipe` loses the piped command's exit status by default (a
/// bare pipe reports the *last* command's exit code, i.e.
/// `compress-output`'s own, not the real command's) -- `2>&1` folds stderr
/// into the piped stream first (test frameworks often write failures
/// there), and bash's `PIPESTATUS` restores the original command's real
/// exit status as the shell's final status afterward, which is what Claude
/// Code's Bash tool inspects for success/failure. This assumes Claude
/// Code's Bash tool invokes `bash -c`, not `sh -c`/dash (which lacks
/// `PIPESTATUS`) -- verify this mechanism in a real shell (and against a
/// real Claude Code session) before relying on it; see this crate's own
/// test coverage for the shell-level check.
fn rewrite_command(original: &str, action: &CompressAction) -> String {
    match action {
        CompressAction::AppendFlag(flag) => format!("{original} {flag}"),
        CompressAction::Pipe(kind) => format!("{{ {original}; }} 2>&1 | agentops compress-output --kind {} ; exit ${{PIPESTATUS[0]}}", kind.as_flag()),
    }
}

fn detect_command_rewrite(tool_name: &str, tool_input: &serde_json::Value) -> Option<String> {
    if tool_name != "Bash" {
        return None;
    }
    let command = tool_input.get("command").and_then(|v| v.as_str())?;
    let action = classify_for_compression(command)?;
    Some(rewrite_command(command, &action))
}

// ---------------------------------------------------------------------
// SessionEnd
// ---------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct SessionEndPayload {
    transcript_path: Option<String>,
    cwd: Option<String>,
    reason: Option<String>,
}

/// Syncs the ending session's usage. Claude Code cancels SessionEnd hooks
/// after 1.5s by default (confirmed against its hooks docs), too short for
/// a remote push -- so this spawns a detached `agentops usage sync
/// --transcript` and returns at once. Its output is appended to
/// `.context/usage-sync.log` so a failed push leaves a trace (gotcha
/// 54237: usage pushes have failed silently before).
fn process_session_end(input: &str) -> Result<()> {
    let payload: SessionEndPayload = serde_json::from_str(input).context("parsing SessionEnd payload")?;
    let transcript = payload.transcript_path.context("SessionEnd payload has no transcript_path")?;
    let cwd = hook_repo_root(Path::new(payload.cwd.as_deref().unwrap_or(".")), std::env::var("CLAUDE_PROJECT_DIR").ok().as_deref());
    let context_dir = cwd.join(".context");
    std::fs::create_dir_all(&context_dir)?;
    let mut log = std::fs::OpenOptions::new().create(true).append(true).open(context_dir.join("usage-sync.log"))?;
    // `reason` = clear/resume/logout/prompt_input_exit/other; a `clear` is
    // a manual reset, which the reset lever's measurement needs to tell apart.
    writeln!(log, "session end (reason: {}): {transcript}", payload.reason.as_deref().unwrap_or("unknown"))?;
    std::process::Command::new(std::env::current_exe()?)
        .args(["usage", "sync", "--transcript", &transcript, "--path"])
        .arg(&cwd)
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .spawn()
        .context("spawning detached usage sync")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn payload(dir: &Path, extra: Value) -> String {
        let mut p = json!({ "session_id": "sess-1", "cwd": dir.to_string_lossy() });
        p.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        p.to_string()
    }

    fn write_marker(dir: &Path, server_url: &str) {
        std::fs::create_dir_all(dir.join(".context")).unwrap();
        std::fs::write(crate::remote_marker::remote_marker_path(dir), json!({ "server_url": server_url, "connection_id": "conn-1", "api_key": "k" }).to_string()).unwrap();
    }

    #[test]
    fn a_local_repo_records_through_the_shared_hooks_module() {
        let dir = tempfile::tempdir().unwrap();
        process("userpromptsubmit", false, &payload(dir.path(), json!({ "prompt": "fix the login bug" }))).unwrap();
        let store = agentops_mcp::open_store(dir.path()).unwrap();
        let events = store.session_events(&agentops_mcp::repo_name(dir.path()), "sess-1").unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_kind, "user_intent");
    }

    #[test]
    fn a_hook_from_a_subdirectory_belongs_to_the_session_root_not_the_shells_cwd() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("crates").join("cli");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        assert_eq!(hook_repo_root(&sub, None), dir.path(), "nearest ancestor with .git");
        assert_eq!(hook_repo_root(&sub, Some("/elsewhere/root")), Path::new("/elsewhere/root"), "CLAUDE_PROJECT_DIR wins");
        assert_eq!(hook_repo_root(&sub, Some("")), dir.path());

        // End to end: a prompt from the subdirectory lands in the root's store
        // (skipped when the tests themselves run under a Claude Code hook env).
        if std::env::var_os("CLAUDE_PROJECT_DIR").is_some() {
            return;
        }
        process("userpromptsubmit", false, &payload(&sub, json!({ "prompt": "fix the login bug" }))).unwrap();
        assert!(!sub.join(".context").exists(), "no stray graph in the subdirectory");
        let store = agentops_mcp::open_store(dir.path()).unwrap();
        assert_eq!(store.session_events(&agentops_mcp::repo_name(dir.path()), "sess-1").unwrap().len(), 1);
    }

    #[test]
    fn the_touched_files_repo_relative_path_is_worked_out_on_the_client() {
        let repo = tempfile::tempdir().unwrap();
        let root = repo.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/a.rs"), "").unwrap();
        let rel = |input: Value, cwd: &Path| touched_rel_path(&json!({ "tool_input": input }), cwd, root);
        let abs = root.join("src/a.rs").to_string_lossy().into_owned();
        assert_eq!(rel(json!({"file_path": abs}), root).as_deref(), Some("src/a.rs"));
        assert_eq!(rel(json!({"command": "sed -n 1,9p a.rs"}), &root.join("src")).as_deref(), Some("src/a.rs"), "relative to the shell's cwd");
        assert_eq!(rel(json!({"file_path": "/etc/hosts"}), root), None, "outside the repo");
        assert_eq!(rel(json!({"command": "cargo test"}), root), None, "not a read");
    }

    #[test]
    fn a_local_read_is_recorded_by_its_repo_relative_path() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn a() {}").unwrap();
        let path = dir.path().join("a.rs").to_string_lossy().into_owned();
        process("posttooluse", false, &payload(dir.path(), json!({ "tool_name": "Read", "tool_input": {"file_path": path}, "tool_response": "fn a() {}" }))).unwrap();
        let store = agentops_mcp::open_store(dir.path()).unwrap();
        let events = store.session_events(&agentops_mcp::repo_name(dir.path()), "sess-1").unwrap();
        assert_eq!(events.iter().map(|e| (e.event_kind.as_str(), e.description.as_str())).collect::<Vec<_>>(), [("file_read", "a.rs")]);
    }

    #[test]
    fn an_irrelevant_tool_call_never_opens_the_store() {
        let dir = tempfile::tempdir().unwrap();
        process("posttooluse", false, &payload(dir.path(), json!({ "tool_name": "Bash", "tool_input": {"command": "ls"}, "tool_response": "ok" }))).unwrap();
        assert!(!dir.path().join(".context").join("graph.db").exists());
    }

    /// The spec's "unreachable server" check, for both failure shapes: a
    /// refused connection, and a server that accepts but never answers
    /// (which must hit `REMOTE_HOOK_TIMEOUT`, not hang).
    #[test]
    fn a_remote_repo_with_an_unreachable_server_exits_cleanly_in_time_and_never_creates_a_local_graph() {
        let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let silent_url = format!("http://{}", silent.local_addr().unwrap());
        let refused_url = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            format!("http://{}", l.local_addr().unwrap())
        };
        for url in [refused_url, silent_url] {
            let dir = tempfile::tempdir().unwrap();
            write_marker(dir.path(), &url);
            let events: [(&str, Value); 5] = [
                ("userpromptsubmit", json!({ "prompt": "hi" })),
                ("sessionstart", json!({ "source": "startup" })),
                ("posttooluse", json!({ "tool_name": "Edit", "tool_input": {"file_path": "a.rs"}, "tool_response": "ok" })),
                ("posttoolusefailure", json!({ "tool_name": "Bash", "error": "boom" })),
                ("pretooluse", json!({ "tool_name": "Read", "tool_input": {"file_path": ".env"} })),
            ];
            for (event, extra) in events {
                let started = std::time::Instant::now();
                let output = process(event, false, &payload(dir.path(), extra)).unwrap();
                assert!(output.is_none(), "{event} against {url} must emit no context");
                assert!(started.elapsed() < REMOTE_HOOK_TIMEOUT + Duration::from_secs(1), "{event} against {url} took {:?}", started.elapsed());
            }
            assert!(!dir.path().join(".context").join("graph.db").exists(), "a remote-connected repo must never open the local graph");
        }
    }

    #[test]
    fn a_remote_repo_forwards_the_payload_without_local_paths_and_returns_the_servers_output() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = vec![0u8; 65536];
            let mut received = String::new();
            // Read until the JSON body's closing brace has arrived.
            while !received.contains("\r\n\r\n") || !received.trim_end().ends_with('}') {
                let n = std::io::Read::read(&mut stream, &mut buf).unwrap();
                if n == 0 {
                    break;
                }
                received.push_str(&String::from_utf8_lossy(&buf[..n]));
            }
            let body = json!({ "output": { "hookSpecificOutput": { "hookEventName": "UserPromptSubmit", "additionalContext": "from the server" } } }).to_string();
            write!(stream, "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len()).unwrap();
            received
        });
        let dir = tempfile::tempdir().unwrap();
        write_marker(dir.path(), &url);
        let output = process("userpromptsubmit", false, &payload(dir.path(), json!({ "prompt": "hi", "transcript_path": "/home/me/t.jsonl" }))).unwrap().unwrap();
        assert_eq!(output["hookSpecificOutput"]["additionalContext"], "from the server");
        let request = server.join().unwrap();
        assert!(request.starts_with("POST /repos/conn-1/hooks/userpromptsubmit "), "{request}");
        assert!(request.contains("Bearer k"), "{request}");
        assert!(!request.contains("transcript_path") && !request.contains("\"cwd\""), "local paths must not be sent: {request}");
        let body: Value = serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(body["agentops_path"], "conn-1", "the agent is told the connection id");
        assert!(!dir.path().join(".context").join("graph.db").exists());
    }

    #[test]
    fn pre_tool_use_never_fails_and_keeps_the_rewrite_when_the_server_is_unreachable() {
        let dir = tempfile::tempdir().unwrap();
        let refused_url = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            format!("http://{}", l.local_addr().unwrap())
        };
        write_marker(dir.path(), &refused_url);
        let output = process("pretooluse", true, &payload(dir.path(), json!({ "tool_name": "Bash", "tool_input": {"command": "cargo test"} }))).unwrap().unwrap();
        assert!(output["hookSpecificOutput"]["updatedInput"]["command"].as_str().unwrap().contains("compress-output"));
    }

    #[test]
    fn classify_for_compression_recognizes_the_supported_commands() {
        assert!(matches!(classify_for_compression("cargo test -p agentops-graph"), Some(CompressAction::Pipe(crate::compress_output::CompressKind::TestRust))));
        assert!(matches!(classify_for_compression("npm test"), Some(CompressAction::Pipe(crate::compress_output::CompressKind::TestNode))));
        assert!(matches!(classify_for_compression("pytest -v"), Some(CompressAction::Pipe(crate::compress_output::CompressKind::TestPytest))));
        assert!(matches!(classify_for_compression("go test ./..."), Some(CompressAction::Pipe(crate::compress_output::CompressKind::TestGo))));
        assert!(matches!(classify_for_compression("grep -rn foo src/"), Some(CompressAction::Pipe(crate::compress_output::CompressKind::Grep))));
        assert!(matches!(classify_for_compression("rg foo"), Some(CompressAction::Pipe(crate::compress_output::CompressKind::Grep))));
        assert!(matches!(classify_for_compression("git log"), Some(CompressAction::AppendFlag("--oneline"))));
        assert!(matches!(classify_for_compression("git diff"), Some(CompressAction::AppendFlag("-U1"))));
    }

    #[test]
    fn classify_for_compression_declines_a_command_already_carrying_the_relevant_flag() {
        assert!(classify_for_compression("git log --oneline").is_none());
        assert!(classify_for_compression("git log --format=%H").is_none());
        assert!(classify_for_compression("git diff -U5").is_none());
        assert!(classify_for_compression("git diff --unified=3").is_none());
    }

    #[test]
    fn classify_for_compression_declines_a_chained_or_unrecognized_command() {
        assert!(classify_for_compression("cargo test && echo done").is_none(), "must not rewrite only the head of a chained command");
        assert!(classify_for_compression("cargo test; echo done").is_none());
        assert!(classify_for_compression("cargo test | tee out.log").is_none(), "must not double-pipe an already-piped command");
        assert!(classify_for_compression("(cd foo && cargo test)").is_none());
        assert!(classify_for_compression("echo hello").is_none());
        assert!(classify_for_compression("git status").is_none(), "git status is not a supported command");
    }

    #[test]
    fn rewrite_command_pipes_through_compress_output_and_restores_the_real_exit_code() {
        let rewritten = rewrite_command("cargo test", &CompressAction::Pipe(crate::compress_output::CompressKind::TestRust));
        assert_eq!(rewritten, "{ cargo test; } 2>&1 | agentops compress-output --kind test-rust ; exit ${PIPESTATUS[0]}");
    }

    #[test]
    fn rewrite_command_appends_a_flag_with_no_pipe_for_git_log_and_diff() {
        assert_eq!(rewrite_command("git log", &CompressAction::AppendFlag("--oneline")), "git log --oneline");
        assert_eq!(rewrite_command("git diff", &CompressAction::AppendFlag("-U1")), "git diff -U1");
    }

    #[test]
    fn detect_command_rewrite_only_applies_to_bash_tool_calls() {
        let tool_input = serde_json::json!({ "command": "cargo test" });
        assert!(detect_command_rewrite("Bash", &tool_input).is_some());
        assert!(detect_command_rewrite("Read", &tool_input).is_none(), "a non-Bash tool must never be rewritten");
    }

    #[test]
    fn pre_tool_use_rewrites_a_test_command_only_when_compress_is_enabled() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_string_lossy().to_string();
        let payload = serde_json::json!({ "tool_name": "Bash", "tool_input": {"command": "cargo test"}, "session_id": "sess-12", "cwd": path }).to_string();

        // Without --compress: silent, no rewrite (and no credential in this
        // command, so no warning either).
        process_pre_tool_use(&payload, false).unwrap();
    }

    /// Confirms the actual shell mechanism `rewrite_command`'s `Pipe`
    /// variant depends on: a bare pipe reports the *piped-to* command's
    /// exit code, but `PIPESTATUS[0]` recovers the original command's real
    /// one, and the trailing `exit ${PIPESTATUS[0]}` makes that the whole
    /// rewritten line's final status -- verified directly in a real shell,
    /// not just asserted about `rewrite_command`'s string output above.
    #[test]
    fn pipestatus_recovers_the_original_commands_real_exit_code_through_a_pipe() {
        let rewritten = rewrite_command("false", &CompressAction::Pipe(crate::compress_output::CompressKind::TestRust));
        // Swap the real binary invocation for `cat` so this test doesn't
        // depend on `agentops` being built/on PATH -- the exit-code
        // mechanism under test is bash's, not this crate's own.
        let rewritten = rewritten.replace("agentops compress-output --kind test-rust", "cat");
        let status = std::process::Command::new("bash").arg("-c").arg(&rewritten).status().unwrap();
        assert_eq!(status.code(), Some(1), "the rewritten line's exit code must be `false`'s real code (1), not `cat`'s (0): {rewritten}");
    }

}

//! `.context/agentops-remote.json`, the marker `agentops connect --remote`
//! writes. Shared by `main.rs` (which writes and re-reads it) and
//! `hook_capture` (which routes every hook to the server when it exists).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// `.context/agentops-remote.json` -- persists a repo's "coding tools here
/// point at this remote server" choice across future `agentops install`/
/// `connect` runs that don't pass `--remote` again (e.g. re-scanning, or
/// adding another agent later). Lives in `.context/` alongside
/// `graph.db` -- a per-repo, not-meant-for-git-history directory this
/// codebase already uses for exactly this kind of local machine state,
/// deliberately not `.ruler/` (whose own files are Ruler-managed/
/// overwritten-on-every-`apply`, the opposite of what a marker needs).
#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct RemoteMcpMarker {
    pub(crate) server_url: String,
    pub(crate) connection_id: String,
    // Embedded literally into 3 of 4 tools' MCP config (see
    // `json_remote_entry`) so re-running `agentops install`/`connect` here
    // later can re-embed the same key without re-prompting or re-doing
    // device-login. Written with `0600` perms below — this file carries a
    // real credential, unlike the rest of `.context/`.
    pub(crate) api_key: String,
}

pub(crate) fn remote_marker_path(path: &Path) -> PathBuf {
    path.join(".context").join("agentops-remote.json")
}

pub(crate) fn read_remote_marker(path: &Path) -> Option<RemoteMcpMarker> {
    let content = std::fs::read_to_string(remote_marker_path(path)).ok()?;
    serde_json::from_str(&content).ok()
}

/// `0600` on Unix, matching the precedent for the only other secret-bearing
/// file this codebase writes (`agentops-repo-access`'s `write_private_key_file`)
/// — a no-op permission-wise on Windows, where there's no equivalent bit.
#[cfg(unix)]
pub(crate) fn write_remote_marker_file(marker_path: &Path, contents: &str) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(marker_path).with_context(|| format!("writing {}", marker_path.display()))?;
    std::io::Write::write_all(&mut file, contents.as_bytes()).with_context(|| format!("writing {}", marker_path.display()))
}

#[cfg(not(unix))]
pub(crate) fn write_remote_marker_file(marker_path: &Path, contents: &str) -> Result<()> {
    std::fs::write(marker_path, contents).with_context(|| format!("writing {}", marker_path.display()))
}

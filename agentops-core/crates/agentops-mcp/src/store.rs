//! Backend-selection factory — the single place every driving adapter
//! (`agentops-mcp`'s own use cases, `agentops-cli`, `agentops-api`) goes
//! through to open a `GraphStore`, instead of each constructing
//! `SqliteGraphStore::open(...)` directly. Adding `PostgresGraphStore`
//! (Module D) as a second backend meant this had to exist somewhere —
//! otherwise "which backend" would be a decision duplicated at every call
//! site, the exact drift class this rebuild keeps avoiding elsewhere.

use std::path::Path;

use agentops_graph::GraphStore;
use anyhow::Result;

use crate::scan::graph_db_path;

/// Opens the configured `GraphStore` backend for `repo_path`.
/// `AGENTOPS_DATABASE_URL`, if set, selects `PostgresGraphStore` — one
/// shared database across every repo, distinguished entirely via the
/// `repo` column, not separate connections/files. Falls back to
/// `SqliteGraphStore` at `.context/graph.db` otherwise — the zero-setup
/// default, unchanged from before this factory existed.
///
/// The returned store's calls block their calling thread during I/O when
/// backed by Postgres (see `PostgresGraphStore`'s doc comment) — an async
/// caller (`agentops-api`'s handlers) must wrap calls in
/// `tokio::task::spawn_blocking` to avoid stalling its executor, and must
/// never call this from inside an already-running Tokio runtime directly.
pub fn open_store(repo_path: &Path) -> Result<Box<dyn GraphStore>> {
    match std::env::var("AGENTOPS_DATABASE_URL") {
        Ok(url) => Ok(Box::new(agentops_graph_pg::PostgresGraphStore::connect(&url)?)),
        Err(_) => Ok(Box::new(agentops_graph::SqliteGraphStore::open(&graph_db_path(repo_path))?)),
    }
}

/// Human-readable description of which backend `open_store` would select
/// for `repo_path` right now — for CLI output, so it doesn't have to
/// re-derive the same `AGENTOPS_DATABASE_URL` decision itself (and risk
/// printing the SQLite path even when Postgres is actually in use).
pub fn describe_backend(repo_path: &Path) -> String {
    match std::env::var("AGENTOPS_DATABASE_URL") {
        // Never print a raw connection string — it may carry a plaintext
        // password. Only the host/database portion is useful for a human
        // reading CLI output anyway.
        Ok(url) => format!("Postgres ({})", redact_credentials(&url)),
        Err(_) => format!("SQLite ({})", graph_db_path(repo_path).display()),
    }
}

fn redact_credentials(url: &str) -> String {
    match url.split_once("://") {
        Some((scheme, rest)) => match rest.split_once('@') {
            Some((_userinfo, host_and_db)) => format!("{scheme}://***@{host_and_db}"),
            None => url.to_string(),
        },
        None => url.to_string(),
    }
}

#[cfg(test)]
mod redact_tests {
    use super::redact_credentials;

    #[test]
    fn strips_userinfo_from_a_connection_string() {
        assert_eq!(redact_credentials("postgres://user:hunter2@localhost:5433/db"), "postgres://***@localhost:5433/db");
    }

    #[test]
    fn leaves_a_url_with_no_userinfo_unchanged() {
        assert_eq!(redact_credentials("postgres://localhost:5433/db"), "postgres://localhost:5433/db");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_sqlite_when_no_database_url_is_set() {
        // SAFETY: test-only, no other test in this process sets this var
        // concurrently with an expectation that conflicts with removing it.
        unsafe { std::env::remove_var("AGENTOPS_DATABASE_URL") };
        let dir = tempfile::tempdir().unwrap();
        let store = open_store(dir.path()).unwrap();
        // Exercises the trait object end-to-end — a fresh SQLite store has
        // zero nodes for a repo that's never been scanned.
        assert_eq!(store.all_nodes(&crate::scan::repo_name(dir.path())).unwrap().len(), 0);
        assert!(graph_db_path(dir.path()).exists());
    }
}

//! Backend-selection factory — the single place every driving adapter
//! (`agentops-mcp`'s own use cases, `agentops-cli`, `agentops-api`) goes
//! through to open a `GraphStore`, instead of each constructing
//! `SqliteGraphStore::open(...)` directly. Adding `PostgresGraphStore`
//! (Module D) as a second backend meant this had to exist somewhere —
//! otherwise "which backend" would be a decision duplicated at every call
//! site, the exact drift class this rebuild keeps avoiding elsewhere.

use std::cell::RefCell;
use std::path::Path;

use agentops_graph::GraphStore;
use anyhow::Result;

use crate::scan::graph_db_path;

thread_local! {
    // Scoped override for `open_store`, set only for the duration of one
    // `with_shared_postgres_store` call (see below) -- lets every existing
    // `tools.rs` handler (and anything else that calls `open_store`
    // transitively, e.g. `scan::persist`) transparently reuse a
    // process-lifetime shared pool instead of connecting fresh, with zero
    // changes to any handler's own code. Deliberately a thread-local, not a
    // parameter threaded through `call_tool`'s `Handler = fn(&Value) ->
    // ...` type -- that's a bare function pointer (no closures, no extra
    // captured state), so adding a parameter would mean touching every one
    // of `tools.rs`'s ~20 handlers individually. `RefCell`, not `Cell`,
    // since `PostgresGraphStore` isn't `Copy`.
    static SHARED_PG_STORE: RefCell<Option<agentops_graph_pg::PostgresGraphStore>> = const { RefCell::new(None) };
}

/// Runs `f` with `store` installed as `open_store`'s override for this
/// thread only, for `f`'s duration. Fixes a real production incident: the
/// tenant-scoped `/mcp` HTTP endpoint (`agentops-heavy-api::mcp_http`)
/// dispatches through `agentops_mcp::call_tool`, the same generic tool
/// dispatcher `agentops-cli` and the stdio MCP server use -- and every
/// individual `tools.rs` handler calls `open_store` directly (correct for
/// the CLI/stdio-server case, where there's no shared-pool concept at all).
/// Without this, `/mcp` never benefited from the shared-Postgres-pool fix
/// the dashboard routes got (`agentops-api`/`agentops-heavy-api`'s
/// `AppState.pg_store` + `resolve_store`) -- confirmed live: 54 concurrent
/// `/mcp` tool calls produced real Postgres deadlocks (`AccessExclusiveLock`
/// conflicts from many concurrent connections each replaying the full
/// schema DDL via `PostgresGraphStore::connect()`), the exact thundering-
/// herd shape the original dashboard incident had, just on a different
/// endpoint. The caller (`mcp_http.rs`) wraps its `call_tool` invocation in
/// this function, passing `state.pg_store`.
///
/// A `Drop` guard clears the override even if `f` panics -- required, not
/// just tidy: `tokio::task::spawn_blocking`'s pool reuses OS threads across
/// unrelated calls, so a panic that skipped clearing this could leak one
/// tenant's shared store into a later, unrelated call on the same pooled
/// thread.
pub fn with_shared_postgres_store<T>(store: Option<&agentops_graph_pg::PostgresGraphStore>, f: impl FnOnce() -> T) -> T {
    SHARED_PG_STORE.with(|cell| *cell.borrow_mut() = store.cloned());
    struct ClearOnDrop;
    impl Drop for ClearOnDrop {
        fn drop(&mut self) {
            SHARED_PG_STORE.with(|cell| *cell.borrow_mut() = None);
        }
    }
    let _guard = ClearOnDrop;
    f()
}

/// Opens the configured `GraphStore` backend for `repo_path`.
/// `AGENTOPS_DATABASE_URL`, if set, selects `PostgresGraphStore` — one
/// shared database across every repo, distinguished entirely via the
/// `repo` column, not separate connections/files. Falls back to
/// `SqliteGraphStore` at `.context/graph.db` otherwise — the zero-setup
/// default, unchanged from before this factory existed.
///
/// Checks `with_shared_postgres_store`'s thread-local override first --
/// when set (only true inside a `call_tool` invocation `mcp_http.rs`
/// wrapped), reuses that shared, cheaply-`Clone`d store instead of
/// connecting fresh. Transparent to every caller: this function's contract
/// (open the right backend for `repo_path`) is unchanged, callers just get
/// a cheaper connection when a shared one happens to be scoped.
///
/// The returned store's calls block their calling thread during I/O when
/// backed by Postgres (see `PostgresGraphStore`'s doc comment) — an async
/// caller (`agentops-api`'s handlers) must wrap calls in
/// `tokio::task::spawn_blocking` to avoid stalling its executor, and must
/// never call this from inside an already-running Tokio runtime directly.
pub fn open_store(repo_path: &Path) -> Result<Box<dyn GraphStore>> {
    if let Some(shared) = SHARED_PG_STORE.with(|cell| cell.borrow().clone()) {
        return Ok(Box::new(shared));
    }
    match std::env::var("AGENTOPS_DATABASE_URL") {
        Ok(url) => Ok(Box::new(agentops_graph_pg::PostgresGraphStore::connect(&url)?)),
        Err(_) => Ok(Box::new(agentops_graph::SqliteGraphStore::open(&graph_db_path(repo_path))?)),
    }
}

/// Connects once, for callers that hold the result for the process's
/// lifetime (`agentops-heavy-api`'s `AppState`, `agentops-api`'s server
/// mode) rather than calling `open_store` per-request the way one-shot CLI
/// invocations do. `None` when `AGENTOPS_DATABASE_URL` isn't set --
/// SQLite-backed deployments' callers fall back to `open_store`'s existing
/// per-repo-path behavior unchanged via `resolve_store` below. Fixes a real
/// production incident: `open_store` used to be called fresh on every HTTP
/// request, and when Postgres-backed that meant a brand-new connection pool
/// per request -- 54 concurrent requests once meant 54 simultaneous pool
/// creations, and 32 of them failed under that thundering herd.
pub fn open_shared_postgres_store() -> Result<Option<agentops_graph_pg::PostgresGraphStore>> {
    match std::env::var("AGENTOPS_DATABASE_URL") {
        Ok(url) => Ok(Some(agentops_graph_pg::PostgresGraphStore::connect(&url)?)),
        Err(_) => Ok(None),
    }
}

/// Resolves the store a handler should use: the pre-shared Postgres store
/// if one was supplied (cloned -- cheap, see `PostgresGraphStore`'s own doc
/// comment: only bumps an `Arc<Runtime>` and the already-`Arc`-backed
/// `deadpool::Pool`, no real connection/runtime work), otherwise falls back
/// to `open_store`'s existing per-call behavior (SQLite, or a fresh
/// Postgres connect if the shared store wasn't threaded in for some
/// caller).
pub fn resolve_store(shared: Option<&agentops_graph_pg::PostgresGraphStore>, repo_path: &Path) -> Result<Box<dyn GraphStore>> {
    match shared {
        Some(store) => Ok(Box::new(store.clone())),
        None => open_store(repo_path),
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

/// Serializes any test in this crate that reads/mutates
/// `AGENTOPS_DATABASE_URL` -- it's process-global, and cargo runs a
/// crate's tests in parallel by default, so one test setting it (e.g.
/// `scan.rs`'s Postgres-path rescan test) could otherwise race a
/// concurrently-running test that assumes it's unset (this file's own
/// `defaults_to_sqlite_when_no_database_url_is_set`).
#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::Mutex;
    pub(crate) static ENV_LOCK: Mutex<()> = Mutex::new(());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_sqlite_when_no_database_url_is_set() {
        let _guard = test_support::ENV_LOCK.lock().unwrap();
        // SAFETY: guarded by ENV_LOCK above, so no other test in this
        // crate's binary can be reading/setting this var concurrently.
        unsafe { std::env::remove_var("AGENTOPS_DATABASE_URL") };
        let dir = tempfile::tempdir().unwrap();
        let store = open_store(dir.path()).unwrap();
        // Exercises the trait object end-to-end — a fresh SQLite store has
        // zero nodes for a repo that's never been scanned.
        assert_eq!(store.all_nodes(&crate::scan::repo_name(dir.path())).unwrap().len(), 0);
        assert!(graph_db_path(dir.path()).exists());
    }

    #[test]
    fn open_shared_postgres_store_returns_none_when_no_database_url_is_set() {
        let _guard = test_support::ENV_LOCK.lock().unwrap();
        // SAFETY: guarded by ENV_LOCK above.
        unsafe { std::env::remove_var("AGENTOPS_DATABASE_URL") };
        assert!(open_shared_postgres_store().unwrap().is_none());
    }

    #[test]
    fn resolve_store_falls_back_to_open_store_when_nothing_is_shared() {
        let _guard = test_support::ENV_LOCK.lock().unwrap();
        // SAFETY: guarded by ENV_LOCK above.
        unsafe { std::env::remove_var("AGENTOPS_DATABASE_URL") };
        let dir = tempfile::tempdir().unwrap();
        // No shared store supplied -- must behave exactly like `open_store`
        // (SQLite here, since AGENTOPS_DATABASE_URL is unset).
        let store = resolve_store(None, dir.path()).unwrap();
        assert_eq!(store.all_nodes(&crate::scan::repo_name(dir.path())).unwrap().len(), 0);
        assert!(graph_db_path(dir.path()).exists());
    }

    /// Live against a real local Postgres, matching `agentops-graph-pg`'s
    /// own established discipline; skips (not fails) when nothing is
    /// reachable. Doesn't touch `AGENTOPS_DATABASE_URL` (the shared store is
    /// passed explicitly), so unlike other tests in this crate that
    /// exercise the Postgres path, this one needs no `ENV_LOCK`/`#[ignore]`.
    #[test]
    fn resolve_store_uses_the_shared_store_when_supplied_instead_of_connecting_fresh() {
        let url = std::env::var("AGENTOPS_TEST_DATABASE_URL").unwrap_or_else(|_| "postgres://postgres:test@localhost:5433/agentops_test".to_string());
        let Ok(shared) = agentops_graph_pg::PostgresGraphStore::connect(&url) else {
            eprintln!("skipping resolve_store_uses_the_shared_store_when_supplied_instead_of_connecting_fresh: no Postgres reachable at {url}");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let repo = crate::scan::repo_name(dir.path());

        let via_resolve = resolve_store(Some(&shared), dir.path()).unwrap();
        let id = via_resolve.add_node(agentops_graph::NewNode { kind: agentops_graph::NodeKind::File, repo: repo.clone(), path: Some("a.rs".into()), name: None, container: None, start_line: None, end_line: None, content: None }).unwrap();

        // Written through the store `resolve_store` returned; visible
        // directly through the original shared instance -- proves it's the
        // same pool, not an independent fresh connect.
        assert!(shared.get_node(&repo, id).unwrap().is_some());
        shared.delete_nodes(&repo, &[id]).unwrap();
    }
}

//! Backend-selection factory — the single place every driving adapter
//! (`agentops-mcp`, `agentops-heavy-mcp`, `agentops-cli`, `agentops-api`)
//! goes through to open a `GraphStore`, instead of each constructing
//! `SqliteGraphStore::open(...)` directly. Extracted out of `agentops-mcp`
//! so `agentops-heavy-mcp` can depend on it directly without a
//! wrong-direction dependency on `agentops-mcp` itself — `agentops-mcp`
//! re-exports everything here under `agentops_mcp::store`/`agentops_mcp::
//! scan::{graph_db_path, repo_name}` unchanged, so no existing caller needs
//! to change its import path.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use agentops_graph::GraphStore;
use anyhow::Result;

/// `.context/graph.db` under a repo's working directory — the SQLite
/// backend's on-disk location.
pub fn graph_db_path(repo_path: &Path) -> PathBuf {
    repo_path.join(".context").join("graph.db")
}

/// The repo's identity within the graph store — canonicalized directory
/// name, falling back to the raw path string if canonicalization fails
/// (e.g. the path doesn't exist yet in a test).
pub fn repo_name(path: &Path) -> String {
    path.canonicalize().ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_else(|| path.display().to_string())
}

thread_local! {
    // Scoped override for `open_store`, set only for the duration of one
    // `with_shared_postgres_store` call (see below) -- lets every existing
    // handler (and anything else that calls `open_store` transitively)
    // transparently reuse a process-lifetime shared pool instead of
    // connecting fresh, with zero changes to any handler's own code.
    // Deliberately a thread-local, not a parameter threaded through
    // `call_tool`'s handler type -- see `agentops-mcp`'s original doc
    // comment for why (a bare function pointer, no closures/extra captured
    // state). `RefCell`, not `Cell`, since `PostgresGraphStore` isn't `Copy`.
    static SHARED_PG_STORE: RefCell<Option<agentops_graph_pg::PostgresGraphStore>> = const { RefCell::new(None) };
}

/// Runs `f` with `store` installed as `open_store`'s override for this
/// thread only, for `f`'s duration. Fixes a real production incident: the
/// tenant-scoped `/mcp` HTTP endpoint (`agentops-heavy-api::mcp_http`)
/// dispatches through `agentops_mcp::call_tool`, the same generic tool
/// dispatcher `agentops-cli` and the stdio MCP server use -- and every
/// individual handler calls `open_store` directly (correct for the
/// CLI/stdio-server case, where there's no shared-pool concept at all).
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
///
/// Retries with exponential backoff + jitter until Postgres is reachable or
/// `retry_budget()` elapses -- fixes a second real production incident: on
/// a host reboot (e.g. after a power outage), this function used to call
/// `PostgresGraphStore::connect` exactly once, and if Postgres hadn't
/// finished its own startup yet the `.expect(...)`/`?` at every caller
/// panicked/errored immediately. PM2 (the process manager wrapping
/// `agentops-server` in production) then burned through its entire
/// restart budget in milliseconds -- zero delay between restarts -- far
/// faster than Postgres's own recovery (confirmed live: 3.4 seconds,
/// including WAL replay, from container start to "ready to accept
/// connections") had any chance to finish, and gave up permanently with no
/// auto-recovery. `PostgresGraphStore::connect` itself is unchanged -- this
/// loop just calls it repeatedly until it succeeds or the budget runs out.
///
/// Deliberately **not** applied to `open_store` (the per-call CLI path
/// just below) or any direct `PostgresGraphStore::connect` caller (e.g.
/// `agentops-cli`'s one-shot `migrate-graph` command) -- a human running a
/// quick command or an explicit one-time migration while watching the
/// terminal should get an immediate, clear error if Postgres isn't up,
/// not a silent minute-long hang. Only a caller starting a long-running
/// server actually wants "wait, don't crash" semantics.
pub fn open_shared_postgres_store() -> Result<Option<agentops_graph_pg::PostgresGraphStore>> {
    let Ok(url) = std::env::var("AGENTOPS_DATABASE_URL") else { return Ok(None) };
    retry_with_backoff(retry_budget(), || agentops_graph_pg::PostgresGraphStore::connect(&url)).map(Some)
}

/// The actual retry-with-backoff loop, generic over the attempt so it's
/// unit-testable without a real Postgres: `PostgresGraphStore::connect`
/// does a full wire-protocol handshake, not just a TCP accept, so a bare
/// `TcpListener` fixture can't stand in for "Postgres becomes reachable
/// mid-retry" the way it could for a simpler TCP-only dependency. This is
/// the only abstraction added for that reason -- `open_shared_postgres_store`
/// itself keeps the exact same public signature and behavior either way.
fn retry_with_backoff<T>(max_wait: std::time::Duration, mut attempt: impl FnMut() -> Result<T>) -> Result<T> {
    let start = std::time::Instant::now();
    let mut delay = std::time::Duration::from_millis(500);
    loop {
        match attempt() {
            Ok(value) => return Ok(value),
            Err(e) if start.elapsed() >= max_wait => {
                return Err(e.context(format!("giving up connecting to Postgres after retrying for {:?}", start.elapsed())));
            }
            Err(e) => {
                eprintln!("waiting for Postgres to become ready ({:?} elapsed, retrying in {:?}): {e:#}", start.elapsed(), delay);
                std::thread::sleep(jittered(delay));
                delay = (delay * 2).min(std::time::Duration::from_secs(5));
            }
        }
    }
}

/// How long `open_shared_postgres_store` retries before giving up, read
/// from `AGENTOPS_PG_CONNECT_RETRY_SECS`. Default `60` -- chosen with a
/// real, live-measured Postgres cold-start recovery of 3.4 seconds as its
/// baseline (roughly a 17x margin), not an arbitrary round number; kept
/// tunable via env var rather than hardcoded because that 3.4s figure is
/// one data point from one incident (a clean shutdown/restart), not a
/// guarantee about every future recovery -- a deployment that ever needs
/// a real WAL-corruption replay could genuinely take longer. A
/// non-positive or unparseable value means `Duration::ZERO`: try once, no
/// retry at all -- explicit, documented behavior, not an infinite-loop-
/// with-zero-sleep footgun.
fn retry_budget() -> std::time::Duration {
    match std::env::var("AGENTOPS_PG_CONNECT_RETRY_SECS") {
        // Unset entirely -- the documented default.
        Err(_) => std::time::Duration::from_secs(60),
        // Set, but non-positive or unparseable -- explicit "try once", not
        // the default (the previous `.filter(...).unwrap_or_else(default)`
        // shape here collapsed this case into "unset", silently ignoring an
        // explicit `=0`).
        Ok(s) => s.parse::<i64>().ok().filter(|&secs| secs > 0).map(|secs| std::time::Duration::from_secs(secs as u64)).unwrap_or(std::time::Duration::ZERO),
    }
}

/// Multiplies `delay` by a random factor in `0.5..=1.0` -- without this,
/// every process restarting after the same outage would retry in
/// lockstep on each backoff boundary, trading "all crash at once" for
/// "all hammer Postgres at once" a few hundred milliseconds later. Reuses
/// `getrandom` (already a workspace dependency, already this codebase's
/// pattern for exactly this kind of lightweight randomness need -- see
/// `agentops_repo_access::store::new_local_repo_id`) rather than adding a
/// new dependency like `rand` just for one random byte.
fn jittered(delay: std::time::Duration) -> std::time::Duration {
    let mut byte = [0u8; 1];
    if getrandom::fill(&mut byte).is_err() {
        return delay;
    }
    let factor = 0.5 + (byte[0] as f64 / 255.0) * 0.5;
    delay.mul_f64(factor)
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

/// Serializes any test (in this crate, or a downstream crate like
/// `agentops-mcp` whose own tests also read/mutate `AGENTOPS_DATABASE_URL`
/// via `open_store`) that touches this process-global env var -- cargo
/// runs a crate's tests in parallel by default, so one test setting it
/// could otherwise race a concurrently-running test that assumes it's
/// unset. `pub`, not `pub(crate)`: downstream crates need this same lock,
/// not just this crate's own tests.
pub mod test_support {
    use std::sync::Mutex;
    pub static ENV_LOCK: Mutex<()> = Mutex::new(());
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
        assert_eq!(store.all_nodes(&repo_name(dir.path())).unwrap().len(), 0);
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
        assert_eq!(store.all_nodes(&repo_name(dir.path())).unwrap().len(), 0);
        assert!(graph_db_path(dir.path()).exists());
    }

    #[test]
    fn retry_with_backoff_gives_up_with_a_clear_error_once_the_budget_elapses() {
        let mut attempts = 0u32;
        let result = retry_with_backoff(std::time::Duration::from_millis(50), || {
            attempts += 1;
            Err::<(), anyhow::Error>(anyhow::anyhow!("not ready yet"))
        });
        let err = result.unwrap_err();
        assert!(err.to_string().contains("giving up connecting to Postgres after retrying for"));
        // At least the first attempt plus one retry after the 500ms->jittered
        // sleep -- actually just one attempt here, since the first attempt
        // already exceeds the tiny 50ms budget before any sleep happens.
        assert!(attempts >= 1);
    }

    #[test]
    fn retry_with_backoff_succeeds_once_the_dependency_becomes_available() {
        let mut attempts = 0u32;
        let result = retry_with_backoff(std::time::Duration::from_secs(5), || {
            attempts += 1;
            if attempts < 3 { Err::<&str, anyhow::Error>(anyhow::anyhow!("still not ready")) } else { Ok("connected") }
        });
        assert_eq!(result.unwrap(), "connected");
        assert_eq!(attempts, 3);
    }

    #[test]
    fn retry_budget_env_var_zero_or_unset_means_try_once() {
        let _guard = test_support::ENV_LOCK.lock().unwrap();
        // SAFETY: guarded by ENV_LOCK above.
        unsafe { std::env::set_var("AGENTOPS_PG_CONNECT_RETRY_SECS", "0") };
        assert_eq!(retry_budget(), std::time::Duration::ZERO);

        // A single attempt against a zero budget must not retry at all: the
        // very first failure already has start.elapsed() >= Duration::ZERO.
        let mut attempts = 0u32;
        let result = retry_with_backoff(retry_budget(), || {
            attempts += 1;
            Err::<(), anyhow::Error>(anyhow::anyhow!("nope"))
        });
        assert!(result.is_err());
        assert_eq!(attempts, 1);

        unsafe { std::env::remove_var("AGENTOPS_PG_CONNECT_RETRY_SECS") };
        assert_eq!(retry_budget(), std::time::Duration::from_secs(60));
    }

    #[test]
    fn jittered_stays_within_half_to_full_of_the_input_delay() {
        let delay = std::time::Duration::from_secs(4);
        for _ in 0..50 {
            let j = jittered(delay);
            assert!(j >= delay.mul_f64(0.5), "{j:?} fell below the 0.5x floor of {delay:?}");
            assert!(j <= delay, "{j:?} exceeded the unjittered {delay:?}");
        }
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
        let repo = repo_name(dir.path());

        let via_resolve = resolve_store(Some(&shared), dir.path()).unwrap();
        let id = via_resolve.add_node(agentops_graph::NewNode { kind: agentops_graph::NodeKind::File, repo: repo.clone(), path: Some("a.rs".into()), name: None, container: None, start_line: None, end_line: None, content: None }).unwrap();

        // Written through the store `resolve_store` returned; visible
        // directly through the original shared instance -- proves it's the
        // same pool, not an independent fresh connect.
        assert!(shared.get_node(&repo, id).unwrap().is_some());
        shared.delete_nodes(&repo, &[id]).unwrap();
    }
}

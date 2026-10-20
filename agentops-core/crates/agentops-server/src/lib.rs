//! Merged REST server: composes `agentops-api`, `docbrain-api`, and
//! `agentops-heavy-api`'s routers into one Axum process on one port,
//! following the Meilisearch/Directus "one open-core, no price gate" model.
//! Each service keeps its own typed state and auth posture — see each
//! crate's `build_router`/`build_router_without_health`/`build_full_router`
//! doc comments — this crate only composes them and binds one socket.
//!
//! `/health` is served exactly once, from `agentops-heavy-api`'s
//! `build_full_router` (the only one of the three still carrying its own
//! `/health` route) — `agentops-api`/`docbrain-api` are mounted via their
//! `_without_health` variant instead, since `Router::merge` panics on a
//! duplicate route and all three otherwise register the same path.
//!
//! `/tools`/`/tools/{name}` similarly collide across all three (each
//! dispatches its own tool table at the same two paths). `agentops-api`'s
//! stays at the top level (unchanged for existing callers). `docbrain-api`'s
//! is nested under `/docbrain` instead — its tool-calling frontend
//! consumers (the Libraries screen's "Add library" flow) need it reachable,
//! not just excluded, so namespacing rather than "one wins" is required
//! here, not merely a nicety. `agentops-heavy-api`'s stays excluded from
//! this merged process (`include_tools: false`) since nothing calls it over
//! REST yet — it's already fully reachable via the merged stdio
//! `agentops-mcp-server` binary, which is where a real unified `/tools`
//! dispatcher covering all three tool tables belongs long-term (tracked as
//! a follow-up, not resolved here).
//!
//! `/activity`, `/local-search`, `/gotchas`, and the `/repos/{id}/nodes/*`
//! `/repos/{id}/graph` `/repos/{id}/docs` routes also collide: `agentops-api`
//! registers them backed by `agentops-manifest`'s local scan registry
//! (single-operator, this process's own filesystem), `agentops-heavy-api`
//! registers the identical paths backed by `ConnectionStore` (tenant-scoped,
//! hosted). This merged process needs the tenant-scoped versions — the web
//! dashboard is a multi-tenant hosted deployment, not a local CLI install —
//! so `agentops-api` is mounted via `build_router_without_dashboard_routes`
//! rather than `build_router_without_health`.

use std::path::PathBuf;

use agentops_mcp::AccessMode;
use axum::Router;
use docbrain_graph::{DocbrainStore, SqliteDocbrainStore};

mod connect_sh;

/// Reads every env var this merged server needs, builds the composed
/// `Router`, binds `AGENTOPS_ADDR` (default `127.0.0.1:8420`), and serves
/// until the process is killed. The one entry point both this crate's own
/// binary and `agentops-cli`'s `serve-api` subcommand call.
pub async fn run() -> anyhow::Result<()> {
    let addr = std::env::var("AGENTOPS_ADDR").unwrap_or_else(|_| "127.0.0.1:8420".to_string());

    let mode = match std::env::var("AGENTOPS_ACCESS_MODE").as_deref() {
        Ok("full") => AccessMode::Full,
        _ => AccessMode::Advisor,
    };
    let api_key_hash = std::env::var("AGENTOPS_API_KEY_HASH").ok();
    // Checked before anything binds or connects, so a misconfigured deploy
    // exits with this message instead of briefly serving open routes.
    let allow_unauthenticated = matches!(std::env::var("AGENTOPS_ALLOW_UNAUTHENTICATED").as_deref(), Ok("1") | Ok("true"));
    startup_auth_check(&addr, api_key_hash.as_deref().filter(|h| !h.trim().is_empty()), allow_unauthenticated)?;
    let manifest_path = std::env::var("AGENTOPS_MANIFEST_PATH").map(PathBuf::from).unwrap_or_else(|_| agentops_manifest::default_manifest_path());
    // `_without_dashboard_routes`, not just `_without_health` — this
    // process also mounts `agentops-heavy-api`'s tenant-scoped equivalents
    // of `/activity`, `/local-search`, `/gotchas`, and the `/repos/{id}/...`
    // node/graph/docs routes at the identical paths; see
    // `agentops_api::build_router_without_dashboard_routes`'s doc comment.
    //
    // `spawn_blocking`-wrapped, not called directly: `build_router_without_dashboard_routes`
    // connects to Postgres internally when `AGENTOPS_DATABASE_URL` is set,
    // and `PostgresGraphStore::connect()` spins up and enters its own
    // dedicated `Runtime`, which panics ("Cannot start a runtime from
    // within a runtime") if attempted directly from this already-running
    // `#[tokio::main]` runtime's thread. Confirmed live: this exact panic
    // took the whole merged server down on deploy the first time this line
    // called the sync builder unwrapped.
    let api_key_hash_for_agentops = api_key_hash.clone();
    let agentops_router = tokio::task::spawn_blocking(move || agentops_api::build_router_without_dashboard_routes(mode, api_key_hash_for_agentops, manifest_path)).await?;

    let docbrain_db_path = docbrain_mcp::default_db_path();
    let docbrain_store = SqliteDocbrainStore::open(&docbrain_db_path)?;
    // Registers agentops itself as a known docbrain library on every boot,
    // so `docbrain list_libraries`/`get_library slug=agentops` work with no
    // manual registration step -- `add_library` is an upsert-by-slug (see
    // `docbrain-graph`'s `add_library_is_idempotent_by_slug` test), so this
    // is safe to call unconditionally rather than needing an existence
    // check first. This only registers metadata; it deliberately does NOT
    // trigger a `scrape_library` (that's a separate, explicit step a user
    // or their agent runs once — same onboarding as any other library —
    // rather than unconditional network egress on every server start).
    docbrain_store.add_library(
        "agentops",
        "AgentOps",
        Some("Scans your repos into a knowledge graph, layers hybrid semantic search and curated gotchas/decisions on top, and exposes all of it to AI coding agents over MCP."),
        Some("https://github.com/Jesus-Glez60/agentops"),
        Some("https://github.com/Jesus-Glez60/agentops#readme"),
    )?;
    // Nested under /docbrain (not merged at the top level) since its
    // /tools/{name} would otherwise collide with agentops-api's — see this
    // module's doc comment.
    let docbrain_router = docbrain_api::build_router_without_health(docbrain_store, docbrain_db_path, api_key_hash);

    let heavy_db_path = std::env::var("AGENTOPS_HEAVY_API_DB").map(PathBuf::from).unwrap_or_else(|_| agentops_data_dir().join("heavy-api.sqlite"));
    let heavy_router = agentops_heavy_api::build_full_router(&heavy_db_path, false).await?;
    // Prompt hooks recall by meaning here; load the model before the
    // first one arrives (ADR 0052).
    agentops_heavy_api::warm_embedder();

    let app = Router::new().merge(agentops_router).nest("/docbrain", docbrain_router).merge(heavy_router).merge(connect_sh::router());

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    println!("agentops-server listening on {addr}");
    // Connect info gives handlers the TCP peer, which decides whether an
    // `X-Real-IP` header may be trusted (`agentops-heavy-api`'s `client_ip`).
    axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>()).await?;
    Ok(())
}

/// Refuses to serve without `AGENTOPS_API_KEY_HASH` on anything but a
/// loopback address. Every route that falls back to the instance key
/// (`agentops_security::api_key::check_bearer_api_key`) treats a missing
/// hash as "no auth at all", so on a network-reachable bind that would let
/// anonymous callers name any tenant on the REST routes and call `/tools`
/// with literal server paths (engineering docs finding F37, live in
/// production until 2026-10-13). A solo loopback setup (`agentops
/// serve-api`, default `127.0.0.1:8420`) keeps working without a key.
/// An address that doesn't resolve counts as non-loopback (fail closed).
fn startup_auth_check(addr: &str, api_key_hash: Option<&str>, allow_unauthenticated: bool) -> anyhow::Result<()> {
    if api_key_hash.is_some() || allow_unauthenticated {
        return Ok(());
    }
    use std::net::ToSocketAddrs;
    let loopback_only = addr.to_socket_addrs().map(|mut addrs| addrs.all(|a| a.ip().is_loopback())).unwrap_or(false);
    if loopback_only {
        return Ok(());
    }
    anyhow::bail!(
        "refusing to start: AGENTOPS_ADDR={addr} is not a loopback address and AGENTOPS_API_KEY_HASH is unset, \
         which would leave the REST API unauthenticated. Generate a key with `agentops api-key generate` and set \
         AGENTOPS_API_KEY_HASH to its hash, or set AGENTOPS_ALLOW_UNAUTHENTICATED=1 if this really is a trusted network."
    )
}

#[cfg(test)]
mod tests {
    use super::startup_auth_check;

    #[test]
    fn a_non_loopback_bind_without_a_key_is_refused() {
        assert!(startup_auth_check("0.0.0.0:8420", None, false).is_err());
        assert!(startup_auth_check("[::]:8420", None, false).is_err());
    }

    #[test]
    fn loopback_binds_need_no_key() {
        assert!(startup_auth_check("127.0.0.1:8420", None, false).is_ok());
        assert!(startup_auth_check("[::1]:8420", None, false).is_ok());
    }

    #[test]
    fn a_key_or_the_explicit_opt_out_allows_any_bind() {
        assert!(startup_auth_check("0.0.0.0:8420", Some("abc123"), false).is_ok());
        assert!(startup_auth_check("0.0.0.0:8420", None, true).is_ok());
    }

    #[test]
    fn an_unresolvable_address_fails_closed() {
        assert!(startup_auth_check("not a socket address", None, false).is_err());
    }
}

/// `AGENTOPS_DATA_DIR` (default `~/.agentops`) — see `agentops-manifest`'s
/// identical helper doc comment; duplicated rather than shared, matching
/// this codebase's established small-helper-duplication precedent (e.g.
/// every MCP crate's own hand-rolled JSON-RPC wire types).
fn agentops_data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("AGENTOPS_DATA_DIR") {
        return PathBuf::from(dir);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".agentops")
}

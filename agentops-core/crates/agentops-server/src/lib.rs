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

use std::path::PathBuf;

use agentops_mcp::AccessMode;
use axum::Router;
use docbrain_graph::{DocbrainStore, SqliteDocbrainStore};

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
    let manifest_path = std::env::var("AGENTOPS_MANIFEST_PATH").map(PathBuf::from).unwrap_or_else(|_| agentops_manifest::default_manifest_path());
    let agentops_router = agentops_api::build_router_without_health(mode, api_key_hash.clone(), manifest_path);

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

    let app = Router::new().merge(agentops_router).nest("/docbrain", docbrain_router).merge(heavy_router);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    println!("agentops-server listening on {addr}");
    axum::serve(listener, app).await?;
    Ok(())
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

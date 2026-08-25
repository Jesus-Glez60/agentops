//! REST API for hosted repo access: drives `agentops-repo-access`'s SSH
//! deploy-key flow and exposes the `agentops-github-app` install URL,
//! behind the same API-key auth pattern as `agentops-api`/`docbrain-api`.
//!
//! **The encrypted private key blob is never returned by any endpoint** —
//! every response DTO here is built by hand from `RepoConnection` rather
//! than serializing it directly, specifically so a future field added to
//! `RepoConnection` doesn't silently leak into an HTTP response the way it
//! would if this server just re-serialized the store's row type.
//!
//! Ported from `main`, adapted in two places for the current codebase:
//! (1) `agentops_embeddings` is `agentops_heavy_embeddings` now (this
//! rebuild's rename to avoid colliding with the new open-core
//! `agentops-embeddings` crate); (2) `docbrain-graph` is single-tenant this
//! rebuild (no `TenantContext`), so multi-tenant
//! isolation for docbrain content is achieved by routing to **one SQLite
//! file per tenant** (`docbrain_db_dir/<org>.db`) rather than a
//! trait-level tenant parameter — the same "isolate by connection string,
//! not by type" approach `agentops-graph-pg` already uses for repos.
//! **Known, flagged limitation of that approach**: Qdrant itself has no
//! tenant concept — it filters by `repo`/`kind` only — so while each
//! tenant's docbrain *source* content is stored separately, the shared
//! vector index doesn't enforce tenant isolation on its own (same
//! trust-at-index-time posture `main` already had). Real per-tenant vector
//! isolation is future work, not solved here.

use std::sync::{Arc, Mutex};

use axum::extract::{Path as AxumPath, Query, Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::Mutex as AsyncMutex;
use tower_http::cors::CorsLayer;

use agentops_accounts::{AccountStore, User};
use agentops_heavy_embeddings::SemanticIndex;
use agentops_repo_access::secrets::SecretsProvider;
use agentops_repo_access::store::{ConnectionStatus, ConnectionStore, RepoConnection};
use agentops_teams::TeamStore;

mod accounts_integrations;
mod github_app_routes;
mod indexing;
mod linear_webhook;
mod team;
pub use accounts_integrations::build_accounts_integrations_router;
pub use linear_webhook::{AutoKickoffTeamConfig, SeenDeliveries};
pub use team::build_team_router;

#[derive(Clone)]
pub struct AppState {
    store: Arc<Mutex<ConnectionStore>>,
    secrets: Arc<dyn SecretsProvider + Send + Sync>,
    /// GitHub App URL slug, if a GitHub App has actually been registered
    /// for this deployment — `GET /repos/github-app/install-url` 404s with
    /// a clear message when this is `None` rather than returning a bogus URL.
    github_app_slug: Option<String>,
    api_key_hash: Option<String>,
    /// `None` when Qdrant isn't configured — `/search*` routes return `503`
    /// with a clear message rather than the server refusing to start at all
    /// over an optional feature.
    search_index: Option<Arc<AsyncMutex<SemanticIndex>>>,
    /// Directory holding one docbrain SQLite file per tenant
    /// (`<org>.db`, or `default.db` when no `org` is given) — see the
    /// module doc comment for why this replaces `main`'s `TenantContext`.
    docbrain_db_dir: std::path::PathBuf,
    /// `Some` once a deployment has accounts/teams wired up (session auth
    /// available for `/repos/*` alongside the existing API-key path);
    /// `None` keeps this router's original self-hosted-single-operator
    /// behavior byte-for-byte (API-key-only, client-supplied `tenant`) —
    /// see `require_api_key_or_session`'s doc comment.
    accounts: Option<Arc<Mutex<AccountStore>>>,
    teams: Option<Arc<Mutex<TeamStore>>>,
    /// Live progress tracking for the clone -> scan -> embed -> docgen
    /// pipeline -- see `indexing.rs`'s module doc comment for why this is a
    /// separate table from both the `tasks` project-task tracker and
    /// `scan_history`.
    indexing: Arc<Mutex<agentops_repo_access::indexing_store::IndexingStore>>,
    /// Root directory each indexing job clones a connection's remote into,
    /// one subdirectory per `<tenant>/<connection_id>` -- see
    /// `indexing::checkout_path`'s doc comment for why the directory name
    /// is derived from `connection_id` rather than anything canonicalized
    /// from a not-yet-existing path.
    repo_checkouts_dir: std::path::PathBuf,
    /// `Some` once a real GitHub App is registered for this deployment
    /// (App id, private key, webhook secret all present) -- the
    /// installation-listing/connect-from-installation/webhook routes 404
    /// or reject with a clear message when this is `None`, same posture as
    /// `github_app_slug` for the install-url endpoint (a deployment without
    /// these env vars still boots normally, GitHub App just isn't usable).
    github_app_config: Option<github_app_routes::GitHubAppConfig>,
}

pub fn build_router(
    store: ConnectionStore,
    secrets: Arc<dyn SecretsProvider + Send + Sync>,
    github_app_slug: Option<String>,
    api_key_hash: Option<String>,
    search_index: Option<Arc<AsyncMutex<SemanticIndex>>>,
    docbrain_db_dir: std::path::PathBuf,
    accounts: Option<AccountStore>,
    teams: Option<TeamStore>,
    indexing: agentops_repo_access::indexing_store::IndexingStore,
    repo_checkouts_dir: std::path::PathBuf,
    github_app_config: Option<github_app_routes::GitHubAppConfig>,
) -> Router {
    build_router_with_tools_flag(store, secrets, github_app_slug, api_key_hash, search_index, docbrain_db_dir, accounts, teams, indexing, repo_checkouts_dir, github_app_config, true)
}

/// Same as [`build_router`] minus `/tools`/`/tools/{name}` — `agentops-api`
/// registers the same two paths for its own tool table, so a process
/// composing both (e.g. `agentops-server`) can only mount one of them
/// as-is; the other needs a merged dispatcher covering both tool tables,
/// not yet built (tracked for the MCP-server-merge follow-up, same as
/// `docbrain-api`'s identical situation). Until then, the merged process
/// keeps `agentops-api`'s `/tools` and mounts this variant instead.
#[allow(clippy::too_many_arguments)]
pub fn build_router_without_tools(
    store: ConnectionStore,
    secrets: Arc<dyn SecretsProvider + Send + Sync>,
    github_app_slug: Option<String>,
    api_key_hash: Option<String>,
    search_index: Option<Arc<AsyncMutex<SemanticIndex>>>,
    docbrain_db_dir: std::path::PathBuf,
    accounts: Option<AccountStore>,
    teams: Option<TeamStore>,
    indexing: agentops_repo_access::indexing_store::IndexingStore,
    repo_checkouts_dir: std::path::PathBuf,
    github_app_config: Option<github_app_routes::GitHubAppConfig>,
) -> Router {
    build_router_with_tools_flag(store, secrets, github_app_slug, api_key_hash, search_index, docbrain_db_dir, accounts, teams, indexing, repo_checkouts_dir, github_app_config, false)
}

#[allow(clippy::too_many_arguments)]
fn build_router_with_tools_flag(
    store: ConnectionStore,
    secrets: Arc<dyn SecretsProvider + Send + Sync>,
    github_app_slug: Option<String>,
    api_key_hash: Option<String>,
    search_index: Option<Arc<AsyncMutex<SemanticIndex>>>,
    docbrain_db_dir: std::path::PathBuf,
    accounts: Option<AccountStore>,
    teams: Option<TeamStore>,
    indexing: agentops_repo_access::indexing_store::IndexingStore,
    repo_checkouts_dir: std::path::PathBuf,
    github_app_config: Option<github_app_routes::GitHubAppConfig>,
    include_tools: bool,
) -> Router {
    let state = AppState {
        store: Arc::new(Mutex::new(store)),
        secrets,
        github_app_slug,
        api_key_hash,
        search_index,
        docbrain_db_dir,
        accounts: accounts.map(|a| Arc::new(Mutex::new(a))),
        teams: teams.map(|t| Arc::new(Mutex::new(t))),
        indexing: Arc::new(Mutex::new(indexing)),
        repo_checkouts_dir,
        github_app_config,
    };
    let mut router = Router::new()
        .route("/repos/connect", post(connect_repo))
        .route("/repos", get(list_repos))
        .route("/repos/{id}/verify", post(verify_repo))
        .route("/repos/{id}/index", post(indexing::start_indexing))
        .route("/repos/{id}/index/status", get(indexing::indexing_status))
        .route("/repos/{id}/index/retry", post(indexing::retry_indexing))
        .route("/repos/{id}/regenerate-key", post(regenerate_key))
        .route("/repos/github-app/install-url", get(github_app_install_url))
        .route("/repos/github-app/callback", get(github_app_routes::github_app_callback))
        .route("/repos/github-app/installations/{id}/repos", get(github_app_routes::list_installation_repos_handler))
        .route("/repos/github-app/installations/{id}/connect", post(github_app_routes::connect_from_installation))
        .route("/search/index", post(search_index_handler))
        .route("/search", get(search_query_handler))
        .route("/docs/search/index", post(docs_search_index_handler))
        .route("/docs/search", get(docs_search_query_handler))
        .route("/consolidate/model", post(consolidate_model_handler));
    if include_tools {
        router = router.route("/tools", get(heavy_tools_list_handler)).route("/tools/{name}", post(heavy_tools_call_handler));
    }
    router
        .layer(middleware::from_fn_with_state(state.clone(), require_api_key_or_session))
        .with_state(state)
        .layer(CorsLayer::permissive())
}

/// Standalone `/health` — split out of [`build_router`] so a merged
/// multi-service process (e.g. `agentops-server`) can mount its own single
/// shared `/health` instead of one copy per merged service (`Router::merge`
/// panics on a duplicate route).
fn health_router() -> Router {
    Router::new().route("/health", get(health)).layer(CorsLayer::permissive())
}

/// Session-first, API-key-fallback: if `accounts` is configured and the
/// bearer token verifies as a live session, a `User` extension is
/// inserted and the request proceeds on the **session path** (handlers
/// below branch on `Option<Extension<User>>`'s presence to pick tenant
/// derivation and capability gating). Otherwise falls through to the
/// original API-key check unchanged -- byte-for-byte the same behavior
/// this router always had when `accounts` is `None` (the deployment truly
/// has no session concept, e.g. a bare `agentops-heavy-api` invocation not
/// wired to `run()`'s full stack) or when the bearer token simply isn't a
/// valid session token (a real API key, or garbage either way ends up
/// re-checked against `verify_api_key` exactly as before).
async fn require_api_key_or_session(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    if let Some(accounts) = &state.accounts {
        if let Some(token) = req.headers().get(axum::http::header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")) {
            let verified = { accounts.lock().unwrap().verify_session(token) };
            if let Ok(user) = verified {
                req.extensions_mut().insert(user);
                return next.run(req).await;
            }
        }
    }
    require_api_key(State(state), req, next).await
}

async fn require_api_key(State(state): State<AppState>, req: Request, next: Next) -> Response {
    match agentops_security::api_key::check_bearer_api_key(req.headers(), state.api_key_hash.as_deref()) {
        Ok(()) => next.run(req).await,
        Err((status, body)) => (status, body).into_response(),
    }
}

/// Session path: server-derived tenant from the verified `User`, ignoring
/// any client-supplied value entirely (the core fix this migration makes
/// -- a session can't spoof another tenant by editing a query param).
/// API-key path: unchanged, client-supplied `tenant` required (the
/// existing self-hosted-single-operator trust model, kept exactly as-is).
fn resolve_tenant(user: &Option<axum::Extension<User>>, provided: Option<&str>) -> Result<String, (StatusCode, Json<Value>)> {
    if let Some(axum::Extension(user)) = user {
        return Ok(user.tenant.clone());
    }
    provided.map(str::to_string).ok_or_else(|| (StatusCode::BAD_REQUEST, Json(json!({ "error": "tenant is required" }))))
}

async fn health() -> &'static str {
    "ok"
}

/// `AGENTOPS_DATA_DIR` (default `~/.agentops`) — the directory every
/// `default_*_path`/`default_*_dir` function below resolves under, so a
/// self-hosted deployment only needs to set one path to relocate all of
/// this crate's storage at once; each specific `AGENTOPS_*_DB`/`*_DIR` env
/// var (checked at each call site, not here) still overrides its own
/// default individually.
fn agentops_data_dir() -> std::path::PathBuf {
    if let Ok(dir) = std::env::var("AGENTOPS_DATA_DIR") {
        return std::path::PathBuf::from(dir);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    std::path::PathBuf::from(home).join(".agentops")
}

fn default_modules_db_path() -> std::path::PathBuf {
    agentops_data_dir().join("modules.sqlite")
}

/// One SQLite file per tenant under `dir` — `<org>.db`, or `default.db`
/// when no `org` is given (a single self-hosted operator with no
/// multi-tenant needs). See the module doc comment for why this replaces
/// `main`'s `TenantContext` parameter.
fn docbrain_db_path_for_org(dir: &std::path::Path, org: Option<&str>) -> std::path::PathBuf {
    match org {
        Some(org) => dir.join(format!("{org}.db")),
        None => dir.join("default.db"),
    }
}

/// `$AGENTOPS_DATA_DIR/docbrain-tenants/` — a directory, not a single file,
/// since this deployment routes one file per tenant. Named
/// `docbrain-tenants` rather than `docbrain` specifically to avoid reading
/// as a near-duplicate of `docbrain_mcp::default_db_path`'s single-file
/// `docbrain.db` (the unrelated single-tenant, CLI-facing store) sitting in
/// the same parent directory — the two are genuinely different storage
/// models, not two copies of the same thing.
fn default_docbrain_db_dir() -> std::path::PathBuf {
    agentops_data_dir().join("docbrain-tenants")
}

fn default_accounts_db_path() -> std::path::PathBuf {
    agentops_data_dir().join("accounts.sqlite")
}

fn default_integrations_db_path() -> std::path::PathBuf {
    agentops_data_dir().join("integrations.sqlite")
}

fn default_teams_db_path() -> std::path::PathBuf {
    agentops_data_dir().join("teams.sqlite")
}

/// `$AGENTOPS_DATA_DIR/repo-checkouts/` -- where indexing jobs actually
/// clone a connection's remote into a working tree before scanning it.
fn default_repo_checkouts_dir() -> std::path::PathBuf {
    agentops_data_dir().join("repo-checkouts")
}

/// Reads `AGENTOPS_GITHUB_APP_ID`, `AGENTOPS_GITHUB_APP_PRIVATE_KEY` (a raw
/// PEM string) or `AGENTOPS_GITHUB_APP_PRIVATE_KEY_PATH` (a file containing
/// one, easier to manage than a multi-line PEM crammed into an env var),
/// and `AGENTOPS_GITHUB_WEBHOOK_SECRET` -- same family as the existing
/// `AGENTOPS_GITHUB_APP_SLUG`. All three must be present for GitHub App
/// support to be enabled; a partially-configured deployment (e.g. someone
/// set the App id but forgot the webhook secret) is treated the same as
/// fully unconfigured, not a half-working state, so a config mistake fails
/// obviously (routes 404/503) rather than partially working in a confusing
/// way.
fn load_github_app_config() -> Option<github_app_routes::GitHubAppConfig> {
    let app_id: u64 = std::env::var("AGENTOPS_GITHUB_APP_ID").ok()?.parse().ok()?;
    let private_key_pem = match std::env::var("AGENTOPS_GITHUB_APP_PRIVATE_KEY") {
        Ok(pem) => pem,
        Err(_) => std::fs::read_to_string(std::env::var("AGENTOPS_GITHUB_APP_PRIVATE_KEY_PATH").ok()?).ok()?,
    };
    let webhook_secret = std::env::var("AGENTOPS_GITHUB_WEBHOOK_SECRET").ok()?;
    Some(github_app_routes::GitHubAppConfig { app_id, private_key_pem, webhook_secret })
}

/// Resolves a `LinearConfig` for `tenant` — **vault only, deliberately no
/// env-var fallback.** Env-var-sourced credentials were an intentional
/// bootstrap-era convenience (Phase 6/7's own live testing used
/// `AGENTOPS_LINEAR_API_KEY` directly); once the vault actually works
/// end-to-end, keeping that fallback alive undermines the entire point of
/// having it — a hands-off deployment shouldn't need an operator-set env
/// var per provider at all. The only supported ways to get a credential
/// into this deployment now are `POST /integrations/{provider}` (BYOK,
/// already built) or a future OAuth flow (designed, not yet implemented).
/// A tenant with nothing stored gets a clear, actionable error, not a
/// silent env-var read.
fn resolve_linear_config(credentials: &agentops_integrations::CredentialStore, secrets: &dyn SecretsProvider, tenant: &str) -> anyhow::Result<agentops_linear::LinearConfig> {
    let cred = credentials
        .get_credential(secrets, tenant, "linear")?
        .ok_or_else(|| anyhow::anyhow!("no Linear credential stored for tenant {tenant:?} — add one via POST /integrations/linear"))?;
    Ok(agentops_linear::LinearConfig { api_key: cred.secret.to_string(), api_url: "https://api.linear.app/graphql".to_string() })
}

/// Same vault-only resolution as `resolve_linear_config`, for
/// `agentops_llm::AnthropicConfig`.
fn resolve_anthropic_config(credentials: &agentops_integrations::CredentialStore, secrets: &dyn SecretsProvider, tenant: &str) -> anyhow::Result<agentops_llm::AnthropicConfig> {
    let cred = credentials
        .get_credential(secrets, tenant, "anthropic")?
        .ok_or_else(|| anyhow::anyhow!("no Anthropic credential stored for tenant {tenant:?} — add one via POST /integrations/anthropic"))?;
    Ok(agentops_llm::AnthropicConfig { api_key: cred.secret.to_string(), model: "claude-sonnet-5".to_string(), max_tokens: 1024, api_url: "https://api.anthropic.com/v1/messages".to_string() })
}

/// Builds this service's complete, ready-to-serve `Router` — every route
/// table this crate owns (`/repos/*`, `/search*`, `/docs/search*`,
/// `/consolidate/model`, the GitHub/Linear webhook receivers, `/auth/*`,
/// `/integrations*`, `/team*`) merged together, plus `/health`. Split out of
/// [`run`] so a merged multi-service process (e.g. `agentops-server`) can
/// compose this alongside `agentops-api`'s and `docbrain-api`'s routers
/// without also binding a socket itself. Reads `AGENTOPS_SECRETS_MASTER_KEY`
/// (required — see `agentops_repo_access::secrets::EnvSecretsProvider`),
/// `AGENTOPS_API_KEY_HASH` (optional — unset means unauthenticated,
/// matching `agentops-api`/`docbrain-api`'s convention), `AGENTOPS_GITHUB_APP_SLUG`
/// (optional), and `AGENTOPS_QDRANT_URL` (optional — enables `/search*` on
/// its own; loading the BGE-M3 model can take real time on first run,
/// downloading it if not already cached).
///
/// `include_tools` mirrors [`build_router`]/[`build_router_without_tools`]'s
/// split: `false` when composing into a process that already mounts
/// `agentops-api`'s `/tools` (see that function's doc comment for why only
/// one of the three tool tables can be mounted at that exact path today).
pub async fn build_full_router(db_path: &std::path::Path, include_tools: bool) -> anyhow::Result<Router> {
    let store = ConnectionStore::open(db_path)?;
    let secrets: Arc<dyn SecretsProvider + Send + Sync> = Arc::new(agentops_repo_access::secrets::EnvSecretsProvider::from_env()?);
    let github_app_slug = std::env::var("AGENTOPS_GITHUB_APP_SLUG").ok();
    let github_app_config = load_github_app_config();
    if github_app_config.is_none() {
        println!("AGENTOPS_GITHUB_APP_ID/PRIVATE_KEY/WEBHOOK_SECRET not fully set — GitHub App installation/webhook routes disabled for this deployment.");
    }
    let api_key_hash = std::env::var("AGENTOPS_API_KEY_HASH").ok();

    let search_index = match std::env::var("AGENTOPS_QDRANT_URL") {
        Ok(qdrant_url) => {
            println!("AGENTOPS_QDRANT_URL set — loading BGE-M3 + bge-reranker-v2-m3 (download on first run, cached after)...");
            let collection = std::env::var("AGENTOPS_QDRANT_COLLECTION").unwrap_or_else(|_| "agentops_semantic".to_string());
            let index = SemanticIndex::connect(&qdrant_url, &collection)?;
            index.ensure_collection().await?;
            println!("Semantic search ready (collection {collection:?} at {qdrant_url}).");
            Some(Arc::new(AsyncMutex::new(index)))
        }
        Err(_) => {
            println!("AGENTOPS_QDRANT_URL not set — /search* routes disabled for this deployment.");
            None
        }
    };

    let docbrain_db_dir = std::env::var("DOCBRAIN_DB_DIR").map(std::path::PathBuf::from).unwrap_or_else(|_| default_docbrain_db_dir());
    std::fs::create_dir_all(&docbrain_db_dir)?;

    // Resolved early (rather than alongside the other accounts/teams
    // connections further down) because `build_router`'s own
    // `/repos/*` session path needs its own independent connections to
    // both files, same "second connection to the same store" pattern
    // `accounts_for_linear`/`accounts_for_teams` already established below.
    let accounts_db_path = std::env::var("AGENTOPS_ACCOUNTS_DB").map(std::path::PathBuf::from).unwrap_or_else(|_| default_accounts_db_path());
    let teams_db_path = std::env::var("AGENTOPS_TEAMS_DB").map(std::path::PathBuf::from).unwrap_or_else(|_| default_teams_db_path());
    let accounts_for_repos = agentops_accounts::AccountStore::open(&accounts_db_path)?;
    let teams_for_repos = agentops_teams::TeamStore::open(&teams_db_path)?;

    // Same primary `db_path` file as `ConnectionStore` (not its own
    // `AGENTOPS_*_DB` env var) -- `ConnectionStore::open(db_path)` above
    // already establishes that this specific subsystem shares the primary
    // file rather than following Accounts/Teams/Integrations/Modules' own
    // per-subsystem file convention.
    let indexing_store = agentops_repo_access::indexing_store::IndexingStore::open(db_path)?;
    let repo_checkouts_dir = std::env::var("AGENTOPS_REPO_CHECKOUTS_DIR").map(std::path::PathBuf::from).unwrap_or_else(|_| default_repo_checkouts_dir());
    std::fs::create_dir_all(&repo_checkouts_dir)?;

    // Independent second connections to the same primary db file, for the
    // public `/webhooks/github` receiver -- same "own connection, not a
    // shared AppState handle" precedent `LinearModuleState` already
    // established (`linear_webhook.rs`'s doc comment), since `build_router`
    // doesn't expose the `AppState` it constructs internally.
    let webhook_deps = indexing::IndexingDeps {
        indexing: Arc::new(Mutex::new(agentops_repo_access::indexing_store::IndexingStore::open(db_path)?)),
        connections: Arc::new(Mutex::new(ConnectionStore::open(db_path)?)),
        secrets: secrets.clone(),
        search_index: search_index.clone(),
        repo_checkouts_dir: repo_checkouts_dir.clone(),
        github_app_config: github_app_config.clone(),
    };

    let mut app = build_router_with_tools_flag(
        store,
        secrets.clone(),
        github_app_slug,
        api_key_hash,
        search_index,
        docbrain_db_dir.clone(),
        Some(accounts_for_repos),
        Some(teams_for_repos),
        indexing_store,
        repo_checkouts_dir,
        github_app_config.clone(),
        include_tools,
    );
    app = github_app_routes::merge_github_webhook_route(app, webhook_deps, github_app_config);

    // Phase 7: accounts + the generic integrations vault. Phase 6c: the
    // generic per-tenant module-enrollment store (Linear auto-kickoff is
    // its first consumer). Always opened (cheap — SQLite files created on
    // demand). `build_accounts_integrations_router` takes `accounts`/
    // `credentials` by value, so `linear_webhook`'s own routes open their
    // own independent second connections to the same files rather than
    // sharing these — see `linear_webhook::LinearModuleState`'s doc comment
    // for why that's a deliberate choice, not an accidental duplicate.
    let credentials_db_path = std::env::var("AGENTOPS_INTEGRATIONS_DB").map(std::path::PathBuf::from).unwrap_or_else(|_| default_integrations_db_path());
    let modules_db_path = std::env::var("AGENTOPS_MODULES_DB").map(std::path::PathBuf::from).unwrap_or_else(|_| default_modules_db_path());
    let accounts = agentops_accounts::AccountStore::open(&accounts_db_path)?;
    let credentials = agentops_integrations::CredentialStore::open(&credentials_db_path)?;

    // Phase 6c: no more boot-time config file, no more global `enabled`
    // switch, no more hardcoded "default" tenant — auto-kickoff is a
    // per-account opt-in now (`POST /linear/auto-kickoff`, session-authed).
    // These routes and the webhook receiver are mounted unconditionally;
    // whether anything is actually enrolled is a runtime, per-tenant
    // question the routes/handler answer themselves, not a startup gate.
    let linear_modules = Arc::new(Mutex::new(agentops_integration_modules::ModuleStore::open(&modules_db_path)?));
    let linear_credentials_2 = Arc::new(Mutex::new(agentops_integrations::CredentialStore::open(&credentials_db_path)?));
    let accounts_for_linear = Arc::new(Mutex::new(agentops_accounts::AccountStore::open(&accounts_db_path)?));

    app = linear_webhook::merge_linear_webhook_route(app, linear_modules.clone(), linear_credentials_2.clone(), secrets.clone());
    app = linear_webhook::merge_linear_module_routes(app, linear_modules, linear_credentials_2, secrets.clone(), accounts_for_linear);
    if std::env::var("AGENTOPS_LINEAR_WEBHOOK_URL").is_err() {
        println!("AGENTOPS_LINEAR_WEBHOOK_URL is not set — POST /linear/auto-kickoff will report 503 until it's configured.");
    }
    println!("Linear auto-kickoff live: POST /linear/auto-kickoff (session-authed opt-in), /webhooks/linear.");

    // Independent connection to teams.sqlite, same pattern as
    // `accounts_for_repos`/`accounts_for_teams` -- needed to gate the
    // org-wide `/integrations*` routes behind `integrations.manage`.
    let teams_for_integrations = agentops_teams::TeamStore::open(&teams_db_path)?;
    app = app.merge(build_accounts_integrations_router(accounts, credentials, secrets, teams_for_integrations));
    println!("Accounts + integrations vault live: POST /auth/signup, POST /auth/login, GET /auth/me, POST /auth/logout, GET /integrations, POST /integrations/{{provider}}.");

    // Team Management's Members tab — own connection to the same
    // accounts.sqlite/teams.sqlite (see `team` module's doc comment for
    // why, matching `accounts_for_linear`/`accounts_for_repos` above). Also
    // its own `credentials`/`docbrain_db_dir` -- needed only by
    // `POST /team/delete-organization`'s cascade.
    let accounts_for_teams = agentops_accounts::AccountStore::open(&accounts_db_path)?;
    let teams = agentops_teams::TeamStore::open(&teams_db_path)?;
    let repos_for_teams = ConnectionStore::open(db_path)?;
    let credentials_for_teams = agentops_integrations::CredentialStore::open(&credentials_db_path)?;
    app = app.merge(build_team_router(accounts_for_teams, teams, repos_for_teams, credentials_for_teams, docbrain_db_dir));
    println!("Team Management live: GET /team, GET /team/members, PATCH/DELETE /team/members/{{id}}, GET/PUT /team/repo-access, POST /team/delete-organization.");

    Ok(app.merge(health_router()))
}

/// Binds `addr` and serves [`build_full_router`]'s output until the process
/// is killed.
pub async fn run(addr: &str, db_path: &std::path::Path) -> anyhow::Result<()> {
    let api_key_hash = std::env::var("AGENTOPS_API_KEY_HASH").ok();
    let auth_status = if api_key_hash.is_some() { "API key required" } else { "UNAUTHENTICATED (set AGENTOPS_API_KEY_HASH to require a key)" };
    let app = build_full_router(db_path, true).await?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("agentops-heavy-api listening on {addr} (auth: {auth_status})");
    axum::serve(listener, app).await?;
    Ok(())
}

#[derive(Debug, Deserialize)]
struct ConnectRequest {
    /// `None` on the session path (the frontend never sends it, tenant is
    /// derived from the session instead); required on the API-key path.
    /// See `resolve_tenant`. `#[serde(default)]` even though `serde_json`
    /// already defaults a missing `Option` field to `None` on its own --
    /// kept for symmetry with `TenantQuery::tenant` below, where the
    /// attribute is load-bearing, not just belt-and-suspenders.
    #[serde(default)]
    tenant: Option<String>,
    repo_id: String,
    repo_url: String,
}

/// The only shape a `RepoConnection` is ever allowed to leave this server
/// in — see the module doc: no encrypted-key field exists on this type at
/// all, so there's nothing to forget to strip.
#[derive(Debug, Serialize)]
struct ConnectionView {
    id: String,
    tenant: String,
    repo_url: String,
    method: String,
    public_key_openssh: Option<String>,
    status: String,
    created_at: String,
}

impl From<RepoConnection> for ConnectionView {
    fn from(c: RepoConnection) -> Self {
        let status = match &c.status {
            ConnectionStatus::Pending => "pending".to_string(),
            ConnectionStatus::Active => "active".to_string(),
            ConnectionStatus::Failed(reason) => format!("failed: {reason}"),
        };
        let method = match c.method {
            agentops_repo_access::store::ConnectionMethod::Ssh => "ssh".to_string(),
            agentops_repo_access::store::ConnectionMethod::GitHubApp => "github_app".to_string(),
        };
        ConnectionView { id: c.id, tenant: c.tenant, repo_url: c.repo_url, method, public_key_openssh: c.public_key_openssh, status, created_at: c.created_at }
    }
}

/// Session-path-only capability gate -- a `None` `user` (API-key path)
/// skips it entirely, matching that path's existing trust model (a valid
/// API key already implies full access, same as before this migration).
fn require_session_capability(state: &AppState, user: &Option<axum::Extension<User>>, tenant: &str, capability: &str) -> Result<(), (StatusCode, Json<Value>)> {
    let Some(axum::Extension(user)) = user else { return Ok(()) };
    let Some(teams) = &state.teams else {
        // A session verified but this deployment has no `teams` store
        // wired up -- an inconsistent configuration (accounts without
        // teams), fail closed rather than silently granting access.
        return Err((StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "session auth is configured but team/capability data is not" }))));
    };
    let teams = teams.lock().unwrap();
    // Must run before the capability check -- a brand-new user who has
    // never touched `/team/*` has no membership row at all yet, and
    // `has_capability` fails closed for that (correctly, in isolation).
    // Without this, such a user would be universally 403'd from every
    // session-authed `/repos/*` route, since only `/team/*` handlers used
    // to run this backfill. Caught via live testing against this exact
    // scenario (a fresh signup hitting `/repos` before ever visiting Team
    // Management), not assumed.
    if let Err(e) = teams.ensure_membership(tenant, user.id) {
        return Err((StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))));
    }
    match agentops_teams::has_capability(&teams, tenant, user.id, capability) {
        Ok(true) => Ok(()),
        Ok(false) => Err((StatusCode::FORBIDDEN, Json(json!({ "error": format!("missing required capability: {capability}") })))),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() })))),
    }
}

async fn connect_repo(State(state): State<AppState>, user: Option<axum::Extension<User>>, Json(req): Json<ConnectRequest>) -> (StatusCode, Json<Value>) {
    let tenant = match resolve_tenant(&user, req.tenant.as_deref()) {
        Ok(t) => t,
        Err(e) => return e,
    };
    if let Err(e) = require_session_capability(&state, &user, &tenant, agentops_teams::CAP_REPOS_CONNECT) {
        return e;
    }

    let keypair = match agentops_repo_access::generate_deploy_keypair_for_repo(state.secrets.as_ref(), &tenant, &req.repo_id) {
        Ok(k) => k,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("generating deploy key: {e}") }))),
    };

    let store = state.store.lock().unwrap();
    match store.create_ssh_connection(&tenant, &req.repo_id, &req.repo_url, &keypair) {
        Ok(connection) => {
            let view = ConnectionView::from(connection);
            (
                StatusCode::CREATED,
                Json(json!({
                    "connection": view,
                    "instructions": "Add public_key_openssh as a read-only Deploy Key on the repo, then POST /repos/{id}/verify?tenant=... to confirm it works.",
                })),
            )
        }
        Err(e) => (StatusCode::CONFLICT, Json(json!({ "error": e.to_string() }))),
    }
}

#[derive(Debug, Deserialize)]
struct TenantQuery {
    /// `None` on the session path -- see `ConnectRequest::tenant`. Also
    /// deliberately **ignored** on the session path even when a client
    /// sends one (`resolve_tenant` never reads it once a `User` is
    /// present), so a session-authed caller can't spoof another tenant by
    /// setting `?tenant=` to something other than their own.
    ///
    /// `#[serde(default)]` is **load-bearing** here, not decorative --
    /// `axum::extract::Query` deserializes via `serde_urlencoded`, which
    /// (unlike `serde_json`) does not treat a struct's only field being
    /// `Option<T>` as automatically satisfied when the query string is
    /// totally absent; without this attribute, a bare `GET /repos` with no
    /// `?tenant=` at all 400s with "missing field `tenant`" before the
    /// handler ever runs. Caught via live testing against a real server
    /// (curl) after this exact request shape passed clean in every
    /// in-process `Router::oneshot` test -- worth flagging as a real gap
    /// in that testing approach's fidelity for `Query` extractors
    /// specifically, not just a one-off bug.
    #[serde(default)]
    tenant: Option<String>,
}

async fn list_repos(State(state): State<AppState>, user: Option<axum::Extension<User>>, Query(q): Query<TenantQuery>) -> (StatusCode, Json<Value>) {
    let tenant = match resolve_tenant(&user, q.tenant.as_deref()) {
        Ok(t) => t,
        Err(e) => return e,
    };
    if let Err(e) = require_session_capability(&state, &user, &tenant, agentops_teams::CAP_REPOS_VIEW) {
        return e;
    }

    let connections = {
        let store = state.store.lock().unwrap();
        match store.list_connections(&tenant) {
            Ok(c) => c,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
        }
    };

    // Session path only: filter to repos this member actually has access
    // to (role default + any per-user override) -- omitted from the
    // response entirely, not just hidden client-side. The API-key path is
    // unfiltered, matching its existing full-access trust model.
    let connections = if let (Some(axum::Extension(user)), Some(teams)) = (&user, &state.teams) {
        let teams = teams.lock().unwrap();
        match connections
            .into_iter()
            .map(|c| {
                let allowed = agentops_teams::effective_repo_access(&teams, &tenant, user.id, &c.id)?;
                Ok((c, allowed))
            })
            .collect::<anyhow::Result<Vec<_>>>()
        {
            Ok(with_access) => with_access.into_iter().filter(|(_, allowed)| *allowed).map(|(c, _)| c).collect(),
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
        }
    } else {
        connections
    };

    // Server-computed, so the frontend never reimplements capability logic
    // client-side just to decide whether to show a "Connect repository"
    // button -- API-key path always `true` (full access, matching that
    // path's existing trust model, same as the filtering above).
    let can_connect = if let (Some(axum::Extension(user)), Some(teams)) = (&user, &state.teams) {
        let teams = teams.lock().unwrap();
        match agentops_teams::has_capability(&teams, &tenant, user.id, agentops_teams::CAP_REPOS_CONNECT) {
            Ok(v) => v,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
        }
    } else {
        true
    };

    let views: Vec<ConnectionView> = connections.into_iter().map(ConnectionView::from).collect();
    (StatusCode::OK, Json(json!({ "connections": views, "can_connect": can_connect })))
}

async fn verify_repo(State(state): State<AppState>, user: Option<axum::Extension<User>>, AxumPath(id): AxumPath<String>, Query(q): Query<TenantQuery>) -> (StatusCode, Json<Value>) {
    let tenant = match resolve_tenant(&user, q.tenant.as_deref()) {
        Ok(t) => t,
        Err(e) => return e,
    };
    if let Err(e) = require_session_capability(&state, &user, &tenant, agentops_teams::CAP_REPOS_CONNECT) {
        return e;
    }

    let connection = {
        let store = state.store.lock().unwrap();
        match store.get_connection(&tenant, &id) {
            Ok(Some(c)) => c,
            Ok(None) => return (StatusCode::NOT_FOUND, Json(json!({ "error": "no such connection for this tenant" }))),
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
        }
    };

    let Some(encrypted_key) = &connection.encrypted_private_key_openssh else {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "connection has no SSH key (not an SSH-method connection)" })));
    };

    let result = verify_ssh_connection(state.secrets.as_ref(), &tenant, &id, encrypted_key, &connection.repo_url);

    let outcome = {
        let store = state.store.lock().unwrap();
        match &result {
            Ok(()) => {
                let _ = store.set_status(&tenant, &id, ConnectionStatus::Active);
                Ok(())
            }
            Err(e) => {
                let reason = e.to_string();
                let _ = store.set_status(&tenant, &id, ConnectionStatus::Failed(reason.clone()));
                Err(reason)
            }
        }
    };

    match outcome {
        Ok(()) => {
            // The key check above is a throwaway clone into a scratch temp
            // dir, discarded either way -- the job below does its own,
            // separate real clone into the job's working tree. A known,
            // deliberate simplification vs. reusing that first clone as the
            // job's own checkout (which would save one clone): kept simple
            // for this pass, flagged rather than silently accepted as
            // optimal.
            let job_id = match indexing::create_and_spawn_job(&state.indexing_deps(), tenant.clone(), connection.clone(), agentops_repo_access::indexing_store::JobKind::Initial) {
                Ok(id) => Some(id),
                Err(e) => {
                    // Verification itself succeeded -- don't fail the whole
                    // request over the indexing job failing to even start;
                    // report it, let the caller retry indexing separately.
                    eprintln!("verify_repo: connection {tenant}/{id} verified but failed to start indexing job: {e}");
                    None
                }
            };
            (StatusCode::OK, Json(json!({ "status": "active", "job_id": job_id })))
        }
        Err(reason) => (StatusCode::OK, Json(json!({ "status": "failed", "reason": reason }))),
    }
}

/// Attempts a throwaway clone into a scratch temp directory purely to
/// confirm the deploy key actually works — this is a credential check, not
/// the repo-sync pipeline (that's separate, future work), so the clone is
/// discarded either way.
fn verify_ssh_connection(secrets: &dyn SecretsProvider, tenant: &str, repo_id: &str, encrypted_key: &str, repo_url: &str) -> anyhow::Result<()> {
    let unlocked = agentops_repo_access::UnlockedKey::unlock_for_repo(secrets, tenant, repo_id, encrypted_key)?;
    let dest = std::env::temp_dir().join(format!("agentops-verify-{tenant}-{repo_id}-{}", std::process::id()));
    let result = agentops_repo_access::clone_repo(repo_url, &dest, &unlocked, agentops_repo_access::GITHUB_KNOWN_HOSTS);
    let _ = std::fs::remove_dir_all(&dest);
    result
}

/// `POST /repos/{id}/regenerate-key` -- the failure screen's "Regenerate
/// deploy key" action. SSH-only: 404s for a GitHub App connection (nothing
/// to regenerate, there's no key custodied on our side for that method).
async fn regenerate_key(State(state): State<AppState>, user: Option<axum::Extension<User>>, AxumPath(id): AxumPath<String>, Query(q): Query<TenantQuery>) -> (StatusCode, Json<Value>) {
    let tenant = match resolve_tenant(&user, q.tenant.as_deref()) {
        Ok(t) => t,
        Err(e) => return e,
    };
    if let Err(e) = require_session_capability(&state, &user, &tenant, agentops_teams::CAP_REPOS_CONNECT) {
        return e;
    }

    let connection = {
        let store = state.store.lock().unwrap();
        match store.get_connection(&tenant, &id) {
            Ok(Some(c)) => c,
            Ok(None) => return (StatusCode::NOT_FOUND, Json(json!({ "error": "no such connection for this tenant" }))),
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
        }
    };
    if connection.method != agentops_repo_access::store::ConnectionMethod::Ssh {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "not an SSH-method connection -- nothing to regenerate" })));
    }

    let keypair = match agentops_repo_access::generate_deploy_keypair_for_repo(state.secrets.as_ref(), &tenant, &id) {
        Ok(k) => k,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("generating deploy key: {e}") }))),
    };
    let store = state.store.lock().unwrap();
    match store.replace_ssh_keypair(&tenant, &id, &keypair) {
        Ok(connection) => (StatusCode::OK, Json(json!({ "connection": ConnectionView::from(connection) }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    }
}

async fn github_app_install_url(State(state): State<AppState>) -> (StatusCode, Json<Value>) {
    match &state.github_app_slug {
        Some(slug) => (StatusCode::OK, Json(json!({ "install_url": agentops_github_app::install_url(slug) }))),
        None => (StatusCode::NOT_FOUND, Json(json!({ "error": "no GitHub App is configured for this deployment (AGENTOPS_GITHUB_APP_SLUG not set)" }))),
    }
}

/// `503 Service Unavailable` — this deployment simply doesn't have Qdrant
/// configured.
fn search_not_configured() -> (StatusCode, Json<Value>) {
    (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "error": "semantic search is not configured for this deployment (AGENTOPS_QDRANT_URL not set)" })))
}

/// `GET /tools` — reuses `agentops-heavy-mcp`'s tool table directly (one
/// implementation, two transports, same pattern `agentops-api`/
/// `docbrain-api` already use for their own `/tools`), closing the gap
/// where this crate's REST doc-search/index handlers used to reimplement
/// logic in parallel with the MCP tool of the same purpose.
async fn heavy_tools_list_handler() -> Json<Value> {
    Json(json!({ "tools": agentops_heavy_mcp::list_tools() }))
}

/// `POST /tools/{name}` — same 503 posture as `/search*`/`/docs/search*`
/// when Qdrant isn't configured, since every tool in this table needs the
/// same `SemanticIndex`.
async fn heavy_tools_call_handler(State(state): State<AppState>, AxumPath(name): AxumPath<String>, body: Option<Json<Value>>) -> (StatusCode, Json<Value>) {
    let Some(search_index) = &state.search_index else { return search_not_configured() };
    let empty = json!({});
    let args = body.map(|Json(v)| v).unwrap_or(empty);
    let mut index = search_index.lock().await;
    let result = agentops_heavy_mcp::call_tool(&mut index, &name, &args).await;
    (StatusCode::OK, Json(serde_json::to_value(result).unwrap()))
}

#[derive(Debug, Deserialize)]
struct SearchIndexRequest {
    path: String,
}

/// Embeds and indexes every Symbol/Gotcha/Decision node from a scanned
/// repo's graph store — call this after `agentops install`, before
/// `/search` will return anything useful for that repo.
async fn search_index_handler(State(state): State<AppState>, Json(req): Json<SearchIndexRequest>) -> (StatusCode, Json<Value>) {
    let Some(search_index) = &state.search_index else { return search_not_configured() };

    // Collecting items is synchronous and fully finishes — including
    // dropping `store` — before the `.await` below. `&dyn GraphStore` isn't
    // provably `Sync` (SQLite connections aren't), so it must never be held
    // across an await; see collect_index_items's doc comment.
    let items = {
        let repo_path = std::path::Path::new(&req.path);
        let db_path = repo_path.join(".context").join("graph.db");
        let store = match agentops_graph::SqliteGraphStore::open(&db_path) {
            Ok(s) => s,
            Err(e) => return (StatusCode::NOT_FOUND, Json(json!({ "error": format!("opening graph store at {}: {e}", db_path.display()) }))),
        };
        // Must be the canonicalized repo name (what nodes are actually
        // stored under), not the raw request path — see repo_name's doc
        // comment; a real bug caught via live testing against this repo.
        let repo = agentops_heavy_embeddings::repo_name(repo_path);
        match agentops_heavy_embeddings::collect_index_items(&store, &repo) {
            Ok(items) => items,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
        }
    };

    let mut index = search_index.lock().await;
    match index.index_items(&items).await {
        Ok(count) => (StatusCode::OK, Json(json!({ "indexed": count }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    }
}

#[derive(Debug, Deserialize)]
struct SearchQuery {
    path: String,
    q: String,
    #[serde(default = "default_top_k")]
    top_k: u64,
}

fn default_top_k() -> u64 {
    5
}

/// Real semantic search — ranks by meaning, not keyword overlap. Requires
/// `/search/index` to have been run for `path` at least once first.
async fn search_query_handler(State(state): State<AppState>, Query(q): Query<SearchQuery>) -> (StatusCode, Json<Value>) {
    let Some(search_index) = &state.search_index else { return search_not_configured() };

    let mut index = search_index.lock().await;
    // Must match the same repo_name value indexing used — see
    // search_index_handler's comment.
    let repo = agentops_heavy_embeddings::repo_name(std::path::Path::new(&q.path));
    match index.search(&q.q, q.top_k, Some(&repo)).await {
        Ok(hits) => {
            let results: Vec<Value> = hits
                .into_iter()
                .map(|h| json!({ "id": h.id, "score": h.score, "kind": h.kind, "name": h.name, "path": h.path, "text": h.text }))
                .collect();
            (StatusCode::OK, Json(json!({ "results": results })))
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    }
}

#[derive(Debug, Deserialize)]
struct DocsIndexRequest {
    slug: String,
    org: Option<String>,
}

/// Embeds and indexes every doc node docbrain has for `slug` (across all its
/// scraped versions), so `/docs/search` has something to find — the REST
/// equivalent of `agentops-heavy-mcp`'s `index_docs` tool, same underlying
/// `collect_doc_index_items`/shared-collection design (see that crate's doc
/// comment for why docbrain and codebrain content safely share one Qdrant
/// collection and one BGE-M3 model instance). `org` selects which tenant's
/// docbrain SQLite file to read from — see the module doc comment.
async fn docs_search_index_handler(State(state): State<AppState>, Json(req): Json<DocsIndexRequest>) -> (StatusCode, Json<Value>) {
    let Some(search_index) = &state.search_index else { return search_not_configured() };
    let db_path = docbrain_db_path_for_org(&state.docbrain_db_dir, req.org.as_deref());

    // Same discipline as search_index_handler: the store is opened and
    // fully done with — including being dropped — before the `.await`
    // below. It wraps a rusqlite::Connection (!Sync), so a reference to it
    // must never cross an .await.
    let items = {
        let store = match docbrain_graph::SqliteDocbrainStore::open(&db_path) {
            Ok(s) => s,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("opening docbrain store at {}: {e}", db_path.display()) }))),
        };
        match agentops_heavy_embeddings::collect_doc_index_items(&store, &req.slug) {
            Ok(items) => items,
            Err(e) => return (StatusCode::BAD_REQUEST, Json(json!({ "error": e.to_string() }))),
        }
    };

    let mut index = search_index.lock().await;
    match index.index_items(&items).await {
        Ok(count) => (StatusCode::OK, Json(json!({ "indexed": count }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    }
}

#[derive(Debug, Deserialize)]
struct DocsSearchQuery {
    slug: String,
    q: String,
    #[serde(default = "default_top_k")]
    top_k: u64,
}

/// Semantic search scoped to docbrain doc content only — `search_scoped`'s
/// `kind: "doc"` filter is what keeps this from ever surfacing a codebrain
/// symbol/gotcha/decision result, even from the same shared collection.
/// No `org` parameter here, matching `main`: Qdrant filters by `repo`/`kind`,
/// not tenant, so tenant isolation for docbrain content is enforced at
/// index time instead (see the module doc comment's flagged limitation) —
/// `docs_search_index_handler` only ever reads doc nodes from the indexing
/// caller's own tenant's docbrain file in the first place, so nothing a
/// different tenant couldn't already see gets into the shared index.
async fn docs_search_query_handler(State(state): State<AppState>, Query(q): Query<DocsSearchQuery>) -> (StatusCode, Json<Value>) {
    let Some(search_index) = &state.search_index else { return search_not_configured() };

    let mut index = search_index.lock().await;
    match index.search_scoped(&q.q, q.top_k, Some(&q.slug), Some("doc")).await {
        Ok(hits) => {
            let results: Vec<Value> = hits
                .into_iter()
                .map(|h| json!({ "id": h.id, "score": h.score, "slug": h.repo, "topic": h.name, "version": h.path, "text": h.text }))
                .collect();
            (StatusCode::OK, Json(json!({ "results": results })))
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    }
}

#[derive(Debug, Deserialize)]
struct ConsolidateModelRequest {
    path: String,
}

#[derive(Debug, Serialize)]
struct ConsolidateModelResponse {
    attempted: bool,
    promoted: bool,
    examples_used: usize,
    candidate_score: f64,
    baseline_score: f64,
    promoted_version: Option<u32>,
    reason: String,
}

/// On-demand REST trigger for Initiative 6 (CLS-inspired retrieval plan):
/// LoRA fine-tunes a small local code model on this repo's own curated
/// Gotcha/Decision notes. No `search_index`/Qdrant dependency at all —
/// unlike every other handler in this file, this one doesn't touch
/// `AppState.search_index`. Runs on a blocking thread (`spawn_blocking`):
/// `agentops_heavy_consolidate::consolidate_model` does real, synchronous
/// CPU work (model download on first run, training, eval) that would
/// otherwise stall this server's whole async runtime for the entire
/// duration of a call, unlike every other handler here.
async fn consolidate_model_handler(Json(req): Json<ConsolidateModelRequest>) -> (StatusCode, Json<Value>) {
    let result = tokio::task::spawn_blocking(move || {
        let repo_path = std::path::Path::new(&req.path);
        let db_path = repo_path.join(".context").join("graph.db");
        let store = agentops_graph::SqliteGraphStore::open(&db_path).map_err(|e| anyhow::anyhow!("opening graph store at {}: {e}", db_path.display()))?;
        let repo = agentops_heavy_embeddings::repo_name(repo_path);
        agentops_heavy_consolidate::consolidate_model(&store, &repo)
    })
    .await;

    match result {
        Ok(Ok(report)) => (
            StatusCode::OK,
            Json(json!(ConsolidateModelResponse {
                attempted: report.attempted,
                promoted: report.promoted,
                examples_used: report.examples_used,
                candidate_score: report.candidate_score,
                baseline_score: report.baseline_score,
                promoted_version: report.promoted_version,
                reason: report.reason,
            })),
        ),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("consolidation task panicked: {e}") }))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentops_graph::GraphStore as _;
    use agentops_repo_access::secrets::EnvSecretsProvider;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use std::path::PathBuf;
    use tower::ServiceExt;

    fn test_state() -> (ConnectionStore, Arc<dyn SecretsProvider + Send + Sync>) {
        let store = ConnectionStore::open_in_memory().unwrap();
        let secrets: Arc<dyn SecretsProvider + Send + Sync> = Arc::new(EnvSecretsProvider::from_hex(&"22".repeat(32)).unwrap());
        (store, secrets)
    }

    fn test_indexing_store() -> agentops_repo_access::indexing_store::IndexingStore {
        agentops_repo_access::indexing_store::IndexingStore::open_in_memory().unwrap()
    }

    async fn body_json(response: axum::response::Response) -> Value {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    /// Locks in the deprecation: an empty vault must produce a clear error
    /// naming `POST /integrations/linear`, never silently fall back to
    /// `AGENTOPS_LINEAR_API_KEY` even if that env var happens to be set in
    /// the test process's own environment.
    #[test]
    fn resolve_linear_config_never_falls_back_to_the_env_var() {
        std::env::set_var("AGENTOPS_LINEAR_API_KEY", "lin_api_should_never_be_read");
        let credentials = agentops_integrations::CredentialStore::open_in_memory().unwrap();
        let secrets: Arc<dyn SecretsProvider + Send + Sync> = Arc::new(EnvSecretsProvider::from_hex(&"22".repeat(32)).unwrap());

        let err = resolve_linear_config(&credentials, secrets.as_ref(), "default").unwrap_err();
        assert!(err.to_string().contains("POST /integrations/linear"), "{err}");

        std::env::remove_var("AGENTOPS_LINEAR_API_KEY");
    }

    #[test]
    fn resolve_linear_config_reads_the_vault_once_a_credential_is_stored() {
        let credentials = agentops_integrations::CredentialStore::open_in_memory().unwrap();
        let secrets: Arc<dyn SecretsProvider + Send + Sync> = Arc::new(EnvSecretsProvider::from_hex(&"22".repeat(32)).unwrap());
        credentials
            .store_credential(secrets.as_ref(), "default", agentops_integrations::NewCredential { provider: "linear", auth_type: agentops_integrations::AuthType::ApiKey, secret: "lin_api_from_vault", refresh_token: None, expires_at: None })
            .unwrap();

        let config = resolve_linear_config(&credentials, secrets.as_ref(), "default").unwrap();
        assert_eq!(config.api_key, "lin_api_from_vault");
    }

    #[test]
    fn resolve_anthropic_config_never_falls_back_to_the_env_var() {
        std::env::set_var("AGENTOPS_ANTHROPIC_API_KEY", "sk-should-never-be-read");
        let credentials = agentops_integrations::CredentialStore::open_in_memory().unwrap();
        let secrets: Arc<dyn SecretsProvider + Send + Sync> = Arc::new(EnvSecretsProvider::from_hex(&"22".repeat(32)).unwrap());

        let err = resolve_anthropic_config(&credentials, secrets.as_ref(), "default").unwrap_err();
        assert!(err.to_string().contains("POST /integrations/anthropic"), "{err}");

        std::env::remove_var("AGENTOPS_ANTHROPIC_API_KEY");
    }

    #[tokio::test]
    async fn health_check_ok() {
        let (store, secrets) = test_state();
        let app = build_router(store, secrets, None, None, None, PathBuf::from("unused-docbrain-dir"), None, None, test_indexing_store(), std::env::temp_dir(), None).merge(health_router());
        let resp = app.oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    fn signup(accounts: &agentops_accounts::AccountStore, email: &str) -> (User, String) {
        accounts.signup(agentops_accounts::NewAccount { email, password: "correct horse battery staple", first_name: "Ada", last_name: "Lovelace" }).unwrap()
    }

    #[tokio::test]
    async fn api_key_path_is_unaffected_when_no_session_store_is_configured_and_still_requires_a_body_tenant() {
        let (store, secrets) = test_state();
        let app = build_router(store, secrets, None, None, None, PathBuf::from("unused-docbrain-dir"), None, None, test_indexing_store(), std::env::temp_dir(), None);
        let req = Request::builder()
            .method("POST")
            .uri("/repos/connect")
            .header("content-type", "application/json")
            .body(Body::from(json!({"repo_id": "widgets", "repo_url": "git@github.com:acme/widgets.git"}).to_string()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "no session and no body tenant -- must not silently pick a tenant");
    }

    #[tokio::test]
    async fn session_authed_list_repos_ignores_a_spoofed_tenant_query_param() {
        let (store, secrets) = test_state();
        // A connection that belongs to a *different* tenant, seeded
        // directly on the store before it's handed to the router.
        {
            let keypair = agentops_repo_access::generate_deploy_keypair_for_repo(secrets.as_ref(), "other-tenant", "other-repo").unwrap();
            store.create_ssh_connection("other-tenant", "other-repo", "git@github.com:other/other.git", &keypair).unwrap();
        }

        let accounts = agentops_accounts::AccountStore::open_in_memory().unwrap();
        let (user, token) = signup(&accounts, "dev@example.com");
        let teams = agentops_teams::TeamStore::open_in_memory().unwrap();
        teams.add_member(&user.tenant, user.id, "admin").unwrap();
        {
            let keypair = agentops_repo_access::generate_deploy_keypair_for_repo(secrets.as_ref(), &user.tenant, "own-repo").unwrap();
            store.create_ssh_connection(&user.tenant, "own-repo", "git@github.com:acme/own.git", &keypair).unwrap();
        }

        let app = build_router(store, secrets, None, None, None, PathBuf::from("unused-docbrain-dir"), Some(accounts), Some(teams), test_indexing_store(), std::env::temp_dir(), None);

        let spoofed = Request::builder().uri("/repos?tenant=other-tenant").header("authorization", format!("Bearer {token}")).body(Body::empty()).unwrap();
        let resp = app.oneshot(spoofed).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp).await;
        let connections = body["connections"].as_array().unwrap();
        assert_eq!(connections.len(), 1, "must only ever see the session's own tenant's repos, regardless of the query param");
        assert_eq!(connections[0]["repo_url"], "git@github.com:acme/own.git");
    }

    /// Regression test for a real bug caught via live testing: a brand-new
    /// signup who has never touched `/team/*` has no membership row yet,
    /// so `has_capability` (correctly, in isolation) fails closed --
    /// without `require_session_capability` also running
    /// `teams.ensure_membership` first, such a user was universally 403'd
    /// from every session-authed `/repos/*` route, since only `/team/*`
    /// handlers used to run that backfill.
    #[tokio::test]
    async fn a_brand_new_signup_who_never_touched_team_management_can_still_use_repos() {
        let (store, secrets) = test_state();
        let accounts = agentops_accounts::AccountStore::open_in_memory().unwrap();
        let (_, token) = signup(&accounts, "fresh@example.com");
        let teams = agentops_teams::TeamStore::open_in_memory().unwrap();
        let app = build_router(store, secrets, None, None, None, PathBuf::from("unused-docbrain-dir"), Some(accounts), Some(teams), test_indexing_store(), std::env::temp_dir(), None);

        let list = app.clone().oneshot(Request::builder().uri("/repos").header("authorization", format!("Bearer {token}")).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(list.status(), StatusCode::OK);
        assert_eq!(body_json(list).await["can_connect"], true, "a brand-new user's first touch backfills them as admin, same as /team/*");

        let connect = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/repos/connect")
                    .header("authorization", format!("Bearer {token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(json!({"repo_id": "widgets", "repo_url": "git@github.com:acme/widgets.git"}).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(connect.status(), StatusCode::CREATED);
    }

    #[tokio::test]
    async fn get_repos_reports_can_connect_based_on_the_callers_capability() {
        let (store, secrets) = test_state();
        let accounts = agentops_accounts::AccountStore::open_in_memory().unwrap();
        let (admin, admin_token) = signup(&accounts, "admin@example.com");
        let (viewer, viewer_token) = signup(&accounts, "viewer@example.com");
        let teams = agentops_teams::TeamStore::open_in_memory().unwrap();
        teams.add_member(&admin.tenant, admin.id, "admin").unwrap();
        teams.add_member(&admin.tenant, viewer.id, "viewer").unwrap();
        accounts.switch_tenant(viewer.id, &admin.tenant).unwrap();

        let app = build_router(store, secrets, None, None, None, PathBuf::from("unused-docbrain-dir"), Some(accounts), Some(teams), test_indexing_store(), std::env::temp_dir(), None);

        let admin_view = app.clone().oneshot(Request::builder().uri("/repos").header("authorization", format!("Bearer {admin_token}")).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(body_json(admin_view).await["can_connect"], true);

        let viewer_view = app.oneshot(Request::builder().uri("/repos").header("authorization", format!("Bearer {viewer_token}")).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(body_json(viewer_view).await["can_connect"], false);
    }

    #[tokio::test]
    async fn session_authed_connect_requires_the_repos_connect_capability() {
        let (store, secrets) = test_state();
        let accounts = agentops_accounts::AccountStore::open_in_memory().unwrap();
        let (admin, admin_token) = signup(&accounts, "admin@example.com");
        let (viewer, viewer_token) = signup(&accounts, "viewer@example.com");
        let teams = agentops_teams::TeamStore::open_in_memory().unwrap();
        teams.add_member(&admin.tenant, admin.id, "admin").unwrap();
        teams.add_member(&admin.tenant, viewer.id, "viewer").unwrap();
        accounts.switch_tenant(viewer.id, &admin.tenant).unwrap();

        let app = build_router(store, secrets, None, None, None, PathBuf::from("unused-docbrain-dir"), Some(accounts), Some(teams), test_indexing_store(), std::env::temp_dir(), None);

        let forbidden = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/repos/connect")
                    .header("authorization", format!("Bearer {viewer_token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(json!({"repo_id": "widgets", "repo_url": "git@github.com:acme/widgets.git"}).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(forbidden.status(), StatusCode::FORBIDDEN, "a viewer lacks repos.connect");

        let allowed = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/repos/connect")
                    .header("authorization", format!("Bearer {admin_token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(json!({"repo_id": "widgets", "repo_url": "git@github.com:acme/widgets.git"}).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(allowed.status(), StatusCode::CREATED, "an admin has repos.connect");
    }

    #[tokio::test]
    async fn session_authed_list_repos_filters_out_a_repo_with_an_explicit_no_access_override() {
        let (store, secrets) = test_state();
        let accounts = agentops_accounts::AccountStore::open_in_memory().unwrap();
        let (admin, admin_token) = signup(&accounts, "admin@example.com");
        let (dev, dev_token) = signup(&accounts, "dev@example.com");
        let teams = agentops_teams::TeamStore::open_in_memory().unwrap();
        teams.add_member(&admin.tenant, admin.id, "admin").unwrap();
        teams.add_member(&admin.tenant, dev.id, "member").unwrap();
        accounts.switch_tenant(dev.id, &admin.tenant).unwrap();
        {
            let keypair = agentops_repo_access::generate_deploy_keypair_for_repo(secrets.as_ref(), &admin.tenant, "widgets").unwrap();
            store.create_ssh_connection(&admin.tenant, "widgets", "git@github.com:acme/widgets.git", &keypair).unwrap();
        }
        // `dev` has `repos.view` (a member does by default) but is
        // explicitly denied access to this specific repo.
        teams.set_repo_override(&admin.tenant, dev.id, "widgets", false).unwrap();

        let app = build_router(store, secrets, None, None, None, PathBuf::from("unused-docbrain-dir"), Some(accounts), Some(teams), test_indexing_store(), std::env::temp_dir(), None);

        let admin_view = app.clone().oneshot(Request::builder().uri("/repos").header("authorization", format!("Bearer {admin_token}")).body(Body::empty()).unwrap()).await.unwrap();
        let body = body_json(admin_view).await;
        assert_eq!(body["connections"].as_array().unwrap().len(), 1, "admin can see it");

        let dev_view = app.oneshot(Request::builder().uri("/repos").header("authorization", format!("Bearer {dev_token}")).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(dev_view.status(), StatusCode::OK, "dev has repos.view, so the request itself succeeds");
        let body = body_json(dev_view).await;
        assert_eq!(body["connections"].as_array().unwrap().len(), 0, "the override-denied repo is omitted from the response entirely, not just hidden client-side");
    }

    #[tokio::test]
    async fn connect_then_list_round_trips_over_http() {
        let (store, secrets) = test_state();
        let app = build_router(store, secrets, None, None, None, PathBuf::from("unused-docbrain-dir"), None, None, test_indexing_store(), std::env::temp_dir(), None);

        let connect_req = Request::builder()
            .method("POST")
            .uri("/repos/connect")
            .header("content-type", "application/json")
            .body(Body::from(json!({"tenant": "acme", "repo_id": "widgets", "repo_url": "git@github.com:acme/widgets.git"}).to_string()))
            .unwrap();
        let resp = app.clone().oneshot(connect_req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = body_json(resp).await;
        let public_key = body["connection"]["public_key_openssh"].as_str().unwrap();
        assert!(public_key.starts_with("ssh-ed25519"));
        assert_eq!(body["connection"]["status"], "pending");
        // The encrypted private key must never appear anywhere in the response.
        assert!(!body.to_string().contains("PRIVATE KEY"));

        let list_req = Request::builder().uri("/repos?tenant=acme").body(Body::empty()).unwrap();
        let resp = app.oneshot(list_req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp).await;
        let connections = body["connections"].as_array().unwrap();
        assert_eq!(connections.len(), 1);
        assert_eq!(connections[0]["repo_url"], "git@github.com:acme/widgets.git");
    }

    #[tokio::test]
    async fn list_never_leaks_across_tenants() {
        let (store, secrets) = test_state();
        let app = build_router(store, secrets, None, None, None, PathBuf::from("unused-docbrain-dir"), None, None, test_indexing_store(), std::env::temp_dir(), None);

        for (tenant, repo_id, url) in [("acme", "w", "url-a"), ("globex", "g", "url-b")] {
            let req = Request::builder()
                .method("POST")
                .uri("/repos/connect")
                .header("content-type", "application/json")
                .body(Body::from(json!({"tenant": tenant, "repo_id": repo_id, "repo_url": url}).to_string()))
                .unwrap();
            app.clone().oneshot(req).await.unwrap();
        }

        let resp = app.oneshot(Request::builder().uri("/repos?tenant=acme").body(Body::empty()).unwrap()).await.unwrap();
        let body = body_json(resp).await;
        let connections = body["connections"].as_array().unwrap();
        assert_eq!(connections.len(), 1);
        assert_eq!(connections[0]["repo_url"], "url-a");
    }

    #[tokio::test]
    async fn regenerate_key_issues_a_fresh_keypair_and_resets_status_to_pending() {
        let (store, secrets) = test_state();
        let app = build_router(store, secrets, None, None, None, PathBuf::from("unused-docbrain-dir"), None, None, test_indexing_store(), std::env::temp_dir(), None);

        let connect_req = Request::builder()
            .method("POST")
            .uri("/repos/connect")
            .header("content-type", "application/json")
            .body(Body::from(json!({"tenant": "acme", "repo_id": "widgets", "repo_url": "ssh://git@127.0.0.1:1/nonexistent.git"}).to_string()))
            .unwrap();
        let resp = app.clone().oneshot(connect_req).await.unwrap();
        let original = body_json(resp).await;
        let original_key = original["connection"]["public_key_openssh"].as_str().unwrap().to_string();

        let regen_req = Request::builder().method("POST").uri("/repos/widgets/regenerate-key?tenant=acme").body(Body::empty()).unwrap();
        let resp = app.oneshot(regen_req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp).await;
        let new_key = body["connection"]["public_key_openssh"].as_str().unwrap();
        assert_ne!(new_key, original_key, "regenerating must issue a genuinely fresh keypair, not echo the old one");
        assert_eq!(body["connection"]["status"], "pending", "a regenerated key resets the connection to pending, mirroring a brand-new connect");
    }

    #[tokio::test]
    async fn verify_against_unreachable_host_marks_connection_failed() {
        let (store, secrets) = test_state();
        let app = build_router(store, secrets, None, None, None, PathBuf::from("unused-docbrain-dir"), None, None, test_indexing_store(), std::env::temp_dir(), None);

        let connect_req = Request::builder()
            .method("POST")
            .uri("/repos/connect")
            .header("content-type", "application/json")
            .body(Body::from(json!({"tenant": "acme", "repo_id": "widgets", "repo_url": "ssh://git@127.0.0.1:1/nonexistent.git"}).to_string()))
            .unwrap();
        app.clone().oneshot(connect_req).await.unwrap();

        let verify_req = Request::builder().method("POST").uri("/repos/widgets/verify?tenant=acme").body(Body::empty()).unwrap();
        let resp = app.clone().oneshot(verify_req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp).await;
        assert_eq!(body["status"], "failed");

        let list_req = Request::builder().uri("/repos?tenant=acme").body(Body::empty()).unwrap();
        let resp = app.oneshot(list_req).await.unwrap();
        let body = body_json(resp).await;
        assert!(body["connections"][0]["status"].as_str().unwrap().starts_with("failed"));
    }

    /// A real local bare git repo (no SSH server needed -- git treats a
    /// plain filesystem path as a local clone and never invokes
    /// `GIT_SSH_COMMAND` for it), used to drive the full connect -> verify
    /// -> index pipeline end to end against real `git`/`agentops_scanner`/
    /// `agentops_mcp::persist` calls, not mocks.
    fn init_bare_repo_with_one_commit() -> PathBuf {
        let bare = std::env::temp_dir().join(format!("agentops-test-bare-{}", uuid_like()));
        let scratch = std::env::temp_dir().join(format!("agentops-test-scratch-{}", uuid_like()));
        std::process::Command::new("git").args(["init", "--bare", "-q"]).arg(&bare).status().unwrap();
        std::process::Command::new("git").args(["clone", "-q"]).arg(&bare).arg(&scratch).status().unwrap();
        std::fs::write(scratch.join("README.md"), "# widgets\n").unwrap();
        std::process::Command::new("git").args(["-C"]).arg(&scratch).args(["add", "."]).status().unwrap();
        std::process::Command::new("git").args(["-C"]).arg(&scratch).args(["-c", "user.email=t@t.com", "-c", "user.name=t", "commit", "-q", "-m", "init"]).status().unwrap();
        std::process::Command::new("git").args(["-C"]).arg(&scratch).args(["push", "-q", "origin", "HEAD:refs/heads/main"]).status().unwrap();
        let _ = std::fs::remove_dir_all(&scratch);
        bare
    }

    fn uuid_like() -> String {
        let mut bytes = [0u8; 8];
        getrandom::fill(&mut bytes).unwrap();
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[tokio::test]
    async fn verify_triggers_indexing_and_the_job_reaches_succeeded_over_http() {
        let bare_repo = init_bare_repo_with_one_commit();
        let checkouts_dir = std::env::temp_dir().join(format!("agentops-test-checkouts-{}", uuid_like()));

        let (store, secrets) = test_state();
        let app = build_router(store, secrets, None, None, None, PathBuf::from("unused-docbrain-dir"), None, None, test_indexing_store(), checkouts_dir.clone(), None);

        let connect_req = Request::builder()
            .method("POST")
            .uri("/repos/connect")
            .header("content-type", "application/json")
            .body(Body::from(json!({"tenant": "acme", "repo_id": "widgets", "repo_url": bare_repo.to_string_lossy()}).to_string()))
            .unwrap();
        let resp = app.clone().oneshot(connect_req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        let verify_req = Request::builder().method("POST").uri("/repos/widgets/verify?tenant=acme").body(Body::empty()).unwrap();
        let resp = app.clone().oneshot(verify_req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let verify_body = body_json(resp).await;
        assert_eq!(verify_body["status"], "active");
        assert!(verify_body["job_id"].is_string(), "verify must kick off an indexing job, not just flip the connection to active");

        // The job runs on a detached `tokio::spawn`'d task -- poll the real
        // status endpoint (the same way the frontend will) rather than
        // reaching into orchestration internals, which are private to this
        // module.
        let mut final_status = String::new();
        for _ in 0..100 {
            let status_req = Request::builder().uri("/repos/widgets/index/status?tenant=acme").body(Body::empty()).unwrap();
            let resp = app.clone().oneshot(status_req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
            let body = body_json(resp).await;
            final_status = body["job"]["status"].as_str().unwrap().to_string();
            if final_status != "running" {
                if final_status != "succeeded" {
                    panic!("indexing job did not succeed: {}", serde_json::to_string_pretty(&body).unwrap());
                }
                assert_eq!(body["overall_percent"], 100);
                let stages = body["stages"].as_array().unwrap();
                assert_eq!(stages.len(), 9);
                for stage in stages {
                    assert_eq!(stage["status"], "done", "stage {:?} should be done: {stage:?}", stage["stage"]);
                }
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert_eq!(final_status, "succeeded", "indexing job against a real local repo must reach succeeded");

        assert!(checkouts_dir.join("acme").join("widgets").join("README.md").exists(), "the job must actually clone into its checkout dir");
        assert!(checkouts_dir.join("acme").join("widgets").join("repo-map.md").exists(), "the docgen stage must actually write repo-map.md");

        let _ = std::fs::remove_dir_all(&bare_repo);
        let _ = std::fs::remove_dir_all(&checkouts_dir);
    }

    #[tokio::test]
    async fn install_url_404s_when_no_app_is_configured() {
        let (store, secrets) = test_state();
        let app = build_router(store, secrets, None, None, None, PathBuf::from("unused-docbrain-dir"), None, None, test_indexing_store(), std::env::temp_dir(), None);
        let resp = app.oneshot(Request::builder().uri("/repos/github-app/install-url").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn install_url_returns_the_configured_slug() {
        let (store, secrets) = test_state();
        let app = build_router(store, secrets, Some("agentops-dev".to_string()), None, None, PathBuf::from("unused-docbrain-dir"), None, None, test_indexing_store(), std::env::temp_dir(), None);
        let resp = app.oneshot(Request::builder().uri("/repos/github-app/install-url").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp).await;
        assert_eq!(body["install_url"], "https://github.com/apps/agentops-dev/installations/new");
    }

    #[tokio::test]
    async fn missing_api_key_is_rejected_when_one_is_required() {
        let (store, secrets) = test_state();
        let (_, hash) = agentops_security::api_key::generate_api_key().unwrap();
        let app = build_router(store, secrets, None, Some(hash), None, PathBuf::from("unused-docbrain-dir"), None, None, test_indexing_store(), std::env::temp_dir(), None);
        let resp = app.oneshot(Request::builder().uri("/repos?tenant=acme").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn health_check_bypasses_auth_even_when_a_key_is_required() {
        let (store, secrets) = test_state();
        let (_, hash) = agentops_security::api_key::generate_api_key().unwrap();
        let app = build_router(store, secrets, None, Some(hash), None, PathBuf::from("unused-docbrain-dir"), None, None, test_indexing_store(), std::env::temp_dir(), None).merge(health_router());
        let resp = app.oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn search_returns_503_when_not_configured() {
        let (store, secrets) = test_state();
        let app = build_router(store, secrets, None, None, None, PathBuf::from("unused-docbrain-dir"), None, None, test_indexing_store(), std::env::temp_dir(), None);
        let resp = app.oneshot(Request::builder().uri("/search?path=/tmp/x&q=hello").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn tools_list_reuses_the_heavy_mcp_tool_table() {
        let (store, secrets) = test_state();
        let app = build_router(store, secrets, None, None, None, PathBuf::from("unused-docbrain-dir"), None, None, test_indexing_store(), std::env::temp_dir(), None);
        let resp = app.oneshot(Request::builder().uri("/tools").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body: Value = serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
        assert_eq!(body["tools"].as_array().unwrap().len(), agentops_heavy_mcp::list_tools().len());
    }

    #[tokio::test]
    async fn tools_call_returns_503_when_not_configured() {
        let (store, secrets) = test_state();
        let app = build_router(store, secrets, None, None, None, PathBuf::from("unused-docbrain-dir"), None, None, test_indexing_store(), std::env::temp_dir(), None);
        let req = Request::builder().method("POST").uri("/tools/semantic_search").header("content-type", "application/json").body(Body::from(json!({}).to_string())).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn search_index_returns_503_when_not_configured() {
        let (store, secrets) = test_state();
        let app = build_router(store, secrets, None, None, None, PathBuf::from("unused-docbrain-dir"), None, None, test_indexing_store(), std::env::temp_dir(), None);
        let req = Request::builder()
            .method("POST")
            .uri("/search/index")
            .header("content-type", "application/json")
            .body(Body::from(json!({"path": "/tmp/x"}).to_string()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn docs_search_returns_503_when_not_configured() {
        let (store, secrets) = test_state();
        let app = build_router(store, secrets, None, None, None, PathBuf::from("unused-docbrain-dir"), None, None, test_indexing_store(), std::env::temp_dir(), None);
        let resp = app.oneshot(Request::builder().uri("/docs/search?slug=next&q=hello").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn docs_search_index_returns_503_when_not_configured() {
        let (store, secrets) = test_state();
        let app = build_router(store, secrets, None, None, None, PathBuf::from("unused-docbrain-dir"), None, None, test_indexing_store(), std::env::temp_dir(), None);
        let req = Request::builder()
            .method("POST")
            .uri("/docs/search/index")
            .header("content-type", "application/json")
            .body(Body::from(json!({"slug": "next"}).to_string()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    /// Same real-Qdrant/real-BGE-M3 live-test convention as
    /// `search_index_then_query_finds_the_right_symbol_over_http`, for the
    /// docbrain side — and specifically checks the kind isolation guarantee:
    /// indexes both a codebrain symbol and a docbrain doc section with
    /// deliberately overlapping content, then confirms /docs/search only
    /// ever returns the doc. Adapted from main: no TenantContext, docbrain
    /// store lives at the per-tenant path this server itself computes.
    #[tokio::test]
    async fn docs_search_index_then_query_finds_the_right_doc_over_http() {
        let Ok(qdrant_url) = std::env::var("AGENTOPS_TEST_QDRANT_URL") else { return };

        let dir = tempfile::tempdir().unwrap();
        let docbrain_db_dir = dir.path().join("docbrain");
        std::fs::create_dir_all(&docbrain_db_dir).unwrap();
        let docbrain_db_path = docbrain_db_path_for_org(&docbrain_db_dir, None);
        {
            use docbrain_graph::{DocbrainStore, NewDocNode, NodeKind as DocNodeKind};
            let docs_store = docbrain_graph::SqliteDocbrainStore::open(&docbrain_db_path).unwrap();
            docs_store.add_library("next", "Next.js", None, None, Some("https://nextjs.org/docs")).unwrap();
            docs_store
                .upsert_doc_node(
                    "next",
                    NewDocNode {
                        version: "15.1.3",
                        topic: "App Router caching",
                        content: "Use the fetch cache option to control revalidation behavior.",
                        content_hash: "hash1",
                        token_count: 10,
                        embedding: None,
                        kind: DocNodeKind::Prose,
                    },
                )
                .unwrap();
        }

        let index = SemanticIndex::connect(&qdrant_url, "agentops_heavy_api_docs_test").unwrap();
        index.ensure_collection().await.unwrap();
        let search_index = Some(Arc::new(AsyncMutex::new(index)));

        let (store, secrets) = test_state();
        let app = build_router(store, secrets, None, None, search_index, docbrain_db_dir, None, None, test_indexing_store(), std::env::temp_dir(), None);

        let index_req = Request::builder()
            .method("POST")
            .uri("/docs/search/index")
            .header("content-type", "application/json")
            .body(Body::from(json!({"slug": "next"}).to_string()))
            .unwrap();
        let resp = app.clone().oneshot(index_req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp).await;
        assert_eq!(body["indexed"], 1);

        let search_req = Request::builder()
            .uri("/docs/search?slug=next&q=how+do+I+control+fetch+revalidation")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(search_req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp).await;
        let results = body["results"].as_array().unwrap();
        assert_eq!(results.len(), 1, "{body:?}");
        assert_eq!(results[0]["topic"], "App Router caching");
        assert_eq!(results[0]["version"], "15.1.3");
    }

    /// Exercises the full HTTP round trip (not just the library) against a
    /// REAL Qdrant instance and the REAL downloaded BGE-M3 model. Set
    /// AGENTOPS_TEST_QDRANT_URL to run; skipped otherwise, matching
    /// agentops-heavy-embeddings' own live-test convention. This is
    /// specifically the regression test for the `!Send` future bug fixed
    /// alongside this endpoint (a `&dyn GraphStore` held across an
    /// `.await`, which only ever showed up through axum's Handler bound,
    /// not in library-level tests) — it must go through the real router
    /// and a real, indexed repo, or it wouldn't have caught that bug.
    #[tokio::test]
    async fn search_index_then_query_finds_the_right_symbol_over_http() {
        let Ok(qdrant_url) = std::env::var("AGENTOPS_TEST_QDRANT_URL") else { return };

        let dir = tempfile::tempdir().unwrap();
        let repo_path = dir.path().to_string_lossy().to_string();
        let db_path = dir.path().join(".context").join("graph.db");
        // Nodes are stored under the canonicalized repo name, not the raw
        // path — the real bug caught via live testing against this repo
        // itself (search_index_handler now uses repo_name too).
        let repo = agentops_heavy_embeddings::repo_name(dir.path());
        {
            let graph_store = agentops_graph::SqliteGraphStore::open(&db_path).unwrap();
            graph_store
                .add_node(agentops_graph::NewNode {
                    kind: agentops_graph::NodeKind::Gotcha,
                    repo: repo.clone(),
                    path: None,
                    name: Some("ssh-host-key-pinning".into()),
                    container: None,
                    start_line: None,
                    end_line: None,
                    content: Some("Pin the SSH host key so a man-in-the-middle can't intercept the deploy key handshake.".into()),
                })
                .unwrap();
        }

        let index = SemanticIndex::connect(&qdrant_url, "agentops_heavy_api_test").unwrap();
        index.ensure_collection().await.unwrap();
        let search_index = Some(Arc::new(AsyncMutex::new(index)));

        let (store, secrets) = test_state();
        let app = build_router(store, secrets, None, None, search_index, PathBuf::from("unused-docbrain-dir"), None, None, test_indexing_store(), std::env::temp_dir(), None);

        let index_req = Request::builder()
            .method("POST")
            .uri("/search/index")
            .header("content-type", "application/json")
            .body(Body::from(json!({"path": repo_path}).to_string()))
            .unwrap();
        let resp = app.clone().oneshot(index_req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp).await;
        assert_eq!(body["indexed"], 1);

        let search_req = Request::builder()
            .uri(format!("/search?path={}&q=SSH+connection+security", urlencoding_path(&repo_path)))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(search_req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp).await;
        let results = body["results"].as_array().unwrap();
        assert_eq!(results.len(), 1, "{body:?}");
        assert_eq!(results[0]["name"], "ssh-host-key-pinning");
    }

    fn urlencoding_path(s: &str) -> String {
        s.replace('/', "%2F")
    }
}

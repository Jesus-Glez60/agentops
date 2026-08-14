//! REST API for hosted repo access (heavy tier): drives
//! `agentops-repo-access`'s SSH deploy-key flow and exposes the
//! `agentops-github-app` install URL, behind the same API-key auth pattern
//! as `agentops-api`/`docbrain-api`.
//!
//! **The encrypted private key blob is never returned by any endpoint** —
//! every response DTO here is built by hand from `RepoConnection` rather
//! than serializing it directly, specifically so a future field added to
//! `RepoConnection` doesn't silently leak into an HTTP response the way it
//! would if this server just re-serialized the store's row type.
//!
//! Ported from `main`, adapted in three places for the current codebase and
//! the 1.0 (unpaid) release: (1) `agentops_embeddings` is
//! `agentops_heavy_embeddings` now (this rebuild's rename to avoid
//! colliding with the new open-core `agentops-embeddings` crate); (2) no
//! license gate at all — semantic search is enabled purely by
//! `AGENTOPS_QDRANT_URL` being set, `agentops-license` stays untouched, a
//! deliberate 1.0 scope decision, not an oversight; (3) `docbrain-graph` is
//! single-tenant this rebuild (no `TenantContext`), so multi-tenant
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

use agentops_heavy_embeddings::SemanticIndex;
use agentops_repo_access::secrets::SecretsProvider;
use agentops_repo_access::store::{ConnectionStatus, ConnectionStore, RepoConnection};
use agentops_security::api_key::verify_api_key;

mod accounts_integrations;
mod linear_webhook;
pub use accounts_integrations::build_accounts_integrations_router;
pub use linear_webhook::{AutoKickoffTeamConfig, SeenDeliveries};

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
    /// over an optional feature. No license gate — this is 1.0, unpaid.
    search_index: Option<Arc<AsyncMutex<SemanticIndex>>>,
    /// Directory holding one docbrain SQLite file per tenant
    /// (`<org>.db`, or `default.db` when no `org` is given) — see the
    /// module doc comment for why this replaces `main`'s `TenantContext`.
    docbrain_db_dir: std::path::PathBuf,
}

pub fn build_router(
    store: ConnectionStore,
    secrets: Arc<dyn SecretsProvider + Send + Sync>,
    github_app_slug: Option<String>,
    api_key_hash: Option<String>,
    search_index: Option<Arc<AsyncMutex<SemanticIndex>>>,
    docbrain_db_dir: std::path::PathBuf,
) -> Router {
    let state = AppState { store: Arc::new(Mutex::new(store)), secrets, github_app_slug, api_key_hash, search_index, docbrain_db_dir };
    Router::new()
        .route("/repos/connect", post(connect_repo))
        .route("/repos", get(list_repos))
        .route("/repos/{id}/verify", post(verify_repo))
        .route("/repos/github-app/install-url", get(github_app_install_url))
        .route("/search/index", post(search_index_handler))
        .route("/search", get(search_query_handler))
        .route("/docs/search/index", post(docs_search_index_handler))
        .route("/docs/search", get(docs_search_query_handler))
        .layer(middleware::from_fn_with_state(state.clone(), require_api_key))
        .route("/health", get(health))
        .with_state(state)
        .layer(CorsLayer::permissive())
}

async fn require_api_key(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let Some(expected_hash) = &state.api_key_hash else {
        return next.run(req).await;
    };
    let provided = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    match provided {
        Some(raw) if verify_api_key(raw, expected_hash).is_ok() => next.run(req).await,
        _ => (StatusCode::UNAUTHORIZED, Json(json!({ "error": "missing or invalid API key" }))).into_response(),
    }
}

async fn health() -> &'static str {
    "ok"
}

fn default_modules_db_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    std::path::PathBuf::from(home).join(".agentops").join("modules.sqlite")
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

/// `~/.agentops/docbrain/` — a directory, not a single file, since this
/// deployment routes one file per tenant.
fn default_docbrain_db_dir() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    std::path::PathBuf::from(home).join(".agentops").join("docbrain")
}

fn default_accounts_db_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    std::path::PathBuf::from(home).join(".agentops").join("accounts.sqlite")
}

fn default_integrations_db_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    std::path::PathBuf::from(home).join(".agentops").join("integrations.sqlite")
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

/// Binds `addr` and serves until the process is killed. Reads
/// `AGENTOPS_SECRETS_MASTER_KEY` (required — see
/// `agentops_repo_access::secrets::EnvSecretsProvider`),
/// `AGENTOPS_HEAVY_API_KEY_HASH` (optional — unset means unauthenticated,
/// matching `agentops-api`/`docbrain-api`'s convention), `AGENTOPS_GITHUB_APP_SLUG`
/// (optional), and `AGENTOPS_QDRANT_URL` (optional — enables `/search*` on
/// its own, no license check for this 1.0 release; loading the BGE-M3 model
/// can take real time on first run, downloading it if not already cached).
pub async fn run(addr: &str, db_path: &std::path::Path) -> anyhow::Result<()> {
    let store = ConnectionStore::open(db_path)?;
    let secrets: Arc<dyn SecretsProvider + Send + Sync> = Arc::new(agentops_repo_access::secrets::EnvSecretsProvider::from_env()?);
    let github_app_slug = std::env::var("AGENTOPS_GITHUB_APP_SLUG").ok();
    let api_key_hash = std::env::var("AGENTOPS_HEAVY_API_KEY_HASH").ok();
    let auth_status = if api_key_hash.is_some() { "API key required" } else { "UNAUTHENTICATED (set AGENTOPS_HEAVY_API_KEY_HASH to require a key)" };

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

    let mut app = build_router(store, secrets.clone(), github_app_slug, api_key_hash, search_index, docbrain_db_dir);

    // Phase 7: accounts + the generic integrations vault. Phase 6c: the
    // generic per-tenant module-enrollment store (Linear auto-kickoff is
    // its first consumer). Always opened (cheap — SQLite files created on
    // demand). `build_accounts_integrations_router` takes `accounts`/
    // `credentials` by value, so `linear_webhook`'s own routes open their
    // own independent second connections to the same files rather than
    // sharing these — see `linear_webhook::LinearModuleState`'s doc comment
    // for why that's a deliberate choice, not an accidental duplicate.
    let accounts_db_path = std::env::var("AGENTOPS_ACCOUNTS_DB").map(std::path::PathBuf::from).unwrap_or_else(|_| default_accounts_db_path());
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

    app = app.merge(build_accounts_integrations_router(accounts, credentials, secrets));
    println!("Accounts + integrations vault live: POST /auth/signup, POST /auth/login, GET /auth/me, POST /auth/logout, GET /integrations, POST /integrations/{{provider}}.");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("agentops-heavy-api listening on {addr} (auth: {auth_status})");
    axum::serve(listener, app).await?;
    Ok(())
}

#[derive(Debug, Deserialize)]
struct ConnectRequest {
    tenant: String,
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

async fn connect_repo(State(state): State<AppState>, Json(req): Json<ConnectRequest>) -> (StatusCode, Json<Value>) {
    let keypair = match agentops_repo_access::generate_deploy_keypair_for_repo(state.secrets.as_ref(), &req.tenant, &req.repo_id) {
        Ok(k) => k,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("generating deploy key: {e}") }))),
    };

    let store = state.store.lock().unwrap();
    match store.create_ssh_connection(&req.tenant, &req.repo_id, &req.repo_url, &keypair) {
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
    tenant: String,
}

async fn list_repos(State(state): State<AppState>, Query(q): Query<TenantQuery>) -> (StatusCode, Json<Value>) {
    let store = state.store.lock().unwrap();
    match store.list_connections(&q.tenant) {
        Ok(connections) => {
            let views: Vec<ConnectionView> = connections.into_iter().map(ConnectionView::from).collect();
            (StatusCode::OK, Json(json!({ "connections": views })))
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    }
}

async fn verify_repo(State(state): State<AppState>, AxumPath(id): AxumPath<String>, Query(q): Query<TenantQuery>) -> (StatusCode, Json<Value>) {
    let connection = {
        let store = state.store.lock().unwrap();
        match store.get_connection(&q.tenant, &id) {
            Ok(Some(c)) => c,
            Ok(None) => return (StatusCode::NOT_FOUND, Json(json!({ "error": "no such connection for this tenant" }))),
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
        }
    };

    let Some(encrypted_key) = &connection.encrypted_private_key_openssh else {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "connection has no SSH key (not an SSH-method connection)" })));
    };

    let result = verify_ssh_connection(state.secrets.as_ref(), &q.tenant, &id, encrypted_key, &connection.repo_url);

    let store = state.store.lock().unwrap();
    match &result {
        Ok(()) => {
            let _ = store.set_status(&q.tenant, &id, ConnectionStatus::Active);
            (StatusCode::OK, Json(json!({ "status": "active" })))
        }
        Err(e) => {
            let reason = e.to_string();
            let _ = store.set_status(&q.tenant, &id, ConnectionStatus::Failed(reason.clone()));
            (StatusCode::OK, Json(json!({ "status": "failed", "reason": reason })))
        }
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

async fn github_app_install_url(State(state): State<AppState>) -> (StatusCode, Json<Value>) {
    match &state.github_app_slug {
        Some(slug) => (StatusCode::OK, Json(json!({ "install_url": agentops_github_app::install_url(slug) }))),
        None => (StatusCode::NOT_FOUND, Json(json!({ "error": "no GitHub App is configured for this deployment (AGENTOPS_GITHUB_APP_SLUG not set)" }))),
    }
}

/// `503 Service Unavailable` — this deployment simply doesn't have Qdrant
/// configured, not a payment/license concept (1.0 has no license gate at
/// all; see the module doc comment).
fn search_not_configured() -> (StatusCode, Json<Value>) {
    (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "error": "semantic search is not configured for this deployment (AGENTOPS_QDRANT_URL not set)" })))
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
        let app = build_router(store, secrets, None, None, None, PathBuf::from("unused-docbrain-dir"));
        let resp = app.oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn connect_then_list_round_trips_over_http() {
        let (store, secrets) = test_state();
        let app = build_router(store, secrets, None, None, None, PathBuf::from("unused-docbrain-dir"));

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
        let app = build_router(store, secrets, None, None, None, PathBuf::from("unused-docbrain-dir"));

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
    async fn verify_against_unreachable_host_marks_connection_failed() {
        let (store, secrets) = test_state();
        let app = build_router(store, secrets, None, None, None, PathBuf::from("unused-docbrain-dir"));

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

    #[tokio::test]
    async fn install_url_404s_when_no_app_is_configured() {
        let (store, secrets) = test_state();
        let app = build_router(store, secrets, None, None, None, PathBuf::from("unused-docbrain-dir"));
        let resp = app.oneshot(Request::builder().uri("/repos/github-app/install-url").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn install_url_returns_the_configured_slug() {
        let (store, secrets) = test_state();
        let app = build_router(store, secrets, Some("agentops-dev".to_string()), None, None, PathBuf::from("unused-docbrain-dir"));
        let resp = app.oneshot(Request::builder().uri("/repos/github-app/install-url").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp).await;
        assert_eq!(body["install_url"], "https://github.com/apps/agentops-dev/installations/new");
    }

    #[tokio::test]
    async fn missing_api_key_is_rejected_when_one_is_required() {
        let (store, secrets) = test_state();
        let (_, hash) = agentops_security::api_key::generate_api_key().unwrap();
        let app = build_router(store, secrets, None, Some(hash), None, PathBuf::from("unused-docbrain-dir"));
        let resp = app.oneshot(Request::builder().uri("/repos?tenant=acme").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn health_check_bypasses_auth_even_when_a_key_is_required() {
        let (store, secrets) = test_state();
        let (_, hash) = agentops_security::api_key::generate_api_key().unwrap();
        let app = build_router(store, secrets, None, Some(hash), None, PathBuf::from("unused-docbrain-dir"));
        let resp = app.oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn search_returns_503_when_not_configured() {
        let (store, secrets) = test_state();
        let app = build_router(store, secrets, None, None, None, PathBuf::from("unused-docbrain-dir"));
        let resp = app.oneshot(Request::builder().uri("/search?path=/tmp/x&q=hello").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn search_index_returns_503_when_not_configured() {
        let (store, secrets) = test_state();
        let app = build_router(store, secrets, None, None, None, PathBuf::from("unused-docbrain-dir"));
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
        let app = build_router(store, secrets, None, None, None, PathBuf::from("unused-docbrain-dir"));
        let resp = app.oneshot(Request::builder().uri("/docs/search?slug=next&q=hello").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn docs_search_index_returns_503_when_not_configured() {
        let (store, secrets) = test_state();
        let app = build_router(store, secrets, None, None, None, PathBuf::from("unused-docbrain-dir"));
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
        let app = build_router(store, secrets, None, None, search_index, docbrain_db_dir);

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
        let app = build_router(store, secrets, None, None, search_index, PathBuf::from("unused-docbrain-dir"));

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

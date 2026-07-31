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

use agentops_embeddings::SemanticIndex;
use agentops_repo_access::secrets::SecretsProvider;
use agentops_repo_access::store::{ConnectionStatus, ConnectionStore, RepoConnection};
use agentops_security::api_key::verify_api_key;

#[derive(Clone)]
pub struct AppState {
    store: Arc<Mutex<ConnectionStore>>,
    secrets: Arc<dyn SecretsProvider + Send + Sync>,
    /// GitHub App URL slug, if a GitHub App has actually been registered
    /// for this deployment — `GET /repos/github-app/install-url` 404s with
    /// a clear message when this is `None` rather than returning a bogus URL.
    github_app_slug: Option<String>,
    api_key_hash: Option<String>,
    /// `None` when Qdrant isn't configured or no valid license was found —
    /// `/search*` routes return `402 Payment Required` with a clear message
    /// rather than the server refusing to start at all over a gated feature.
    search_index: Option<Arc<AsyncMutex<SemanticIndex>>>,
}

pub fn build_router(
    store: ConnectionStore,
    secrets: Arc<dyn SecretsProvider + Send + Sync>,
    github_app_slug: Option<String>,
    api_key_hash: Option<String>,
    search_index: Option<Arc<AsyncMutex<SemanticIndex>>>,
) -> Router {
    let state = AppState { store: Arc::new(Mutex::new(store)), secrets, github_app_slug, api_key_hash, search_index };
    Router::new()
        .route("/repos/connect", post(connect_repo))
        .route("/repos", get(list_repos))
        .route("/repos/{id}/verify", post(verify_repo))
        .route("/repos/github-app/install-url", get(github_app_install_url))
        .route("/search/index", post(search_index_handler))
        .route("/search", get(search_query_handler))
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

/// Binds `addr` and serves until the process is killed. Reads
/// `AGENTOPS_SECRETS_MASTER_KEY` (required — see
/// `agentops_repo_access::secrets::EnvSecretsProvider`),
/// `AGENTOPS_HEAVY_API_KEY_HASH` (optional — unset means unauthenticated,
/// matching `agentops-api`/`docbrain-api`'s convention), `AGENTOPS_GITHUB_APP_SLUG`
/// (optional), and `AGENTOPS_QDRANT_URL` + `AGENTOPS_LICENSE_KEY` (both
/// required together to enable `/search*` — semantic search is a paid-tier
/// capability, gated on a valid license the same way any other paid feature
/// should be: check once at startup, degrade the one feature rather than
/// refusing to start the whole server over it. Loading the BGE-M3 model can
/// take real time on first run, downloading it if not already cached).
pub async fn run(addr: &str, db_path: &std::path::Path) -> anyhow::Result<()> {
    let store = ConnectionStore::open(db_path)?;
    let secrets: Arc<dyn SecretsProvider + Send + Sync> = Arc::new(agentops_repo_access::secrets::EnvSecretsProvider::from_env()?);
    let github_app_slug = std::env::var("AGENTOPS_GITHUB_APP_SLUG").ok();
    let api_key_hash = std::env::var("AGENTOPS_HEAVY_API_KEY_HASH").ok();
    let auth_status = if api_key_hash.is_some() { "API key required" } else { "UNAUTHENTICATED (set AGENTOPS_HEAVY_API_KEY_HASH to require a key)" };

    let search_index = match std::env::var("AGENTOPS_QDRANT_URL") {
        Ok(qdrant_url) => match agentops_license::require_valid_license_from_env() {
            Ok(claims) => {
                println!("Licensed to {:?} (tier: {:?}) — loading BGE-M3 (downloads on first run, cached after)...", claims.licensee, claims.tier);
                let collection = std::env::var("AGENTOPS_QDRANT_COLLECTION").unwrap_or_else(|_| "agentops_semantic".to_string());
                let index = SemanticIndex::connect(&qdrant_url, &collection)?;
                index.ensure_collection().await?;
                println!("Semantic search ready (collection {collection:?} at {qdrant_url}).");
                Some(Arc::new(AsyncMutex::new(index)))
            }
            Err(e) => {
                println!("AGENTOPS_QDRANT_URL is set but no valid license was found ({e}) — /search* routes disabled. Semantic search is a paid-tier feature.");
                None
            }
        },
        Err(_) => {
            println!("AGENTOPS_QDRANT_URL not set — /search* routes disabled for this deployment.");
            None
        }
    };

    let app = build_router(store, secrets, github_app_slug, api_key_hash, search_index);
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

/// `402 Payment Required` — the semantically correct status for "this
/// endpoint exists, but the deployment isn't licensed for it," distinct
/// from `404` (doesn't exist) or `401`/`403` (identity/permission problem).
fn search_not_licensed() -> (StatusCode, Json<Value>) {
    (
        StatusCode::PAYMENT_REQUIRED,
        Json(json!({ "error": "semantic search is a paid-tier feature — this deployment has no valid license configured (AGENTOPS_LICENSE_KEY / AGENTOPS_QDRANT_URL)" })),
    )
}

#[derive(Debug, Deserialize)]
struct SearchIndexRequest {
    path: String,
}

/// Embeds and indexes every Symbol/Gotcha/Decision node from a scanned
/// repo's graph store — call this after `agentops install`, before
/// `/search` will return anything useful for that repo.
async fn search_index_handler(State(state): State<AppState>, Json(req): Json<SearchIndexRequest>) -> (StatusCode, Json<Value>) {
    let Some(search_index) = &state.search_index else { return search_not_licensed() };

    // Collecting items is synchronous and fully finishes — including
    // dropping `store` — before the `.await` below. `&dyn GraphStore` isn't
    // provably `Sync` (SQLite connections aren't), so it must never be held
    // across an await; see collect_index_items's doc comment.
    let items = {
        let db_path = std::path::Path::new(&req.path).join(".context").join("graph.db");
        let store = match agentops_graph::SqliteGraphStore::open(&db_path) {
            Ok(s) => s,
            Err(e) => return (StatusCode::NOT_FOUND, Json(json!({ "error": format!("opening graph store at {}: {e}", db_path.display()) }))),
        };
        match agentops_embeddings::collect_index_items(&store, &req.path) {
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
    let Some(search_index) = &state.search_index else { return search_not_licensed() };

    let mut index = search_index.lock().await;
    match index.search(&q.q, q.top_k, Some(&q.path)).await {
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

#[cfg(test)]
mod tests {
    use super::*;
    use agentops_graph::GraphStore as _;
    use agentops_repo_access::secrets::EnvSecretsProvider;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
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

    #[tokio::test]
    async fn health_check_ok() {
        let (store, secrets) = test_state();
        let app = build_router(store, secrets, None, None, None);
        let resp = app.oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn connect_then_list_round_trips_over_http() {
        let (store, secrets) = test_state();
        let app = build_router(store, secrets, None, None, None);

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
        let app = build_router(store, secrets, None, None, None);

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
        let app = build_router(store, secrets, None, None, None);

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
        let app = build_router(store, secrets, None, None, None);
        let resp = app.oneshot(Request::builder().uri("/repos/github-app/install-url").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn install_url_returns_the_configured_slug() {
        let (store, secrets) = test_state();
        let app = build_router(store, secrets, Some("agentops-dev".to_string()), None, None);
        let resp = app.oneshot(Request::builder().uri("/repos/github-app/install-url").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp).await;
        assert_eq!(body["install_url"], "https://github.com/apps/agentops-dev/installations/new");
    }

    #[tokio::test]
    async fn missing_api_key_is_rejected_when_one_is_required() {
        let (store, secrets) = test_state();
        let (_, hash) = agentops_security::api_key::generate_api_key().unwrap();
        let app = build_router(store, secrets, None, Some(hash), None);
        let resp = app.oneshot(Request::builder().uri("/repos?tenant=acme").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn health_check_bypasses_auth_even_when_a_key_is_required() {
        let (store, secrets) = test_state();
        let (_, hash) = agentops_security::api_key::generate_api_key().unwrap();
        let app = build_router(store, secrets, None, Some(hash), None);
        let resp = app.oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn search_returns_402_when_not_licensed_or_configured() {
        let (store, secrets) = test_state();
        let app = build_router(store, secrets, None, None, None);
        let resp = app.oneshot(Request::builder().uri("/search?path=/tmp/x&q=hello").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::PAYMENT_REQUIRED);
    }

    #[tokio::test]
    async fn search_index_returns_402_when_not_licensed_or_configured() {
        let (store, secrets) = test_state();
        let app = build_router(store, secrets, None, None, None);
        let req = Request::builder()
            .method("POST")
            .uri("/search/index")
            .header("content-type", "application/json")
            .body(Body::from(json!({"path": "/tmp/x"}).to_string()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::PAYMENT_REQUIRED);
    }

    /// Exercises the full HTTP round trip (not just the library) against a
    /// REAL Qdrant instance and the REAL downloaded BGE-M3 model. Set
    /// AGENTOPS_TEST_QDRANT_URL to run; skipped otherwise, matching
    /// agentops-embeddings' own live-test convention. This is specifically
    /// the regression test for the `!Send` future bug fixed alongside this
    /// endpoint (a `&dyn GraphStore` held across an `.await`, which only
    /// ever showed up through axum's Handler bound, not in library-level
    /// tests) — it must go through the real router and a real, indexed
    /// repo, or it wouldn't have caught that bug.
    #[tokio::test]
    async fn search_index_then_query_finds_the_right_symbol_over_http() {
        let Ok(qdrant_url) = std::env::var("AGENTOPS_TEST_QDRANT_URL") else { return };

        let dir = tempfile::tempdir().unwrap();
        let repo_path = dir.path().to_string_lossy().to_string();
        let db_path = dir.path().join(".context").join("graph.db");
        {
            let graph_store = agentops_graph::SqliteGraphStore::open(&db_path).unwrap();
            graph_store
                .add_node(agentops_graph::NewNode {
                    kind: agentops_graph::NodeKind::Gotcha,
                    repo: repo_path.clone(),
                    path: None,
                    name: Some("ssh-host-key-pinning".into()),
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
        let app = build_router(store, secrets, None, None, search_index);

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


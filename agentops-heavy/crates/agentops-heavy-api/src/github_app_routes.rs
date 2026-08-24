//! GitHub App installation flow + webhook receiver: the "Continue with
//! GitHub App" side of the "Connect repository" wizard.
//!
//! Three session-authenticated routes (mounted on the main, layered
//! `Router` in `build_router` -- they get `require_api_key_or_session` for
//! free the same way every other `/repos/*` route does) plus one public,
//! unauthenticated route (`POST /webhooks/github`, HMAC-verified instead,
//! merged in *after* the layer the same way `linear_webhook`'s route is).
//!
//! **No runtime "ensure webhook registered" step, unlike Linear's Phase 6
//! flow.** A GitHub App has exactly one webhook URL, configured once in the
//! App's own GitHub.com settings when it's registered -- not per-tenant or
//! per-installation. `AGENTOPS_GITHUB_WEBHOOK_SECRET` is something the
//! operator pastes into that same App settings page, not something this
//! code pushes there.
//!
//! **Installation tokens are never persisted.** Every route that needs one
//! (listing an installation's repos, cloning during an indexing job) mints
//! a fresh one on demand via `agentops_github_app::get_installation_token`
//! and discards it once used -- short-lived by design, matching this
//! crate's existing "don't custody a secret longer than the moment it's
//! needed" posture for SSH deploy keys.

use std::sync::{Arc, Mutex};

use axum::extract::{Path as AxumPath, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::post;
use axum::{Json, Router};
use hmac::{Hmac, KeyInit, Mac};
use serde::Deserialize;
use serde_json::json;
use sha2::Sha256;

use agentops_accounts::User;
use agentops_repo_access::indexing_store::JobKind;
use agentops_repo_access::store::{ConnectionMethod, ConnectionStatus, ConnectionStore};

use crate::indexing::IndexingDeps;
use crate::{require_session_capability, resolve_tenant, AppState};

const DELIVERY_HEADER: &str = "x-github-delivery";
const SIGNATURE_HEADER: &str = "x-hub-signature-256";

#[derive(Clone)]
pub struct GitHubAppConfig {
    pub app_id: u64,
    pub private_key_pem: String,
    pub webhook_secret: String,
}

fn service_unavailable() -> Response {
    (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "error": "no GitHub App is configured for this deployment (AGENTOPS_GITHUB_APP_ID/PRIVATE_KEY/WEBHOOK_SECRET not all set)" }))).into_response()
}

pub(crate) async fn fresh_installation_token(config: &GitHubAppConfig, installation_id: u64) -> anyhow::Result<String> {
    let app_jwt = agentops_github_app::generate_app_jwt(config.app_id, &config.private_key_pem)?;
    let client = reqwest::Client::new();
    let token = agentops_github_app::get_installation_token(&client, &app_jwt, installation_id).await?;
    Ok(token.token)
}

#[derive(Debug, Deserialize)]
pub struct CallbackQuery {
    installation_id: u64,
    #[serde(default)]
    #[allow(dead_code)]
    setup_action: Option<String>,
}

/// `GET /repos/github-app/callback` -- GitHub redirects the browser here
/// right after an admin installs (or updates) the App on their org.
/// Confirms the installation is real by actually exchanging a token for it
/// (not just trusting the query param), persists it, and hands the browser
/// off to the wizard's repo-selection step.
pub async fn github_app_callback(State(state): State<AppState>, user: Option<axum::Extension<User>>, Query(q): Query<CallbackQuery>) -> Response {
    let tenant = match resolve_tenant(&user, None) {
        Ok(t) => t,
        Err(e) => return e.into_response(),
    };
    if let Err(e) = require_session_capability(&state, &user, &tenant, agentops_teams::CAP_REPOS_CONNECT) {
        return e.into_response();
    }
    let Some(config) = &state.github_app_config else { return service_unavailable() };

    let token = match fresh_installation_token(config, q.installation_id).await {
        Ok(t) => t,
        Err(e) => return (StatusCode::BAD_GATEWAY, Json(json!({ "error": format!("confirming installation: {e}") }))).into_response(),
    };
    let client = reqwest::Client::new();
    let repos = match agentops_github_app::list_installation_repos(&client, &token).await {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_GATEWAY, Json(json!({ "error": format!("listing installation repos: {e}") }))).into_response(),
    };
    let account_login = repos.first().and_then(|r| r.full_name.split('/').next()).unwrap_or("unknown").to_string();

    {
        let store = state.indexing.lock().unwrap();
        if let Err(e) = store.create_installation(&tenant, &q.installation_id.to_string(), &account_login) {
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response();
        }
    }

    Redirect::to(&format!("/repositories/connect/github-app/select?installation_id={}", q.installation_id)).into_response()
}

#[derive(Debug, Deserialize)]
pub struct InstallationQuery {
    #[serde(default)]
    tenant: Option<String>,
}

/// `GET /repos/github-app/installations/{id}/repos` -- lists every repo the
/// installation can access, for the wizard's repo-selection checklist.
pub async fn list_installation_repos_handler(State(state): State<AppState>, user: Option<axum::Extension<User>>, AxumPath(id): AxumPath<String>, Query(q): Query<InstallationQuery>) -> Response {
    let tenant = match resolve_tenant(&user, q.tenant.as_deref()) {
        Ok(t) => t,
        Err(e) => return e.into_response(),
    };
    if let Err(e) = require_session_capability(&state, &user, &tenant, agentops_teams::CAP_REPOS_VIEW) {
        return e.into_response();
    }
    let Some(config) = &state.github_app_config else { return service_unavailable() };

    let owns_installation = { state.indexing.lock().unwrap().get_installation(&tenant, &id) };
    match owns_installation {
        Ok(Some(_)) => {}
        Ok(None) => return (StatusCode::NOT_FOUND, Json(json!({ "error": "no such installation for this tenant" }))).into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }

    let installation_id: u64 = match id.parse() {
        Ok(v) => v,
        Err(_) => return (StatusCode::BAD_REQUEST, Json(json!({ "error": "installation id must be numeric" }))).into_response(),
    };
    let token = match fresh_installation_token(config, installation_id).await {
        Ok(t) => t,
        Err(e) => return (StatusCode::BAD_GATEWAY, Json(json!({ "error": format!("minting installation token: {e}") }))).into_response(),
    };
    let client = reqwest::Client::new();
    match agentops_github_app::list_installation_repos(&client, &token).await {
        Ok(repos) => (StatusCode::OK, Json(json!({ "repositories": repos }))).into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct ConnectFromInstallationRequest {
    #[serde(default)]
    tenant: Option<String>,
    repo_full_names: Vec<String>,
}

/// `POST /repos/github-app/installations/{id}/connect` -- creates one
/// `RepoConnection` per selected repo and kicks off an indexing job for
/// each. A GitHub App connection is trusted immediately (`Active` from the
/// moment it's created, see `create_github_app_connection`'s doc comment)
/// -- there's no separate verify step the way SSH needs.
pub async fn connect_from_installation(State(state): State<AppState>, user: Option<axum::Extension<User>>, AxumPath(id): AxumPath<String>, Json(req): Json<ConnectFromInstallationRequest>) -> Response {
    let tenant = match resolve_tenant(&user, req.tenant.as_deref()) {
        Ok(t) => t,
        Err(e) => return e.into_response(),
    };
    if let Err(e) = require_session_capability(&state, &user, &tenant, agentops_teams::CAP_REPOS_CONNECT) {
        return e.into_response();
    }

    let owns_installation = { state.indexing.lock().unwrap().get_installation(&tenant, &id) };
    match owns_installation {
        Ok(Some(_)) => {}
        Ok(None) => return (StatusCode::NOT_FOUND, Json(json!({ "error": "no such installation for this tenant" }))).into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }

    let mut created = Vec::new();
    for full_name in &req.repo_full_names {
        let connection_id = full_name.replace('/', "--");
        let repo_url = format!("https://github.com/{full_name}.git");
        let connection = {
            let store = state.store.lock().unwrap();
            match store.create_github_app_connection(&tenant, &connection_id, &repo_url, &id) {
                Ok(c) => c,
                Err(e) => return (StatusCode::CONFLICT, Json(json!({ "error": format!("{full_name}: {e}") }))).into_response(),
            }
        };
        let job_id = match crate::indexing::create_and_spawn_job(&state.indexing_deps(), tenant.clone(), connection.clone(), JobKind::Initial) {
            Ok(id) => Some(id),
            Err(e) => {
                eprintln!("connect_from_installation: connection {tenant}/{connection_id} created but failed to start indexing job: {e}");
                None
            }
        };
        created.push(json!({ "connection": crate::ConnectionView::from(connection), "job_id": job_id }));
    }

    (StatusCode::CREATED, Json(json!({ "connections": created }))).into_response()
}

/// State for the public, unauthenticated `/webhooks/github` route --
/// deliberately its own independent second connections to the same
/// underlying SQLite files `run()` already opened for `build_router`'s
/// `AppState`, matching `LinearModuleState`'s established precedent
/// (`linear_webhook.rs`'s own doc comment) rather than trying to thread
/// `AppState`'s handles across an unrelated router-construction path.
struct GithubWebhookState {
    deps: IndexingDeps,
    config: GitHubAppConfig,
    seen: crate::SeenDeliveries,
}

fn verify_github_signature(secret: &str, raw_body: &[u8], header_value: &str) -> bool {
    let Some(hex) = header_value.strip_prefix("sha256=") else { return false };
    let Ok(expected) = decode_hex(hex) else { return false };
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret.as_bytes()) else { return false };
    mac.update(raw_body);
    mac.verify_slice(&expected).is_ok()
}

fn decode_hex(s: &str) -> anyhow::Result<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        anyhow::bail!("hex string has odd length");
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(anyhow::Error::from)).collect()
}

async fn github_webhook_handler(State(state): State<Arc<GithubWebhookState>>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    let Some(signature) = headers.get(SIGNATURE_HEADER).and_then(|v| v.to_str().ok()) else {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "missing X-Hub-Signature-256 header" }))).into_response();
    };
    if !verify_github_signature(&state.config.webhook_secret, &body, signature) {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "invalid signature" }))).into_response();
    }
    if let Some(delivery_id) = headers.get(DELIVERY_HEADER).and_then(|v| v.to_str().ok()) {
        if !state.seen.record_if_new(delivery_id) {
            return (StatusCode::OK, Json(json!({ "dispatched": false, "reason": "duplicate delivery" }))).into_response();
        }
    }

    let Some(event) = headers.get("x-github-event").and_then(|v| v.to_str().ok()).map(str::to_string) else {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "missing X-GitHub-Event header" }))).into_response();
    };
    let payload: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return (StatusCode::BAD_REQUEST, Json(json!({ "error": "invalid JSON body" }))).into_response(),
    };

    let Some(installation_id) = payload.pointer("/installation/id").and_then(|v| v.as_u64()) else {
        // Not every GitHub event carries an installation (e.g. a
        // marketplace/ping event) -- nothing to dispatch, acknowledge and
        // move on rather than erroring.
        return (StatusCode::OK, Json(json!({ "dispatched": false, "reason": "no installation id in payload" }))).into_response();
    };
    let tenant = { state.deps.indexing.lock().unwrap().tenant_for_installation(&installation_id.to_string()) };
    let tenant = match tenant {
        Ok(Some(t)) => t,
        Ok(None) => return (StatusCode::OK, Json(json!({ "dispatched": false, "reason": "installation not recognized by this deployment" }))).into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    };

    match event.as_str() {
        "installation" if payload.get("action").and_then(|v| v.as_str()) == Some("deleted") => {
            mark_installation_connections_failed(&state.deps.connections, &tenant, &installation_id.to_string(), "GitHub App installation removed");
            (StatusCode::OK, Json(json!({ "dispatched": true, "reason": "installation removed, connections marked failed" }))).into_response()
        }
        "installation_repositories" if payload.get("action").and_then(|v| v.as_str()) == Some("removed") => {
            let removed_full_names: Vec<String> = payload
                .pointer("/repositories_removed")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|r| r.get("full_name").and_then(|v| v.as_str()).map(str::to_string)).collect())
                .unwrap_or_default();
            let connections = state.deps.connections.lock().unwrap();
            for full_name in &removed_full_names {
                let connection_id = full_name.replace('/', "--");
                let _ = connections.set_status(&tenant, &connection_id, ConnectionStatus::Failed("repository access removed from GitHub App installation".to_string()));
            }
            (StatusCode::OK, Json(json!({ "dispatched": true, "reason": "removed repos marked failed" }))).into_response()
        }
        "push" => {
            let Some(full_name) = payload.pointer("/repository/full_name").and_then(|v| v.as_str()) else {
                return (StatusCode::OK, Json(json!({ "dispatched": false, "reason": "push payload missing repository.full_name" }))).into_response();
            };
            let connection_id = full_name.replace('/', "--");
            let connection = { state.deps.connections.lock().unwrap().get_connection(&tenant, &connection_id) };
            match connection {
                Ok(Some(c)) if c.method == ConnectionMethod::GitHubApp => match crate::indexing::create_and_spawn_job(&state.deps, tenant, c, JobKind::Reindex) {
                    Ok(job_id) => (StatusCode::OK, Json(json!({ "dispatched": true, "job_id": job_id }))).into_response(),
                    Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
                },
                Ok(_) => (StatusCode::OK, Json(json!({ "dispatched": false, "reason": "no matching GitHub App connection for this repo" }))).into_response(),
                Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
            }
        }
        other => (StatusCode::OK, Json(json!({ "dispatched": false, "reason": format!("unhandled event type {other:?}") }))).into_response(),
    }
}

fn mark_installation_connections_failed(connections: &Arc<Mutex<ConnectionStore>>, tenant: &str, installation_id: &str, reason: &str) {
    let store = connections.lock().unwrap();
    if let Ok(rows) = store.connections_for_installation(tenant, installation_id) {
        for c in rows {
            let _ = store.set_status(tenant, &c.id, ConnectionStatus::Failed(reason.to_string()));
        }
    }
}

/// Merges the public `/webhooks/github` route into `app` -- mounted
/// *outside* the session/API-key auth layer, same as `/webhooks/linear`,
/// since GitHub itself is the caller and can't present a session token.
/// A `None` config means no GitHub App is registered for this deployment;
/// the route still mounts but always 503s, rather than being absent
/// entirely (a consistent 503 is easier to diagnose than a 404 that could
/// also mean "wrong URL"). `deps` are independent store connections the
/// caller (`run()`) opens itself -- see `GithubWebhookState`'s doc comment
/// for why this handler doesn't share `build_router`'s `AppState`.
pub fn merge_github_webhook_route(app: Router, deps: IndexingDeps, config: Option<GitHubAppConfig>) -> Router {
    let Some(config) = config else {
        let router = Router::new().route("/webhooks/github", post(|| async { service_unavailable() }));
        return app.merge(router);
    };
    let state = Arc::new(GithubWebhookState { deps, config, seen: crate::SeenDeliveries::new(1000) });
    let router = Router::new().route("/webhooks/github", post(github_webhook_handler)).with_state(state);
    app.merge(router)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentops_repo_access::indexing_store::IndexingStore;
    use agentops_repo_access::secrets::EnvSecretsProvider;
    use agentops_repo_access::store::ConnectionStore;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn test_deps() -> IndexingDeps {
        IndexingDeps {
            indexing: Arc::new(Mutex::new(IndexingStore::open_in_memory().unwrap())),
            connections: Arc::new(Mutex::new(ConnectionStore::open_in_memory().unwrap())),
            secrets: Arc::new(EnvSecretsProvider::from_hex(&"33".repeat(32)).unwrap()),
            search_index: None,
            repo_checkouts_dir: std::env::temp_dir(),
            github_app_config: None,
        }
    }

    fn sign(secret: &str, body: &[u8]) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        format!("sha256={}", mac.finalize().into_bytes().iter().map(|b| format!("{b:02x}")).collect::<String>())
    }

    async fn body_json(resp: axum::response::Response) -> serde_json::Value {
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn webhook_503s_when_no_github_app_is_configured() {
        let router = merge_github_webhook_route(Router::new(), test_deps(), None);
        let resp = router.oneshot(Request::builder().method("POST").uri("/webhooks/github").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn webhook_rejects_a_missing_signature() {
        let config = GitHubAppConfig { app_id: 1, private_key_pem: String::new(), webhook_secret: "wh-secret".to_string() };
        let router = merge_github_webhook_route(Router::new(), test_deps(), Some(config));
        let resp = router.oneshot(Request::builder().method("POST").uri("/webhooks/github").body(Body::from("{}")).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn webhook_rejects_a_tampered_body_against_a_correctly_signed_header() {
        let secret = "wh-secret";
        let config = GitHubAppConfig { app_id: 1, private_key_pem: String::new(), webhook_secret: secret.to_string() };
        let router = merge_github_webhook_route(Router::new(), test_deps(), Some(config));

        let signature = sign(secret, b"{\"a\":1}");
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/webhooks/github")
                    .header(SIGNATURE_HEADER, signature)
                    .header("x-github-event", "push")
                    .body(Body::from("{\"a\":2}")) // tampered after signing
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn a_correctly_signed_push_event_triggers_a_reindex_job_for_the_matching_connection() {
        let secret = "wh-secret";
        let deps = test_deps();

        // Seed: an installation owned by "acme", and a GitHub-App-method
        // connection for a repo under it -- exactly what
        // `connect_from_installation` would have created.
        {
            let indexing = deps.indexing.lock().unwrap();
            indexing.create_installation("acme", "555", "acme-corp").unwrap();
        }
        {
            let connections = deps.connections.lock().unwrap();
            connections.create_github_app_connection("acme", "acme-corp--widgets", "https://github.com/acme-corp/widgets.git", "555").unwrap();
        }

        let config = GitHubAppConfig { app_id: 1, private_key_pem: String::new(), webhook_secret: secret.to_string() };
        let router = merge_github_webhook_route(Router::new(), deps.clone(), Some(config));

        let body = serde_json::json!({
            "installation": { "id": 555 },
            "repository": { "full_name": "acme-corp/widgets" },
        })
        .to_string();
        let signature = sign(secret, body.as_bytes());

        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/webhooks/github")
                    .header(SIGNATURE_HEADER, signature)
                    .header("x-github-event", "push")
                    .header(DELIVERY_HEADER, "delivery-1")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json = body_json(resp).await;
        assert_eq!(json["dispatched"], true);
        assert!(json["job_id"].is_string(), "a push event for a known GitHub App connection must spawn a reindex job: {json:?}");

        // A job row must have actually been created (kind=reindex), even
        // though the job itself will go on to fail at the clone stage in
        // this test (no real network access to github.com, no real
        // installation token) -- that failure is expected and out of
        // scope for this test, which only asserts dispatch happened.
        let job = { deps.indexing.lock().unwrap().latest_job_for_connection("acme", "acme-corp--widgets").unwrap() };
        assert!(job.is_some());
        assert_eq!(job.unwrap().kind, agentops_repo_access::indexing_store::JobKind::Reindex);
    }

    #[tokio::test]
    async fn a_duplicate_delivery_id_is_not_dispatched_twice() {
        let secret = "wh-secret";
        let deps = test_deps();
        let config = GitHubAppConfig { app_id: 1, private_key_pem: String::new(), webhook_secret: secret.to_string() };
        let router = merge_github_webhook_route(Router::new(), deps, Some(config));

        let body = serde_json::json!({ "installation": { "id": 999 } }).to_string();
        let signature = sign(secret, body.as_bytes());
        let make_req = || {
            Request::builder()
                .method("POST")
                .uri("/webhooks/github")
                .header(SIGNATURE_HEADER, signature.clone())
                .header("x-github-event", "ping")
                .header(DELIVERY_HEADER, "delivery-dup")
                .body(Body::from(body.clone()))
                .unwrap()
        };

        let first = router.clone().oneshot(make_req()).await.unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let second = router.oneshot(make_req()).await.unwrap();
        let json = body_json(second).await;
        assert_eq!(json["dispatched"], false);
        assert_eq!(json["reason"], "duplicate delivery");
    }
}

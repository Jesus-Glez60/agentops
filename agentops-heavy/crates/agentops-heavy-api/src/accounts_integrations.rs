//! Phase 7 (Module 9's follow-on): minimal user accounts + the generic
//! per-tenant integrations vault, as a self-contained sub-router merged
//! onto the main app (same `merge_*` pattern already used for
//! `/webhooks/linear` — keeps `build_router`'s existing signature and its
//! ~15 existing tests untouched, since this is an independent concern with
//! its own auth model).
//!
//! **A new, separate auth layer, not a replacement** — existing routes
//! (`/repos/*`, `/search/*`) keep the pre-existing single deployment-wide
//! API-key gate (`require_api_key`, self-hosted single-operator model).
//! `/auth/*` and `/integrations/*` here use `require_session` instead — a
//! per-user Bearer session token, verified against `agentops-accounts`.
//! The two coexist deliberately.

use std::sync::{Arc, Mutex};

use agentops_accounts::{AccountStore, User};
use agentops_integrations::{AuthType, CredentialStore, NewCredential};
use agentops_repo_access::secrets::SecretsProvider;
use axum::extract::{Path as AxumPath, Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Clone)]
struct AccountsState {
    accounts: Arc<Mutex<AccountStore>>,
    credentials: Arc<Mutex<CredentialStore>>,
    secrets: Arc<dyn SecretsProvider + Send + Sync>,
}

pub fn build_accounts_integrations_router(accounts: AccountStore, credentials: CredentialStore, secrets: Arc<dyn SecretsProvider + Send + Sync>) -> Router {
    let state = AccountsState { accounts: Arc::new(Mutex::new(accounts)), credentials: Arc::new(Mutex::new(credentials)), secrets };

    Router::new()
        .route("/integrations", get(list_integrations))
        .route("/integrations/{provider}", post(store_integration).delete(delete_integration))
        .route("/integrations/{provider}/oauth/start", get(oauth_start))
        .route("/integrations/{provider}/oauth/callback", get(oauth_callback))
        .layer(middleware::from_fn_with_state(state.clone(), require_session))
        .route("/auth/signup", post(signup))
        .route("/auth/login", post(login))
        .with_state(state)
}

async fn require_session(State(state): State<AccountsState>, mut req: Request, next: Next) -> Response {
    let token = req.headers().get(axum::http::header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer "));
    let Some(token) = token else {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "missing session token" }))).into_response();
    };

    let verified = { state.accounts.lock().unwrap().verify_session(token) };
    match verified {
        Ok(user) => {
            req.extensions_mut().insert(user);
            next.run(req).await
        }
        Err(_) => (StatusCode::UNAUTHORIZED, Json(json!({ "error": "invalid or expired session" }))).into_response(),
    }
}

#[derive(Deserialize)]
struct SignupOrLoginRequest {
    email: String,
    password: String,
}

#[derive(Serialize)]
struct UserView {
    id: i64,
    email: String,
    tenant: String,
}

impl From<User> for UserView {
    fn from(u: User) -> Self {
        UserView { id: u.id, email: u.email, tenant: u.tenant }
    }
}

#[derive(Serialize)]
struct AuthResponse {
    user: UserView,
    session_token: String,
}

async fn signup(State(state): State<AccountsState>, Json(req): Json<SignupOrLoginRequest>) -> (StatusCode, Json<serde_json::Value>) {
    let result = { state.accounts.lock().unwrap().signup(&req.email, &req.password) };
    match result {
        Ok((user, session_token)) => (StatusCode::CREATED, Json(serde_json::to_value(AuthResponse { user: user.into(), session_token }).unwrap())),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e.to_string() }))),
    }
}

async fn login(State(state): State<AccountsState>, Json(req): Json<SignupOrLoginRequest>) -> (StatusCode, Json<serde_json::Value>) {
    let result = { state.accounts.lock().unwrap().login(&req.email, &req.password) };
    match result {
        Ok((user, session_token)) => (StatusCode::OK, Json(serde_json::to_value(AuthResponse { user: user.into(), session_token }).unwrap())),
        Err(_) => (StatusCode::UNAUTHORIZED, Json(json!({ "error": "invalid email or password" }))),
    }
}

#[derive(Serialize)]
struct CredentialView {
    provider: String,
    auth_type: &'static str,
    created_at: String,
    updated_at: String,
}

async fn list_integrations(State(state): State<AccountsState>, axum::Extension(user): axum::Extension<User>) -> (StatusCode, Json<Vec<CredentialView>>) {
    let credentials = state.credentials.lock().unwrap();
    let listed = credentials.list_credentials(&user.tenant).unwrap_or_default();
    let views = listed
        .into_iter()
        .map(|c| CredentialView { provider: c.provider, auth_type: match c.auth_type { AuthType::ApiKey => "api_key", AuthType::OAuth => "oauth" }, created_at: c.created_at, updated_at: c.updated_at })
        .collect();
    (StatusCode::OK, Json(views))
}

#[derive(Deserialize)]
struct StoreIntegrationRequest {
    auth_type: String,
    secret: String,
    refresh_token: Option<String>,
    expires_at: Option<String>,
}

async fn store_integration(
    State(state): State<AccountsState>,
    axum::Extension(user): axum::Extension<User>,
    AxumPath(provider): AxumPath<String>,
    Json(req): Json<StoreIntegrationRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    let auth_type = match req.auth_type.as_str() {
        "api_key" => AuthType::ApiKey,
        "oauth" => AuthType::OAuth,
        other => return (StatusCode::BAD_REQUEST, Json(json!({ "error": format!("invalid auth_type {other:?}, expected 'api_key' or 'oauth'") }))),
    };

    let result = {
        let credentials = state.credentials.lock().unwrap();
        credentials.store_credential(
            state.secrets.as_ref(),
            &user.tenant,
            NewCredential { provider: &provider, auth_type, secret: &req.secret, refresh_token: req.refresh_token.as_deref(), expires_at: req.expires_at.as_deref() },
        )
    };
    match result {
        Ok(()) => (StatusCode::OK, Json(json!({ "provider": provider, "stored": true }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    }
}

async fn delete_integration(State(state): State<AccountsState>, axum::Extension(user): axum::Extension<User>, AxumPath(provider): AxumPath<String>) -> (StatusCode, Json<serde_json::Value>) {
    let result = { state.credentials.lock().unwrap().delete_credential(&user.tenant, &provider) };
    match result {
        Ok(true) => (StatusCode::OK, Json(json!({ "provider": provider, "deleted": true }))),
        Ok(false) => (StatusCode::NOT_FOUND, Json(json!({ "error": format!("no credential stored for provider {provider:?}") }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    }
}

/// Designed, not implemented — a real OAuth flow needs a registered
/// provider OAuth app (`client_id`/`client_secret`), an operator setup
/// step this can't do via API (see Phase 6's `ensure_webhook_registered`
/// doc comment for the analogous GitHub-App-style precedent). Returns a
/// clear "not configured" error until that env var actually exists for
/// `provider`, rather than a 404 that reads as "route doesn't exist."
async fn oauth_start(AxumPath(provider): AxumPath<String>) -> (StatusCode, Json<serde_json::Value>) {
    (StatusCode::NOT_IMPLEMENTED, Json(json!({ "error": format!("OAuth is not configured for provider {provider:?} on this deployment yet — use POST /integrations/{provider} with an API key instead") })))
}

async fn oauth_callback(AxumPath(provider): AxumPath<String>) -> (StatusCode, Json<serde_json::Value>) {
    (StatusCode::NOT_IMPLEMENTED, Json(json!({ "error": format!("OAuth is not configured for provider {provider:?} on this deployment yet") })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentops_repo_access::secrets::EnvSecretsProvider;
    use axum::body::Body;
    use axum::http::Request as HttpRequest;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn test_router() -> Router {
        let accounts = AccountStore::open_in_memory().unwrap();
        let credentials = CredentialStore::open_in_memory().unwrap();
        let secrets: Arc<dyn SecretsProvider + Send + Sync> = Arc::new(EnvSecretsProvider::from_hex(&"ab".repeat(32)).unwrap());
        build_accounts_integrations_router(accounts, credentials, secrets)
    }

    async fn body_json(response: axum::response::Response) -> serde_json::Value {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn signup_then_using_the_session_token_to_list_integrations_works() {
        let app = test_router();

        let signup_response = app
            .clone()
            .oneshot(HttpRequest::post("/auth/signup").header("content-type", "application/json").body(Body::from(r#"{"email":"dev@example.com","password":"correct horse battery staple"}"#)).unwrap())
            .await
            .unwrap();
        assert_eq!(signup_response.status(), StatusCode::CREATED);
        let body = body_json(signup_response).await;
        let token = body["session_token"].as_str().unwrap().to_string();

        let list_response = app.oneshot(HttpRequest::get("/integrations").header("authorization", format!("Bearer {token}")).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(list_response.status(), StatusCode::OK);
        let listed = body_json(list_response).await;
        assert_eq!(listed, serde_json::json!([]));
    }

    #[tokio::test]
    async fn integrations_endpoints_reject_a_request_with_no_session_token() {
        let app = test_router();
        let response = app.oneshot(HttpRequest::get("/integrations").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn integrations_endpoints_reject_a_bogus_session_token() {
        let app = test_router();
        let response = app.oneshot(HttpRequest::get("/integrations").header("authorization", "Bearer not-a-real-token").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn storing_then_listing_an_integration_never_returns_the_secret() {
        let app = test_router();
        let signup_response =
            app.clone().oneshot(HttpRequest::post("/auth/signup").header("content-type", "application/json").body(Body::from(r#"{"email":"dev@example.com","password":"pw"}"#)).unwrap()).await.unwrap();
        let token = body_json(signup_response).await["session_token"].as_str().unwrap().to_string();

        let store_response = app
            .clone()
            .oneshot(
                HttpRequest::post("/integrations/linear")
                    .header("authorization", format!("Bearer {token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"auth_type":"api_key","secret":"lin_api_supersecret"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(store_response.status(), StatusCode::OK);

        let list_response = app.oneshot(HttpRequest::get("/integrations").header("authorization", format!("Bearer {token}")).body(Body::empty()).unwrap()).await.unwrap();
        let listed = body_json(list_response).await;
        assert_eq!(listed[0]["provider"], "linear");
        assert!(!listed.to_string().contains("lin_api_supersecret"), "the raw secret must never appear in a list response");
    }

    #[tokio::test]
    async fn two_different_users_integrations_never_cross_over() {
        let app = test_router();

        let sign_up = |email: &'static str, app: Router| async move {
            let response = app.oneshot(HttpRequest::post("/auth/signup").header("content-type", "application/json").body(Body::from(format!(r#"{{"email":"{email}","password":"pw"}}"#))).unwrap()).await.unwrap();
            body_json(response).await["session_token"].as_str().unwrap().to_string()
        };
        let token_a = sign_up("a@example.com", app.clone()).await;
        let token_b = sign_up("b@example.com", app.clone()).await;

        app.clone()
            .oneshot(
                HttpRequest::post("/integrations/linear")
                    .header("authorization", format!("Bearer {token_a}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"auth_type":"api_key","secret":"tenant-a-secret"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        let list_b = app.oneshot(HttpRequest::get("/integrations").header("authorization", format!("Bearer {token_b}")).body(Body::empty()).unwrap()).await.unwrap();
        let listed_b = body_json(list_b).await;
        assert_eq!(listed_b, serde_json::json!([]), "tenant b must not see tenant a's integrations");
    }

    #[tokio::test]
    async fn oauth_endpoints_report_not_implemented_rather_than_404() {
        let app = test_router();
        let signup_response =
            app.clone().oneshot(HttpRequest::post("/auth/signup").header("content-type", "application/json").body(Body::from(r#"{"email":"dev@example.com","password":"pw"}"#)).unwrap()).await.unwrap();
        let token = body_json(signup_response).await["session_token"].as_str().unwrap().to_string();

        let response = app.oneshot(HttpRequest::get("/integrations/linear/oauth/start").header("authorization", format!("Bearer {token}")).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    }
}

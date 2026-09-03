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

use agentops_accounts::{AccountStore, NewAccount, PreferencesUpdate, ProfileUpdate, User};
use agentops_security::api_key::hash_api_key;
use agentops_integrations::{AuthType, CredentialStore, NewCredential};
use agentops_repo_access::secrets::SecretsProvider;
use agentops_teams::TeamStore;
use axum::extract::{Path as AxumPath, Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Clone)]
struct AccountsState {
    accounts: Arc<Mutex<AccountStore>>,
    credentials: Arc<Mutex<CredentialStore>>,
    secrets: Arc<dyn SecretsProvider + Send + Sync>,
    /// For gating the org-wide `/integrations*` routes behind
    /// `integrations.manage` -- added alongside the Owner/custom-roles
    /// work. Not `Option` (unlike `AppState`'s `accounts`/`teams`): every
    /// test in this file already funnels through the single
    /// `test_router()` helper below, so requiring it outright is a
    /// one-line fix here, not the many-call-site cost that made `AppState`
    /// keep its fields optional.
    teams: Arc<Mutex<TeamStore>>,
    /// For building `verification_uri`/`verification_uri_complete` on the
    /// device-authorization flow's `POST /auth/cli/device` response --
    /// reuses the same `AGENTOPS_WEB_APP_URL` value `AppState` (the
    /// `/repos/*` router) already reads for its GitHub App install-flow
    /// callback, rather than introducing a second env var for the same
    /// "what's this deployment's own public URL" concept.
    ///
    /// Deliberately `Option`, not a silently-wrong fallback like
    /// `http://localhost:3000` -- caught live: the API backend and the
    /// web app frontend are commonly on entirely different public domains
    /// in a real deployment (confirmed against this project's own
    /// `app.agentops.dedyn.io` web app vs. a separate API host), so
    /// there's no correct default to guess from the request itself the
    /// way `connect_sh.rs`'s `derive_remote_url` can for a same-host
    /// deployment. `cli_device_start` returns a clear, actionable error
    /// when this is unset rather than minting a device code that would
    /// point at a dead link.
    web_app_url: Option<String>,
}

pub fn build_accounts_integrations_router(accounts: AccountStore, credentials: CredentialStore, secrets: Arc<dyn SecretsProvider + Send + Sync>, teams: TeamStore, web_app_url: Option<String>) -> Router {
    let state = AccountsState { accounts: Arc::new(Mutex::new(accounts)), credentials: Arc::new(Mutex::new(credentials)), secrets, teams: Arc::new(Mutex::new(teams)), web_app_url };

    Router::new()
        .route("/integrations", get(list_integrations))
        .route("/integrations/{provider}", post(store_integration).delete(delete_integration))
        .route("/integrations/{provider}/oauth/start", get(oauth_start))
        .route("/integrations/{provider}/oauth/callback", get(oauth_callback))
        .route("/integrations/me", get(list_my_integrations))
        .route("/integrations/me/{provider}", post(store_my_integration).delete(delete_my_integration))
        .route("/auth/me", get(me).patch(update_me))
        .route("/auth/me/preferences", patch(update_me_preferences))
        .route("/auth/me/password", post(update_me_password))
        .route("/auth/me/complete-onboarding", post(complete_onboarding))
        .route("/auth/sessions", get(list_sessions))
        .route("/auth/sessions/revoke-others", post(revoke_other_sessions))
        .route("/auth/sessions/{id}", axum::routing::delete(revoke_session_by_id))
        .route("/auth/api-keys", get(list_api_keys).post(create_api_key))
        .route("/auth/api-keys/{id}", axum::routing::delete(revoke_api_key))
        .route("/auth/2fa/enroll", post(enroll_2fa))
        .route("/auth/2fa/confirm", post(confirm_2fa))
        .route("/auth/2fa/disable", post(disable_2fa))
        .route("/auth/2fa/backup-codes/regenerate", post(regenerate_backup_codes))
        .route("/auth/logout", post(logout))
        .route("/auth/cli/device/approve", post(cli_device_approve))
        .layer(middleware::from_fn_with_state(state.clone(), require_session))
        .route("/auth/bootstrap-status", get(bootstrap_status))
        .route("/bootstrap/config", post(bootstrap_config))
        .route("/auth/signup", post(signup))
        .route("/auth/login", post(login))
        .route("/auth/login/2fa", post(login_2fa))
        .route("/auth/cli/device", post(cli_device_start))
        .route("/auth/cli/device/token", post(cli_device_token))
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
            {
                let accounts = state.accounts.lock().unwrap();
                let _ = accounts.touch_session(token);
            }
            req.extensions_mut().insert(user);
            next.run(req).await
        }
        Err(_) => (StatusCode::UNAUTHORIZED, Json(json!({ "error": "invalid or expired session" }))).into_response(),
    }
}

/// Best-effort request metadata for the Active Sessions UI -- `User-Agent`
/// is always present from a real browser; `ip_address` prefers
/// `X-Forwarded-For` (set by a reverse proxy) and falls back to empty
/// rather than requiring `ConnectInfo` wiring this deployment doesn't set
/// up yet (self-hosted, typically local/behind a proxy that already sets
/// this header).
fn request_metadata(headers: &axum::http::HeaderMap) -> (String, String) {
    let user_agent = headers.get(axum::http::header::USER_AGENT).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let ip_address = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()).and_then(|v| v.split(',').next()).unwrap_or("").trim().to_string();
    (user_agent, ip_address)
}

#[derive(Deserialize)]
struct SignupRequest {
    email: String,
    password: String,
    first_name: String,
    last_name: String,
    /// Redeems a pending invite in the same request as account creation —
    /// required once `AGENTOPS_SIGNUP_MODE=first-user-only` (the default)
    /// and an account already exists on this instance. See `signup()`.
    #[serde(default)]
    invite_token: Option<String>,
}

/// `open` (any request creates a brand-new tenant, unlimited -- the
/// existing/default behavior, preserved for anyone already running this
/// way) or `first-user-only` (once any account exists on this instance,
/// further signup requires a valid `invite_token`) -- see `signup()`. The
/// three self-host deployment wizards (Docker/PM2/classic) write
/// `AGENTOPS_SIGNUP_MODE=first-user-only` into the `.env` they generate,
/// rather than this function defaulting to it, so existing/hosted
/// deployments that never opted in keep today's always-open behavior.
fn signup_mode() -> String {
    std::env::var("AGENTOPS_SIGNUP_MODE").unwrap_or_else(|_| "open".to_string())
}

/// Pure gating decision, factored out of `signup()` so it's unit-testable
/// without mutating the process-global `AGENTOPS_SIGNUP_MODE` env var (which
/// would race every other concurrently-running test that also calls
/// `signup()`).
fn signup_allowed(mode: &str, has_accounts: bool, has_invite_token: bool) -> bool {
    mode != "first-user-only" || !has_accounts || has_invite_token
}

#[derive(Deserialize)]
struct LoginRequest {
    email: String,
    password: String,
}

#[derive(Serialize)]
struct UserView {
    id: i64,
    email: String,
    first_name: String,
    last_name: String,
    tenant: String,
    avatar_url: Option<String>,
    handle: Option<String>,
    bio: String,
    location: String,
    theme_pref: String,
    default_search_scope: String,
    show_gotcha_callouts: bool,
    graph_layout_algorithm: String,
    two_factor_enabled: bool,
    onboarding_completed: bool,
}

/// Not a plain `From<User>` -- 2FA status lives in a separate table
/// (`AccountStore::has_2fa_enabled`), so building the full view needs a
/// store reference, not just the `User` struct. `unwrap_or(false)` on a
/// lookup failure isn't reachable in practice (the user row was just read
/// from the same store), but fails safe (looks disabled) rather than
/// panicking a response over a display field if it ever were.
fn user_view(accounts: &AccountStore, u: User) -> UserView {
    let two_factor_enabled = accounts.has_2fa_enabled(u.id).unwrap_or(false);
    let onboarding_completed = u.onboarding_completed_at.is_some();
    UserView {
        id: u.id,
        email: u.email,
        first_name: u.first_name,
        last_name: u.last_name,
        tenant: u.tenant,
        avatar_url: u.avatar_url,
        handle: u.handle,
        bio: u.bio,
        location: u.location,
        theme_pref: u.theme_pref,
        default_search_scope: u.default_search_scope,
        show_gotcha_callouts: u.show_gotcha_callouts,
        graph_layout_algorithm: u.graph_layout_algorithm,
        two_factor_enabled,
        onboarding_completed,
    }
}

#[derive(Serialize)]
struct AuthResponse {
    user: UserView,
    session_token: String,
}

/// Unauthenticated -- lets the frontend steer first-run UX (default to the
/// Signup tab on an empty instance; hide it once `signup_open` is false and
/// no invite token is present) without needing a session first.
async fn bootstrap_status(State(state): State<AccountsState>) -> (StatusCode, Json<serde_json::Value>) {
    match state.accounts.lock().unwrap().any_account_exists() {
        Ok(has_accounts) => {
            let signup_open = !has_accounts || signup_mode() == "open";
            (StatusCode::OK, Json(json!({ "has_accounts": has_accounts, "signup_open": signup_open })))
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    }
}

/// Unauthenticated by design (there's no session to have yet on a brand
/// new instance) but deliberately **first-run only**: once any account
/// exists, this 403s rather than letting an anonymous network caller
/// rewrite a running instance's master key/DB credentials. Writes `.env`
/// in the server process's current directory (the same file both the PM2
/// `ecosystem.config.js` and a later `agentops serve-api` invocation read
/// from — see the Method 2/3 deployment docs); the frontend tells the user
/// to restart (`pm2 restart ecosystem.config.js`) since there's no
/// hot-reload of env-derived config.
async fn bootstrap_config(State(state): State<AccountsState>, Json(config): Json<agentops_manifest::BootstrapConfig>) -> (StatusCode, Json<serde_json::Value>) {
    match state.accounts.lock().unwrap().any_account_exists() {
        Ok(true) => return (StatusCode::FORBIDDEN, Json(json!({ "error": "this instance is already set up; edit .env by hand and restart to change infra config" }))),
        Ok(false) => {}
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    }

    if let Err(errors) = config.validate() {
        return (StatusCode::BAD_REQUEST, Json(json!({ "errors": errors })));
    }

    match std::fs::write(".env", config.to_env_file()) {
        Ok(()) => (StatusCode::OK, Json(json!({ "ok": true }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("writing .env: {e}") }))),
    }
}

async fn signup(State(state): State<AccountsState>, headers: axum::http::HeaderMap, Json(req): Json<SignupRequest>) -> (StatusCode, Json<serde_json::Value>) {
    let first_name = req.first_name.trim();
    let last_name = req.last_name.trim();
    if first_name.is_empty() || last_name.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "first_name and last_name are required" })));
    }

    let has_accounts = match state.accounts.lock().unwrap().any_account_exists() {
        Ok(v) => v,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    };
    // `invite_token` only has to be *presented* here to prove this signup
    // is invite-driven -- `preview_invite` is read-only (unlike
    // `accept_invite`, it never marks the invite consumed), because the
    // actual join happens later: the existing `/invite/{token}` page
    // (`InviteLandingClient`) already redirects a signed-out visitor to
    // `/login?from=/invite/{token}`, and once they're signed in (from this
    // very signup) it calls the real `POST /invites/accept` itself. Calling
    // `accept_invite` here too would burn the token twice and make that
    // follow-up call fail.
    let invite_is_valid = match &req.invite_token {
        Some(token) => state.teams.lock().unwrap().preview_invite(token).map(|p| p.is_some()),
        None => Ok(false),
    };
    let invite_is_valid = match invite_is_valid {
        Ok(v) => v,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    };
    if req.invite_token.is_some() && !invite_is_valid {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "invalid or expired invite" })));
    }
    if !signup_allowed(&signup_mode(), has_accounts, invite_is_valid) {
        return (StatusCode::FORBIDDEN, Json(json!({ "error": "this instance requires an invite to sign up" })));
    }

    let result = { state.accounts.lock().unwrap().signup(NewAccount { email: &req.email, password: &req.password, first_name, last_name }) };
    let (user, session_token) = match result {
        Ok(v) => v,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(json!({ "error": e.to_string() }))),
    };

    let (user_agent, ip_address) = request_metadata(&headers);
    let accounts = state.accounts.lock().unwrap();
    let _ = accounts.record_session_metadata(&session_token, &user_agent, &ip_address);
    (StatusCode::CREATED, Json(serde_json::to_value(AuthResponse { user: user_view(&accounts, user), session_token }).unwrap()))
}

/// **2FA branch point.** Credentials alone used to be enough to issue a
/// session (`AccountStore::login` still works exactly that way, for
/// callers that don't care about 2FA); this handler instead checks
/// credentials via `verify_credentials`, and if the user has 2FA enabled,
/// stops short of issuing a session and returns a `202` challenge instead
/// -- the real session only comes from `POST /auth/login/2fa` completing
/// that challenge with a valid code. Any client of `POST /auth/login`
/// (this app's `loginWithPassword`, any other caller) must handle both
/// response shapes now.
async fn login(State(state): State<AccountsState>, headers: axum::http::HeaderMap, Json(req): Json<LoginRequest>) -> (StatusCode, Json<serde_json::Value>) {
    let user = { state.accounts.lock().unwrap().verify_credentials(&req.email, &req.password) };
    let Ok(user) = user else {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "invalid email or password" })));
    };

    let needs_2fa = { state.accounts.lock().unwrap().has_2fa_enabled(user.id) };
    match needs_2fa {
        Ok(true) => {
            let challenge = { state.accounts.lock().unwrap().create_login_challenge(user.id) };
            match challenge {
                Ok(challenge_token) => (StatusCode::ACCEPTED, Json(json!({ "two_factor_required": true, "challenge_token": challenge_token }))),
                Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
            }
        }
        Ok(false) => {
            let session_token = { state.accounts.lock().unwrap().issue_session(user.id) };
            match session_token {
                Ok(session_token) => {
                    let (user_agent, ip_address) = request_metadata(&headers);
                    let accounts = state.accounts.lock().unwrap();
                    let _ = accounts.record_session_metadata(&session_token, &user_agent, &ip_address);
                    (StatusCode::OK, Json(serde_json::to_value(AuthResponse { user: user_view(&accounts, user), session_token }).unwrap()))
                }
                Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
            }
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    }
}

#[derive(Deserialize)]
struct Login2faRequest {
    challenge_token: String,
    code: String,
}

async fn login_2fa(State(state): State<AccountsState>, headers: axum::http::HeaderMap, Json(req): Json<Login2faRequest>) -> (StatusCode, Json<serde_json::Value>) {
    let result = { state.accounts.lock().unwrap().complete_login_challenge(state.secrets.as_ref(), &req.challenge_token, &req.code) };
    match result {
        Ok((user, session_token)) => {
            let (user_agent, ip_address) = request_metadata(&headers);
            let accounts = state.accounts.lock().unwrap();
            let _ = accounts.record_session_metadata(&session_token, &user_agent, &ip_address);
            (StatusCode::OK, Json(serde_json::to_value(AuthResponse { user: user_view(&accounts, user), session_token }).unwrap()))
        }
        Err(e) => (StatusCode::UNAUTHORIZED, Json(json!({ "error": e.to_string() }))),
    }
}

/// `gh auth login`-style device-authorization flow, step 1: a CLI with no
/// browser of its own (or on a headless/SSH-only machine) requests a code
/// pair here, then shows `verification_uri_complete` for the person to
/// open on *any* device with a browser. Field/status names throughout
/// this flow follow RFC 8628 (OAuth 2.0 Device Authorization Grant)
/// directly rather than inventing ad-hoc equivalents.
#[derive(Deserialize)]
struct CliDeviceStartRequest {
    device_name: String,
}

async fn cli_device_start(State(state): State<AccountsState>, Json(req): Json<CliDeviceStartRequest>) -> (StatusCode, Json<serde_json::Value>) {
    let Some(web_app_url) = state.web_app_url.as_deref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "this deployment hasn't set AGENTOPS_WEB_APP_URL, so device-authorization login can't build a working link — ask your admin to set it, or paste an already-generated API key instead" })),
        );
    };
    let result = { state.accounts.lock().unwrap().create_device_auth_code(&req.device_name) };
    match result {
        Ok((device_code, user_code, expires_in)) => {
            let verification_uri = format!("{web_app_url}/cli-auth");
            let verification_uri_complete = format!("{web_app_url}/cli-auth?code={user_code}");
            (
                StatusCode::OK,
                Json(json!({
                    "device_code": device_code,
                    "user_code": user_code,
                    "verification_uri": verification_uri,
                    "verification_uri_complete": verification_uri_complete,
                    "expires_in": expires_in,
                    "interval": 5,
                })),
            )
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    }
}

/// Step 2: the browser-side approval, called from the logged-in user's own
/// session (the `/cli-auth` page) when they click Approve/Deny.
#[derive(Deserialize)]
struct CliDeviceApproveRequest {
    user_code: String,
    action: String,
}

async fn cli_device_approve(State(state): State<AccountsState>, axum::Extension(user): axum::Extension<User>, Json(req): Json<CliDeviceApproveRequest>) -> (StatusCode, Json<serde_json::Value>) {
    let approve = match req.action.as_str() {
        "approve" => true,
        "deny" => false,
        _ => return (StatusCode::BAD_REQUEST, Json(json!({ "error": "action must be \"approve\" or \"deny\"" }))),
    };
    let result = { state.accounts.lock().unwrap().resolve_device_auth_code(&req.user_code, user.id, approve) };
    match result {
        Ok(()) => (StatusCode::OK, Json(json!({ "ok": true }))),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e.to_string() }))),
    }
}

/// Step 3: the CLI polls this repeatedly until it stops returning
/// `authorization_pending`. On `approved`, a key is minted just-in-time --
/// `resolve_device_auth_code` never stores a raw key anywhere, so this is
/// the only place one comes into existence, right before being handed
/// back over the wire once.
#[derive(Deserialize)]
struct CliDeviceTokenRequest {
    device_code: String,
}

async fn cli_device_token(State(state): State<AccountsState>, Json(req): Json<CliDeviceTokenRequest>) -> (StatusCode, Json<serde_json::Value>) {
    let poll_result = { state.accounts.lock().unwrap().poll_device_auth_code(&req.device_code) };
    match poll_result {
        Ok(agentops_accounts::DeviceAuthPollResult::Pending) => (StatusCode::OK, Json(json!({ "error": "authorization_pending" }))),
        Ok(agentops_accounts::DeviceAuthPollResult::Denied) => (StatusCode::OK, Json(json!({ "error": "access_denied" }))),
        Ok(agentops_accounts::DeviceAuthPollResult::Expired) => (StatusCode::OK, Json(json!({ "error": "expired_token" }))),
        Ok(agentops_accounts::DeviceAuthPollResult::Approved { user_id, device_name }) => {
            let key_name = device_name.map(|n| format!("CLI ({n})")).unwrap_or_else(|| "CLI".to_string());
            let key_result = { state.accounts.lock().unwrap().create_user_api_key(user_id, &key_name) };
            match key_result {
                Ok((_, raw_key)) => (StatusCode::OK, Json(json!({ "status": "approved", "api_key": raw_key }))),
                Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
            }
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    }
}

async fn me(State(state): State<AccountsState>, axum::Extension(user): axum::Extension<User>) -> (StatusCode, Json<UserView>) {
    let view = user_view(&state.accounts.lock().unwrap(), user);
    (StatusCode::OK, Json(view))
}

/// Every field optional -- an omitted (or `null`) field means "leave this
/// alone", matching `ProfileUpdate`'s `None` = unchanged semantics. `serde`
/// maps a missing JSON key and an explicit `null` to the same `None`, which
/// is fine here: there's no field where a client would ever want to send
/// an explicit "clear this back to null" (bio/location/handle all have
/// sane empty-string/None defaults already reachable by just not setting
/// them in the first place).
#[derive(Deserialize, Default)]
struct UpdateMeRequest {
    first_name: Option<String>,
    last_name: Option<String>,
    handle: Option<String>,
    bio: Option<String>,
    location: Option<String>,
}

async fn update_me(State(state): State<AccountsState>, axum::Extension(user): axum::Extension<User>, Json(req): Json<UpdateMeRequest>) -> (StatusCode, Json<serde_json::Value>) {
    let update = ProfileUpdate { first_name: req.first_name.as_deref(), last_name: req.last_name.as_deref(), handle: req.handle.as_deref(), bio: req.bio.as_deref(), location: req.location.as_deref() };
    let accounts = state.accounts.lock().unwrap();
    let result = accounts.update_profile(user.id, update);
    match result {
        Ok(updated) => (StatusCode::OK, Json(serde_json::to_value(user_view(&accounts, updated)).unwrap())),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e.to_string() }))),
    }
}

#[derive(Deserialize, Default)]
struct UpdateMePreferencesRequest {
    theme_pref: Option<String>,
    default_search_scope: Option<String>,
    show_gotcha_callouts: Option<bool>,
    graph_layout_algorithm: Option<String>,
}

/// Marks the `/welcome` checklist done -- idempotent, callable from any
/// item's own completion or the persistent "Continue to dashboard" button,
/// per that page's "never block the escape hatch" design (see the
/// onboarding plan's UX research section).
async fn complete_onboarding(State(state): State<AccountsState>, axum::Extension(user): axum::Extension<User>) -> (StatusCode, Json<serde_json::Value>) {
    let accounts = state.accounts.lock().unwrap();
    match accounts.mark_onboarding_complete(user.id) {
        Ok(updated) => (StatusCode::OK, Json(serde_json::to_value(user_view(&accounts, updated)).unwrap())),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    }
}

async fn update_me_preferences(State(state): State<AccountsState>, axum::Extension(user): axum::Extension<User>, Json(req): Json<UpdateMePreferencesRequest>) -> (StatusCode, Json<serde_json::Value>) {
    let update = PreferencesUpdate {
        theme_pref: req.theme_pref.as_deref(),
        default_search_scope: req.default_search_scope.as_deref(),
        show_gotcha_callouts: req.show_gotcha_callouts,
        graph_layout_algorithm: req.graph_layout_algorithm.as_deref(),
    };
    let accounts = state.accounts.lock().unwrap();
    let result = accounts.update_preferences(user.id, update);
    match result {
        Ok(updated) => (StatusCode::OK, Json(serde_json::to_value(user_view(&accounts, updated)).unwrap())),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e.to_string() }))),
    }
}

#[derive(Deserialize)]
struct UpdateMePasswordRequest {
    current_password: String,
    new_password: String,
}

/// Changes the password, then revokes every *other* session -- a stolen
/// session shouldn't survive its owner noticing and changing their
/// password, but the request that just proved it knows the new password
/// shouldn't lock itself out either.
async fn update_me_password(
    State(state): State<AccountsState>,
    headers: axum::http::HeaderMap,
    axum::Extension(user): axum::Extension<User>,
    Json(req): Json<UpdateMePasswordRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    let result = { state.accounts.lock().unwrap().change_password(user.id, &req.current_password, &req.new_password) };
    match result {
        Ok(()) => {
            if let Some(current_token) = bearer_token(&headers) {
                let current_hash = hash_api_key(current_token);
                let _ = state.accounts.lock().unwrap().revoke_all_other_sessions(user.id, &current_hash);
            }
            (StatusCode::OK, Json(json!({ "updated": true })))
        }
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e.to_string() }))),
    }
}

#[derive(Serialize)]
struct SessionView {
    id: i64,
    user_agent: String,
    ip_address: String,
    created_at: String,
    last_seen_at: String,
    is_current: bool,
}

fn bearer_token(headers: &axum::http::HeaderMap) -> Option<&str> {
    headers.get(axum::http::header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer "))
}

async fn list_sessions(State(state): State<AccountsState>, headers: axum::http::HeaderMap, axum::Extension(user): axum::Extension<User>) -> (StatusCode, Json<serde_json::Value>) {
    let current_hash = bearer_token(&headers).map(hash_api_key);
    let result = { state.accounts.lock().unwrap().list_sessions(user.id) };
    match result {
        Ok(sessions) => {
            let views: Vec<SessionView> = sessions
                .into_iter()
                .map(|s| SessionView {
                    id: s.id,
                    user_agent: s.user_agent,
                    ip_address: s.ip_address,
                    created_at: s.created_at,
                    is_current: current_hash.as_deref() == Some(s.token_hash.as_str()),
                    last_seen_at: s.last_seen_at,
                })
                .collect();
            (StatusCode::OK, Json(serde_json::to_value(views).unwrap()))
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    }
}

async fn revoke_session_by_id(State(state): State<AccountsState>, axum::Extension(user): axum::Extension<User>, AxumPath(session_id): AxumPath<i64>) -> (StatusCode, Json<serde_json::Value>) {
    let result = { state.accounts.lock().unwrap().revoke_session_by_id(user.id, session_id) };
    match result {
        Ok(true) => (StatusCode::OK, Json(json!({ "revoked": true }))),
        Ok(false) => (StatusCode::NOT_FOUND, Json(json!({ "error": "no such session" }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    }
}

async fn revoke_other_sessions(State(state): State<AccountsState>, headers: axum::http::HeaderMap, axum::Extension(user): axum::Extension<User>) -> (StatusCode, Json<serde_json::Value>) {
    let Some(current_hash) = bearer_token(&headers).map(hash_api_key) else {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "missing session token" })));
    };
    let result = { state.accounts.lock().unwrap().revoke_all_other_sessions(user.id, &current_hash) };
    match result {
        Ok(revoked) => (StatusCode::OK, Json(json!({ "revoked": revoked }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    }
}

#[derive(Serialize)]
struct ApiKeyView {
    id: i64,
    name: String,
    key_prefix: String,
    last_used_at: Option<String>,
    created_at: String,
}

impl From<agentops_accounts::ApiKeyInfo> for ApiKeyView {
    fn from(k: agentops_accounts::ApiKeyInfo) -> Self {
        ApiKeyView { id: k.id, name: k.name, key_prefix: k.key_prefix, last_used_at: k.last_used_at, created_at: k.created_at }
    }
}

async fn list_api_keys(State(state): State<AccountsState>, axum::Extension(user): axum::Extension<User>) -> (StatusCode, Json<serde_json::Value>) {
    let result = { state.accounts.lock().unwrap().list_user_api_keys(user.id) };
    match result {
        Ok(keys) => {
            let views: Vec<ApiKeyView> = keys.into_iter().map(ApiKeyView::from).collect();
            (StatusCode::OK, Json(serde_json::to_value(views).unwrap()))
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    }
}

#[derive(Deserialize)]
struct CreateApiKeyRequest {
    name: String,
}

async fn create_api_key(State(state): State<AccountsState>, axum::Extension(user): axum::Extension<User>, Json(req): Json<CreateApiKeyRequest>) -> (StatusCode, Json<serde_json::Value>) {
    if req.name.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "name is required" })));
    }
    let result = { state.accounts.lock().unwrap().create_user_api_key(user.id, req.name.trim()) };
    match result {
        Ok((info, raw_key)) => {
            let mut view = serde_json::to_value(ApiKeyView::from(info)).unwrap();
            view["key"] = json!(raw_key);
            (StatusCode::CREATED, Json(view))
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    }
}

async fn revoke_api_key(State(state): State<AccountsState>, axum::Extension(user): axum::Extension<User>, AxumPath(key_id): AxumPath<i64>) -> (StatusCode, Json<serde_json::Value>) {
    let result = { state.accounts.lock().unwrap().revoke_user_api_key(user.id, key_id) };
    match result {
        Ok(true) => (StatusCode::OK, Json(json!({ "revoked": true }))),
        Ok(false) => (StatusCode::NOT_FOUND, Json(json!({ "error": "no such API key" }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    }
}

async fn enroll_2fa(State(state): State<AccountsState>, axum::Extension(user): axum::Extension<User>) -> (StatusCode, Json<serde_json::Value>) {
    let result = { state.accounts.lock().unwrap().begin_2fa_enrollment(state.secrets.as_ref(), user.id) };
    match result {
        Ok(enrollment) => (StatusCode::OK, Json(json!({ "secret": enrollment.secret_base32, "otpauth_uri": enrollment.otpauth_uri, "qr_data_uri": enrollment.qr_data_uri }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    }
}

#[derive(Deserialize)]
struct Confirm2faRequest {
    code: String,
}

async fn confirm_2fa(State(state): State<AccountsState>, axum::Extension(user): axum::Extension<User>, Json(req): Json<Confirm2faRequest>) -> (StatusCode, Json<serde_json::Value>) {
    let result = { state.accounts.lock().unwrap().confirm_2fa_enrollment(state.secrets.as_ref(), user.id, &req.code) };
    match result {
        Ok(backup_codes) => (StatusCode::OK, Json(json!({ "backup_codes": backup_codes }))),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e.to_string() }))),
    }
}

#[derive(Deserialize)]
struct PasswordGatedRequest {
    password: String,
}

async fn disable_2fa(State(state): State<AccountsState>, axum::Extension(user): axum::Extension<User>, Json(req): Json<PasswordGatedRequest>) -> (StatusCode, Json<serde_json::Value>) {
    let result = { state.accounts.lock().unwrap().disable_2fa(user.id, &req.password) };
    match result {
        Ok(()) => (StatusCode::OK, Json(json!({ "disabled": true }))),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e.to_string() }))),
    }
}

async fn regenerate_backup_codes(State(state): State<AccountsState>, axum::Extension(user): axum::Extension<User>, Json(req): Json<PasswordGatedRequest>) -> (StatusCode, Json<serde_json::Value>) {
    let result = { state.accounts.lock().unwrap().regenerate_backup_codes(user.id, &req.password) };
    match result {
        Ok(backup_codes) => (StatusCode::OK, Json(json!({ "backup_codes": backup_codes }))),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e.to_string() }))),
    }
}

/// Revokes the presented session token server-side, so a stolen or
/// forgotten-about token stops working immediately rather than merely
/// being discarded client-side until its 30-day natural expiry.
/// `require_session` already validated the token and inserted the `User`
/// extension, but it doesn't hand the raw token forward — re-reading the
/// `Authorization` header here is simpler than threading it through the
/// middleware just for this one handler.
async fn logout(headers: axum::http::HeaderMap, State(state): State<AccountsState>) -> StatusCode {
    if let Some(token) = bearer_token(&headers) {
        let _ = state.accounts.lock().unwrap().revoke_session(token);
    }
    StatusCode::NO_CONTENT
}

#[derive(Serialize)]
struct CredentialView {
    provider: String,
    auth_type: &'static str,
    created_at: String,
    updated_at: String,
}

/// The org-wide credential vault is Owner/Admin territory -- even read
/// access to "which providers are connected" is more than a Member needs,
/// same least-privilege posture `/team/audit-log` already established.
/// Must run `ensure_membership` first: a brand-new user who has never
/// touched `/team/*` has no membership row yet, and `has_capability` fails
/// closed for that in isolation -- same bug class caught live-testing
/// `/repos/*`'s equivalent gate, fixed here from the start rather than
/// rediscovered.
fn require_integrations_manage(state: &AccountsState, user: &User) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    let teams = state.teams.lock().unwrap();
    if let Err(e) = teams.ensure_membership(&user.tenant, user.id) {
        return Err((StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))));
    }
    match agentops_teams::has_capability(&teams, &user.tenant, user.id, agentops_teams::CAP_INTEGRATIONS_MANAGE) {
        Ok(true) => Ok(()),
        Ok(false) => Err((StatusCode::FORBIDDEN, Json(json!({ "error": "missing required capability: integrations.manage" })))),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() })))),
    }
}

async fn list_integrations(State(state): State<AccountsState>, axum::Extension(user): axum::Extension<User>) -> (StatusCode, Json<serde_json::Value>) {
    if let Err(err) = require_integrations_manage(&state, &user) {
        return err;
    }
    let credentials = state.credentials.lock().unwrap();
    let listed = credentials.list_credentials(&user.tenant).unwrap_or_default();
    let views: Vec<CredentialView> = listed
        .into_iter()
        .map(|c| CredentialView { provider: c.provider, auth_type: match c.auth_type { AuthType::ApiKey => "api_key", AuthType::OAuth => "oauth" }, created_at: c.created_at, updated_at: c.updated_at })
        .collect();
    (StatusCode::OK, Json(serde_json::to_value(views).unwrap()))
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
    if let Err(err) = require_integrations_manage(&state, &user) {
        return err;
    }
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
    if let Err(err) = require_integrations_manage(&state, &user) {
        return err;
    }
    let result = { state.credentials.lock().unwrap().delete_credential(&user.tenant, &provider) };
    match result {
        Ok(true) => (StatusCode::OK, Json(json!({ "provider": provider, "deleted": true }))),
        Ok(false) => (StatusCode::NOT_FOUND, Json(json!({ "error": format!("no credential stored for provider {provider:?}") }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    }
}

/// Personal-connection layer: `GET/POST/DELETE /integrations/me[/{provider}]`,
/// self-scoped by the caller's own `user.id` -- **no capability check**,
/// unlike the org-wide `/integrations*` routes above. Every member always
/// manages their own personal connections regardless of role, the same way
/// a `viewer` can still generate their own personal API key
/// (`api_keys.personal` in `PERMISSIONS_MATRIX`) without any team
/// capability. Backend stays provider-agnostic (any provider string
/// accepted, matching the org-wide routes); the Profile UI only ships a
/// Linear card for this pass, so no backend change is needed to add a
/// second provider later.
async fn list_my_integrations(State(state): State<AccountsState>, axum::Extension(user): axum::Extension<User>) -> (StatusCode, Json<serde_json::Value>) {
    let credentials = state.credentials.lock().unwrap();
    let listed = credentials.list_user_credentials(&user.tenant, user.id).unwrap_or_default();
    let views: Vec<CredentialView> = listed
        .into_iter()
        .map(|c| CredentialView { provider: c.provider, auth_type: match c.auth_type { AuthType::ApiKey => "api_key", AuthType::OAuth => "oauth" }, created_at: c.created_at, updated_at: c.updated_at })
        .collect();
    (StatusCode::OK, Json(serde_json::to_value(views).unwrap()))
}

async fn store_my_integration(
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
        credentials.store_user_credential(
            state.secrets.as_ref(),
            &user.tenant,
            user.id,
            NewCredential { provider: &provider, auth_type, secret: &req.secret, refresh_token: req.refresh_token.as_deref(), expires_at: req.expires_at.as_deref() },
        )
    };
    match result {
        Ok(()) => (StatusCode::OK, Json(json!({ "provider": provider, "stored": true }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    }
}

async fn delete_my_integration(State(state): State<AccountsState>, axum::Extension(user): axum::Extension<User>, AxumPath(provider): AxumPath<String>) -> (StatusCode, Json<serde_json::Value>) {
    let result = { state.credentials.lock().unwrap().delete_user_credential(&user.tenant, user.id, &provider) };
    match result {
        Ok(true) => (StatusCode::OK, Json(json!({ "provider": provider, "deleted": true }))),
        Ok(false) => (StatusCode::NOT_FOUND, Json(json!({ "error": format!("no personal credential stored for provider {provider:?}") }))),
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
        let teams = TeamStore::open_in_memory().unwrap();
        build_accounts_integrations_router(accounts, credentials, secrets, teams, Some("http://localhost:3000".to_string()))
    }

    async fn body_json(response: axum::response::Response) -> serde_json::Value {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn bootstrap_status_flips_has_accounts_true_after_the_first_signup() {
        let app = test_router();

        let before = app.clone().oneshot(HttpRequest::get("/auth/bootstrap-status").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(before.status(), StatusCode::OK);
        let before_body = body_json(before).await;
        assert_eq!(before_body["has_accounts"], false);
        assert_eq!(before_body["signup_open"], true);

        app.clone()
            .oneshot(HttpRequest::post("/auth/signup").header("content-type", "application/json").body(Body::from(r#"{"email":"dev@example.com","password":"pw","first_name":"Ada","last_name":"Lovelace"}"#)).unwrap())
            .await
            .unwrap();

        let after = app.oneshot(HttpRequest::get("/auth/bootstrap-status").body(Body::empty()).unwrap()).await.unwrap();
        let after_body = body_json(after).await;
        assert_eq!(after_body["has_accounts"], true);
    }

    #[tokio::test]
    async fn signup_with_an_invalid_invite_token_is_rejected() {
        let app = test_router();

        let response = app
            .oneshot(
                HttpRequest::post("/auth/signup")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"email":"dev@example.com","password":"pw","first_name":"Ada","last_name":"Lovelace","invite_token":"ao_not-a-real-token"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn signup_allowed_matches_the_documented_gating_rules() {
        // open mode: always allowed, with or without accounts/invite.
        assert!(signup_allowed("open", true, false));
        assert!(signup_allowed("open", false, false));
        // first-user-only: fine until an account exists, then requires an
        // invite token.
        assert!(signup_allowed("first-user-only", false, false));
        assert!(!signup_allowed("first-user-only", true, false));
        assert!(signup_allowed("first-user-only", true, true));
    }

    #[tokio::test]
    async fn bootstrap_config_rejects_invalid_config_with_the_validation_errors() {
        let app = test_router();
        let response = app
            .oneshot(HttpRequest::post("/bootstrap/config").header("content-type", "application/json").body(Body::from(r#"{"secrets_master_key":"too-short"}"#)).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = body_json(response).await;
        assert!(body["errors"].as_array().unwrap().iter().any(|e| e.as_str().unwrap().contains("secrets_master_key")));
    }

    #[tokio::test]
    async fn bootstrap_config_is_rejected_once_an_account_already_exists() {
        let app = test_router();
        app.clone()
            .oneshot(HttpRequest::post("/auth/signup").header("content-type", "application/json").body(Body::from(r#"{"email":"dev@example.com","password":"pw","first_name":"Ada","last_name":"Lovelace"}"#)).unwrap())
            .await
            .unwrap();

        let key = "ab".repeat(32);
        let response = app
            .oneshot(HttpRequest::post("/bootstrap/config").header("content-type", "application/json").body(Body::from(format!(r#"{{"secrets_master_key":"{key}"}}"#))).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn a_valid_invite_token_is_accepted_as_proof_but_never_consumed_by_signup_itself() {
        let accounts = AccountStore::open_in_memory().unwrap();
        let credentials = CredentialStore::open_in_memory().unwrap();
        let secrets: Arc<dyn SecretsProvider + Send + Sync> = Arc::new(EnvSecretsProvider::from_hex(&"ab".repeat(32)).unwrap());
        let teams = TeamStore::open_in_memory().unwrap();
        let (_, raw_token) = teams.create_invite("tenant-a", "dev@example.com", "member", None, 1).unwrap();
        let app = build_accounts_integrations_router(accounts, credentials, secrets, teams, Some("http://localhost:3000".to_string()));

        let response = app
            .oneshot(
                HttpRequest::post("/auth/signup")
                    .header("content-type", "application/json")
                    .body(Body::from(format!(r#"{{"email":"dev@example.com","password":"pw","first_name":"Ada","last_name":"Lovelace","invite_token":"{raw_token}"}}"#)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED, "a valid invite token must let signup through");
    }

    #[tokio::test]
    async fn signup_then_using_the_session_token_to_list_integrations_works() {
        let app = test_router();

        let signup_response = app
            .clone()
            .oneshot(HttpRequest::post("/auth/signup").header("content-type", "application/json").body(Body::from(r#"{"email":"dev@example.com","password":"correct horse battery staple","first_name":"Ada","last_name":"Lovelace"}"#)).unwrap())
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

    /// A non-admin member (real member, not the auto-admin-backfilled sole
    /// user of their own tenant) must be blocked from every org-wide
    /// integrations action -- least-privilege posture matching
    /// `/team/audit-log`. Built manually rather than via `test_router()`
    /// since this needs direct access to `teams`/`accounts` to seed a
    /// second, non-admin member before the router takes ownership of them.
    #[tokio::test]
    async fn non_admin_member_cannot_read_or_write_the_org_wide_integration_vault() {
        let accounts = AccountStore::open_in_memory().unwrap();
        let admin = accounts.signup(NewAccount { email: "admin@example.com", password: "correct horse battery staple", first_name: "Ada", last_name: "Lovelace" }).unwrap().0;
        let (member, member_token) = accounts.signup(NewAccount { email: "member@example.com", password: "correct horse battery staple", first_name: "Bob", last_name: "Builder" }).unwrap();
        let teams = TeamStore::open_in_memory().unwrap();
        teams.add_member(&admin.tenant, admin.id, "admin").unwrap();
        teams.add_member(&admin.tenant, member.id, "member").unwrap();
        accounts.switch_tenant(member.id, &admin.tenant).unwrap();

        let credentials = CredentialStore::open_in_memory().unwrap();
        let secrets: Arc<dyn SecretsProvider + Send + Sync> = Arc::new(EnvSecretsProvider::from_hex(&"ab".repeat(32)).unwrap());
        let app = build_accounts_integrations_router(accounts, credentials, secrets, teams, Some("http://localhost:3000".to_string()));

        let list = app.clone().oneshot(HttpRequest::get("/integrations").header("authorization", format!("Bearer {member_token}")).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(list.status(), StatusCode::FORBIDDEN, "even read access is admin-only");

        let store = app
            .clone()
            .oneshot(
                HttpRequest::post("/integrations/linear")
                    .header("authorization", format!("Bearer {member_token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"auth_type":"api_key","secret":"lin_api_x"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(store.status(), StatusCode::FORBIDDEN);

        let delete = app.oneshot(HttpRequest::delete("/integrations/linear").header("authorization", format!("Bearer {member_token}")).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(delete.status(), StatusCode::FORBIDDEN);
    }

    /// A non-admin member is blocked from the org-wide vault (previous
    /// test) but must still fully manage their *own* personal connection --
    /// no capability check at all on `/integrations/me*`, matching
    /// `api_keys.personal`'s "every member always has this" posture.
    #[tokio::test]
    async fn non_admin_member_can_freely_manage_their_own_personal_integration() {
        let accounts = AccountStore::open_in_memory().unwrap();
        let admin = accounts.signup(NewAccount { email: "admin@example.com", password: "correct horse battery staple", first_name: "Ada", last_name: "Lovelace" }).unwrap().0;
        let (member, member_token) = accounts.signup(NewAccount { email: "member@example.com", password: "correct horse battery staple", first_name: "Bob", last_name: "Builder" }).unwrap();
        let teams = TeamStore::open_in_memory().unwrap();
        teams.add_member(&admin.tenant, admin.id, "admin").unwrap();
        teams.add_member(&admin.tenant, member.id, "member").unwrap();
        accounts.switch_tenant(member.id, &admin.tenant).unwrap();

        let credentials = CredentialStore::open_in_memory().unwrap();
        let secrets: Arc<dyn SecretsProvider + Send + Sync> = Arc::new(EnvSecretsProvider::from_hex(&"ab".repeat(32)).unwrap());
        let app = build_accounts_integrations_router(accounts, credentials, secrets, teams, Some("http://localhost:3000".to_string()));

        let list_before = app.clone().oneshot(HttpRequest::get("/integrations/me").header("authorization", format!("Bearer {member_token}")).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(list_before.status(), StatusCode::OK, "no capability check for the personal layer");
        assert_eq!(body_json(list_before).await, serde_json::json!([]));

        let store = app
            .clone()
            .oneshot(
                HttpRequest::post("/integrations/me/linear")
                    .header("authorization", format!("Bearer {member_token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"auth_type":"api_key","secret":"my-personal-linear-key"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(store.status(), StatusCode::OK);

        let list_after = app.clone().oneshot(HttpRequest::get("/integrations/me").header("authorization", format!("Bearer {member_token}")).body(Body::empty()).unwrap()).await.unwrap();
        let listed = body_json(list_after).await;
        assert_eq!(listed.as_array().unwrap().len(), 1);
        assert_eq!(listed[0]["provider"], "linear");

        let delete = app.oneshot(HttpRequest::delete("/integrations/me/linear").header("authorization", format!("Bearer {member_token}")).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(delete.status(), StatusCode::OK);
    }

    /// Two members' personal connections must stay isolated from each
    /// other and from the org-wide vault -- the HTTP-layer counterpart to
    /// `agentops-integrations`' own cryptographic-isolation test.
    #[tokio::test]
    async fn two_members_personal_integrations_stay_isolated_from_each_other_and_the_org_vault() {
        let accounts = AccountStore::open_in_memory().unwrap();
        let (admin, admin_token) = accounts.signup(NewAccount { email: "admin@example.com", password: "correct horse battery staple", first_name: "Ada", last_name: "Lovelace" }).unwrap();
        let (alice, alice_token) = accounts.signup(NewAccount { email: "alice@example.com", password: "correct horse battery staple", first_name: "Alice", last_name: "A" }).unwrap();
        let (bob, bob_token) = accounts.signup(NewAccount { email: "bob@example.com", password: "correct horse battery staple", first_name: "Bob", last_name: "B" }).unwrap();
        let teams = TeamStore::open_in_memory().unwrap();
        teams.add_member(&admin.tenant, admin.id, "admin").unwrap();
        teams.add_member(&admin.tenant, alice.id, "member").unwrap();
        teams.add_member(&admin.tenant, bob.id, "member").unwrap();
        accounts.switch_tenant(alice.id, &admin.tenant).unwrap();
        accounts.switch_tenant(bob.id, &admin.tenant).unwrap();

        let credentials = CredentialStore::open_in_memory().unwrap();
        let secrets: Arc<dyn SecretsProvider + Send + Sync> = Arc::new(EnvSecretsProvider::from_hex(&"ab".repeat(32)).unwrap());
        let app = build_accounts_integrations_router(accounts, credentials, secrets, teams, Some("http://localhost:3000".to_string()));

        // The admin sets the org-wide credential.
        app.clone()
            .oneshot(
                HttpRequest::post("/integrations/linear")
                    .header("authorization", format!("Bearer {admin_token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"auth_type":"api_key","secret":"org-wide-key"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        // Alice and Bob each set their own personal credential.
        for (token, secret) in [(&alice_token, "alice-key"), (&bob_token, "bob-key")] {
            let resp = app
                .clone()
                .oneshot(
                    HttpRequest::post("/integrations/me/linear")
                        .header("authorization", format!("Bearer {token}"))
                        .header("content-type", "application/json")
                        .body(Body::from(format!(r#"{{"auth_type":"api_key","secret":"{secret}"}}"#)))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
        }

        // Each only ever sees their own single personal connection -- not
        // each other's, and not the org-wide one leaking in as "personal".
        for token in [&alice_token, &bob_token] {
            let list = app.clone().oneshot(HttpRequest::get("/integrations/me").header("authorization", format!("Bearer {token}")).body(Body::empty()).unwrap()).await.unwrap();
            let listed = body_json(list).await;
            assert_eq!(listed.as_array().unwrap().len(), 1, "must see exactly their own personal connection, not zero, not more");
        }
    }

    #[tokio::test]
    async fn signup_returns_first_and_last_name_in_the_user_view() {
        let app = test_router();
        let response = app
            .oneshot(HttpRequest::post("/auth/signup").header("content-type", "application/json").body(Body::from(r#"{"email":"dev@example.com","password":"pw","first_name":"Ada","last_name":"Lovelace"}"#)).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = body_json(response).await;
        assert_eq!(body["user"]["first_name"], "Ada");
        assert_eq!(body["user"]["last_name"], "Lovelace");
    }

    #[tokio::test]
    async fn signup_rejects_a_blank_first_or_last_name() {
        let app = test_router();
        let response = app
            .clone()
            .oneshot(HttpRequest::post("/auth/signup").header("content-type", "application/json").body(Body::from(r#"{"email":"dev@example.com","password":"pw","first_name":"  ","last_name":"Lovelace"}"#)).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let response = app
            .oneshot(HttpRequest::post("/auth/signup").header("content-type", "application/json").body(Body::from(r#"{"email":"dev2@example.com","password":"pw","first_name":"Ada","last_name":""}"#)).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
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
            app.clone().oneshot(HttpRequest::post("/auth/signup").header("content-type", "application/json").body(Body::from(r#"{"email":"dev@example.com","password":"pw","first_name":"Ada","last_name":"Lovelace"}"#)).unwrap()).await.unwrap();
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
            let response = app.oneshot(HttpRequest::post("/auth/signup").header("content-type", "application/json").body(Body::from(format!(r#"{{"email":"{email}","password":"pw","first_name":"Ada","last_name":"Lovelace"}}"#))).unwrap()).await.unwrap();
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
    async fn get_auth_me_with_a_valid_session_returns_that_user() {
        let app = test_router();
        let signup_response =
            app.clone().oneshot(HttpRequest::post("/auth/signup").header("content-type", "application/json").body(Body::from(r#"{"email":"dev@example.com","password":"pw","first_name":"Ada","last_name":"Lovelace"}"#)).unwrap()).await.unwrap();
        let signed_up = body_json(signup_response).await;
        let token = signed_up["session_token"].as_str().unwrap().to_string();

        let me_response = app.oneshot(HttpRequest::get("/auth/me").header("authorization", format!("Bearer {token}")).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(me_response.status(), StatusCode::OK);
        let me = body_json(me_response).await;
        assert_eq!(me["email"], "dev@example.com");
        assert_eq!(me, signed_up["user"]);
    }

    #[tokio::test]
    async fn get_auth_me_with_no_session_token_is_rejected() {
        let app = test_router();
        let response = app.oneshot(HttpRequest::get("/auth/me").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn post_auth_logout_then_reusing_the_same_token_is_rejected() {
        let app = test_router();
        let signup_response =
            app.clone().oneshot(HttpRequest::post("/auth/signup").header("content-type", "application/json").body(Body::from(r#"{"email":"dev@example.com","password":"pw","first_name":"Ada","last_name":"Lovelace"}"#)).unwrap()).await.unwrap();
        let token = body_json(signup_response).await["session_token"].as_str().unwrap().to_string();

        let logout_response = app.clone().oneshot(HttpRequest::post("/auth/logout").header("authorization", format!("Bearer {token}")).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(logout_response.status(), StatusCode::NO_CONTENT);

        let me_response = app.oneshot(HttpRequest::get("/auth/me").header("authorization", format!("Bearer {token}")).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(me_response.status(), StatusCode::UNAUTHORIZED);
    }

    /// Enrolls and confirms 2FA for a freshly-signed-up user, returning their
    /// session token and the confirmed `TOTP` (so the caller can generate
    /// valid codes for further test steps, e.g. logging back in).
    async fn signup_and_enable_2fa(app: &Router, email: &str) -> (String, totp_rs::TOTP) {
        let signup_response =
            app.clone().oneshot(HttpRequest::post("/auth/signup").header("content-type", "application/json").body(Body::from(format!(r#"{{"email":"{email}","password":"correct horse battery staple","first_name":"Ada","last_name":"Lovelace"}}"#))).unwrap()).await.unwrap();
        let token = body_json(signup_response).await["session_token"].as_str().unwrap().to_string();

        let enroll_response = app.clone().oneshot(HttpRequest::post("/auth/2fa/enroll").header("authorization", format!("Bearer {token}")).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(enroll_response.status(), StatusCode::OK);
        let enrolled = body_json(enroll_response).await;
        let secret = enrolled["secret"].as_str().unwrap().to_string();
        assert!(enrolled["qr_data_uri"].as_str().unwrap().starts_with("data:image/png;base64,"));

        let totp = totp_rs::TOTP::new(totp_rs::Algorithm::SHA1, 6, 1, 30, totp_rs::Secret::Encoded(secret).to_bytes().unwrap(), Some("AgentOps".to_string()), email.to_string()).unwrap();

        let confirm_response = app
            .clone()
            .oneshot(
                HttpRequest::post("/auth/2fa/confirm")
                    .header("authorization", format!("Bearer {token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(format!(r#"{{"code":"{}"}}"#, totp.generate_current().unwrap())))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(confirm_response.status(), StatusCode::OK);
        let confirmed = body_json(confirm_response).await;
        assert_eq!(confirmed["backup_codes"].as_array().unwrap().len(), 10);

        (token, totp)
    }

    #[tokio::test]
    async fn enroll_then_confirm_2fa_over_http_enables_it() {
        let app = test_router();
        signup_and_enable_2fa(&app, "dev@example.com").await;
        // signup_and_enable_2fa's own assertions (200s + 10 backup codes) are the coverage here.
    }

    #[tokio::test]
    async fn post_auth_login_requires_a_2fa_code_once_enabled() {
        let app = test_router();
        signup_and_enable_2fa(&app, "dev@example.com").await;

        let login_response = app.clone().oneshot(HttpRequest::post("/auth/login").header("content-type", "application/json").body(Body::from(r#"{"email":"dev@example.com","password":"correct horse battery staple"}"#)).unwrap()).await.unwrap();
        assert_eq!(login_response.status(), StatusCode::ACCEPTED);
        let body = body_json(login_response).await;
        assert_eq!(body["two_factor_required"], true);
        assert!(body["challenge_token"].as_str().is_some());
        assert!(body.get("session_token").is_none(), "no session capability may leak before the 2FA step succeeds");
    }

    #[tokio::test]
    async fn post_auth_login_2fa_completes_the_challenge_with_a_valid_code() {
        let app = test_router();
        let (_, totp) = signup_and_enable_2fa(&app, "dev@example.com").await;

        let login_response = app.clone().oneshot(HttpRequest::post("/auth/login").header("content-type", "application/json").body(Body::from(r#"{"email":"dev@example.com","password":"correct horse battery staple"}"#)).unwrap()).await.unwrap();
        let challenge_token = body_json(login_response).await["challenge_token"].as_str().unwrap().to_string();

        let bad_code_response = app
            .clone()
            .oneshot(HttpRequest::post("/auth/login/2fa").header("content-type", "application/json").body(Body::from(format!(r#"{{"challenge_token":"{challenge_token}","code":"000000"}}"#))).unwrap())
            .await
            .unwrap();
        assert_eq!(bad_code_response.status(), StatusCode::UNAUTHORIZED);

        let good_code_response = app
            .oneshot(HttpRequest::post("/auth/login/2fa").header("content-type", "application/json").body(Body::from(format!(r#"{{"challenge_token":"{challenge_token}","code":"{}"}}"#, totp.generate_current().unwrap()))).unwrap())
            .await
            .unwrap();
        assert_eq!(good_code_response.status(), StatusCode::OK);
        let body = body_json(good_code_response).await;
        assert_eq!(body["user"]["email"], "dev@example.com");
        assert!(body["session_token"].as_str().is_some());
    }

    #[tokio::test]
    async fn post_auth_2fa_disable_requires_the_password() {
        let app = test_router();
        let (token, _) = signup_and_enable_2fa(&app, "dev@example.com").await;

        let wrong_password = app
            .clone()
            .oneshot(HttpRequest::post("/auth/2fa/disable").header("authorization", format!("Bearer {token}")).header("content-type", "application/json").body(Body::from(r#"{"password":"wrong"}"#)).unwrap())
            .await
            .unwrap();
        assert_eq!(wrong_password.status(), StatusCode::BAD_REQUEST);

        let right_password = app
            .clone()
            .oneshot(
                HttpRequest::post("/auth/2fa/disable")
                    .header("authorization", format!("Bearer {token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"password":"correct horse battery staple"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(right_password.status(), StatusCode::OK);

        // Login no longer requires a 2FA step.
        let login_response = app.oneshot(HttpRequest::post("/auth/login").header("content-type", "application/json").body(Body::from(r#"{"email":"dev@example.com","password":"correct horse battery staple"}"#)).unwrap()).await.unwrap();
        assert_eq!(login_response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn get_auth_sessions_lists_the_current_session_marked_is_current() {
        let app = test_router();
        let signup_response =
            app.clone().oneshot(HttpRequest::post("/auth/signup").header("content-type", "application/json").header("user-agent", "TestBrowser/1.0").body(Body::from(r#"{"email":"dev@example.com","password":"pw","first_name":"Ada","last_name":"Lovelace"}"#)).unwrap()).await.unwrap();
        let token = body_json(signup_response).await["session_token"].as_str().unwrap().to_string();

        let response = app.oneshot(HttpRequest::get("/auth/sessions").header("authorization", format!("Bearer {token}")).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let sessions = body_json(response).await;
        assert_eq!(sessions.as_array().unwrap().len(), 1);
        assert_eq!(sessions[0]["user_agent"], "TestBrowser/1.0");
        assert_eq!(sessions[0]["is_current"], true);
    }

    #[tokio::test]
    async fn delete_auth_sessions_by_id_revokes_it() {
        let app = test_router();
        let signup_response =
            app.clone().oneshot(HttpRequest::post("/auth/signup").header("content-type", "application/json").body(Body::from(r#"{"email":"dev@example.com","password":"pw","first_name":"Ada","last_name":"Lovelace"}"#)).unwrap()).await.unwrap();
        let first_token = body_json(signup_response).await["session_token"].as_str().unwrap().to_string();

        let login_response = app.clone().oneshot(HttpRequest::post("/auth/login").header("content-type", "application/json").body(Body::from(r#"{"email":"dev@example.com","password":"pw"}"#)).unwrap()).await.unwrap();
        let second_token = body_json(login_response).await["session_token"].as_str().unwrap().to_string();

        let sessions_response = app.clone().oneshot(HttpRequest::get("/auth/sessions").header("authorization", format!("Bearer {second_token}")).body(Body::empty()).unwrap()).await.unwrap();
        let sessions = body_json(sessions_response).await;
        let other_session_id = sessions.as_array().unwrap().iter().find(|s| s["is_current"] != true).unwrap()["id"].as_i64().unwrap();

        let delete_response = app.clone().oneshot(HttpRequest::delete(format!("/auth/sessions/{other_session_id}")).header("authorization", format!("Bearer {second_token}")).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(delete_response.status(), StatusCode::OK);

        let me_response = app.oneshot(HttpRequest::get("/auth/me").header("authorization", format!("Bearer {first_token}")).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(me_response.status(), StatusCode::UNAUTHORIZED, "the revoked session must stop working");
    }

    #[tokio::test]
    async fn post_auth_sessions_revoke_others_keeps_only_the_caller() {
        let app = test_router();
        let signup_response =
            app.clone().oneshot(HttpRequest::post("/auth/signup").header("content-type", "application/json").body(Body::from(r#"{"email":"dev@example.com","password":"pw","first_name":"Ada","last_name":"Lovelace"}"#)).unwrap()).await.unwrap();
        let first_token = body_json(signup_response).await["session_token"].as_str().unwrap().to_string();

        let login_response = app.clone().oneshot(HttpRequest::post("/auth/login").header("content-type", "application/json").body(Body::from(r#"{"email":"dev@example.com","password":"pw"}"#)).unwrap()).await.unwrap();
        let second_token = body_json(login_response).await["session_token"].as_str().unwrap().to_string();

        let response = app.clone().oneshot(HttpRequest::post("/auth/sessions/revoke-others").header("authorization", format!("Bearer {second_token}")).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let first_me = app.clone().oneshot(HttpRequest::get("/auth/me").header("authorization", format!("Bearer {first_token}")).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(first_me.status(), StatusCode::UNAUTHORIZED);

        let second_me = app.oneshot(HttpRequest::get("/auth/me").header("authorization", format!("Bearer {second_token}")).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(second_me.status(), StatusCode::OK, "the caller's own session must survive");
    }

    #[tokio::test]
    async fn cannot_revoke_another_users_session_by_guessing_its_id() {
        let app = test_router();
        let signup_a =
            app.clone().oneshot(HttpRequest::post("/auth/signup").header("content-type", "application/json").body(Body::from(r#"{"email":"a@example.com","password":"pw","first_name":"Ada","last_name":"Lovelace"}"#)).unwrap()).await.unwrap();
        let token_a = body_json(signup_a).await["session_token"].as_str().unwrap().to_string();
        let signup_b =
            app.clone().oneshot(HttpRequest::post("/auth/signup").header("content-type", "application/json").body(Body::from(r#"{"email":"b@example.com","password":"pw","first_name":"Grace","last_name":"Hopper"}"#)).unwrap()).await.unwrap();
        let token_b = body_json(signup_b).await["session_token"].as_str().unwrap().to_string();

        let sessions_a = app.clone().oneshot(HttpRequest::get("/auth/sessions").header("authorization", format!("Bearer {token_a}")).body(Body::empty()).unwrap()).await.unwrap();
        let session_a_id = body_json(sessions_a).await[0]["id"].as_i64().unwrap();

        let delete_response = app.clone().oneshot(HttpRequest::delete(format!("/auth/sessions/{session_a_id}")).header("authorization", format!("Bearer {token_b}")).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(delete_response.status(), StatusCode::NOT_FOUND);

        let me_a = app.oneshot(HttpRequest::get("/auth/me").header("authorization", format!("Bearer {token_a}")).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(me_a.status(), StatusCode::OK, "a's session must survive b's attempt");
    }

    #[tokio::test]
    async fn post_auth_me_password_changes_it_and_revokes_other_sessions() {
        let app = test_router();
        let signup_response =
            app.clone().oneshot(HttpRequest::post("/auth/signup").header("content-type", "application/json").body(Body::from(r#"{"email":"dev@example.com","password":"old password","first_name":"Ada","last_name":"Lovelace"}"#)).unwrap()).await.unwrap();
        let first_token = body_json(signup_response).await["session_token"].as_str().unwrap().to_string();

        let login_response = app.clone().oneshot(HttpRequest::post("/auth/login").header("content-type", "application/json").body(Body::from(r#"{"email":"dev@example.com","password":"old password"}"#)).unwrap()).await.unwrap();
        let second_token = body_json(login_response).await["session_token"].as_str().unwrap().to_string();

        let response = app
            .clone()
            .oneshot(
                HttpRequest::post("/auth/me/password")
                    .header("authorization", format!("Bearer {second_token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"current_password":"old password","new_password":"new password"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // The session that changed the password survives...
        let second_me = app.clone().oneshot(HttpRequest::get("/auth/me").header("authorization", format!("Bearer {second_token}")).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(second_me.status(), StatusCode::OK);
        // ...but every other session was revoked by the change.
        let first_me = app.clone().oneshot(HttpRequest::get("/auth/me").header("authorization", format!("Bearer {first_token}")).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(first_me.status(), StatusCode::UNAUTHORIZED);

        // And a fresh login now requires the new password.
        let relogin = app.oneshot(HttpRequest::post("/auth/login").header("content-type", "application/json").body(Body::from(r#"{"email":"dev@example.com","password":"new password"}"#)).unwrap()).await.unwrap();
        assert_eq!(relogin.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn post_auth_me_password_rejects_the_wrong_current_password() {
        let app = test_router();
        let signup_response =
            app.clone().oneshot(HttpRequest::post("/auth/signup").header("content-type", "application/json").body(Body::from(r#"{"email":"dev@example.com","password":"old password","first_name":"Ada","last_name":"Lovelace"}"#)).unwrap()).await.unwrap();
        let token = body_json(signup_response).await["session_token"].as_str().unwrap().to_string();

        let response = app
            .oneshot(
                HttpRequest::post("/auth/me/password")
                    .header("authorization", format!("Bearer {token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"current_password":"totally wrong","new_password":"new password"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn post_auth_api_keys_returns_the_raw_key_once_then_list_only_shows_the_prefix() {
        let app = test_router();
        let signup_response =
            app.clone().oneshot(HttpRequest::post("/auth/signup").header("content-type", "application/json").body(Body::from(r#"{"email":"dev@example.com","password":"pw","first_name":"Ada","last_name":"Lovelace"}"#)).unwrap()).await.unwrap();
        let token = body_json(signup_response).await["session_token"].as_str().unwrap().to_string();

        let create_response = app
            .clone()
            .oneshot(
                HttpRequest::post("/auth/api-keys")
                    .header("authorization", format!("Bearer {token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"CI / CD Pipeline"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(create_response.status(), StatusCode::CREATED);
        let created = body_json(create_response).await;
        let raw_key = created["key"].as_str().unwrap().to_string();
        assert!(raw_key.starts_with("ao_"));

        let list_response = app.oneshot(HttpRequest::get("/auth/api-keys").header("authorization", format!("Bearer {token}")).body(Body::empty()).unwrap()).await.unwrap();
        let listed = body_json(list_response).await;
        assert_eq!(listed[0]["name"], "CI / CD Pipeline");
        assert!(!listed.to_string().contains(&raw_key), "the raw key must never appear in a list response");
    }

    #[tokio::test]
    async fn delete_auth_api_keys_by_id_is_scoped_to_the_owning_user() {
        let app = test_router();
        let signup_a =
            app.clone().oneshot(HttpRequest::post("/auth/signup").header("content-type", "application/json").body(Body::from(r#"{"email":"a@example.com","password":"pw","first_name":"Ada","last_name":"Lovelace"}"#)).unwrap()).await.unwrap();
        let token_a = body_json(signup_a).await["session_token"].as_str().unwrap().to_string();
        let signup_b =
            app.clone().oneshot(HttpRequest::post("/auth/signup").header("content-type", "application/json").body(Body::from(r#"{"email":"b@example.com","password":"pw","first_name":"Grace","last_name":"Hopper"}"#)).unwrap()).await.unwrap();
        let token_b = body_json(signup_b).await["session_token"].as_str().unwrap().to_string();

        let created = app
            .clone()
            .oneshot(HttpRequest::post("/auth/api-keys").header("authorization", format!("Bearer {token_a}")).header("content-type", "application/json").body(Body::from(r#"{"name":"Local dev"}"#)).unwrap())
            .await
            .unwrap();
        let key_id = body_json(created).await["id"].as_i64().unwrap();

        let delete_by_b = app.clone().oneshot(HttpRequest::delete(format!("/auth/api-keys/{key_id}")).header("authorization", format!("Bearer {token_b}")).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(delete_by_b.status(), StatusCode::NOT_FOUND);

        let delete_by_a = app.oneshot(HttpRequest::delete(format!("/auth/api-keys/{key_id}")).header("authorization", format!("Bearer {token_a}")).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(delete_by_a.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn patch_auth_me_updates_only_the_fields_sent() {
        let app = test_router();
        let signup_response =
            app.clone().oneshot(HttpRequest::post("/auth/signup").header("content-type", "application/json").body(Body::from(r#"{"email":"dev@example.com","password":"pw","first_name":"Ada","last_name":"Lovelace"}"#)).unwrap()).await.unwrap();
        let token = body_json(signup_response).await["session_token"].as_str().unwrap().to_string();

        let response = app
            .clone()
            .oneshot(
                HttpRequest::patch("/auth/me")
                    .header("authorization", format!("Bearer {token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"bio":"Staff engineer","location":"San Francisco, CA"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["bio"], "Staff engineer");
        assert_eq!(body["location"], "San Francisco, CA");
        assert_eq!(body["first_name"], "Ada", "fields not sent in the PATCH must be untouched");
    }

    #[tokio::test]
    async fn patch_auth_me_preferences_updates_only_the_fields_sent() {
        let app = test_router();
        let signup_response =
            app.clone().oneshot(HttpRequest::post("/auth/signup").header("content-type", "application/json").body(Body::from(r#"{"email":"dev@example.com","password":"pw","first_name":"Ada","last_name":"Lovelace"}"#)).unwrap()).await.unwrap();
        let token = body_json(signup_response).await["session_token"].as_str().unwrap().to_string();

        let response = app
            .oneshot(
                HttpRequest::patch("/auth/me/preferences")
                    .header("authorization", format!("Bearer {token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"show_gotcha_callouts":false}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["show_gotcha_callouts"], false);
        assert_eq!(body["theme_pref"], "dark", "fields not sent in the PATCH must be untouched");
    }

    #[tokio::test]
    async fn post_auth_me_complete_onboarding_flips_the_flag() {
        let app = test_router();
        let signup_response =
            app.clone().oneshot(HttpRequest::post("/auth/signup").header("content-type", "application/json").body(Body::from(r#"{"email":"dev@example.com","password":"pw","first_name":"Ada","last_name":"Lovelace"}"#)).unwrap()).await.unwrap();
        let signup_body = body_json(signup_response).await;
        assert_eq!(signup_body["user"]["onboarding_completed"], false, "not complete right after signup");
        let token = signup_body["session_token"].as_str().unwrap().to_string();

        let response = app.oneshot(HttpRequest::post("/auth/me/complete-onboarding").header("authorization", format!("Bearer {token}")).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["onboarding_completed"], true);
    }

    #[tokio::test]
    async fn oauth_endpoints_report_not_implemented_rather_than_404() {
        let app = test_router();
        let signup_response =
            app.clone().oneshot(HttpRequest::post("/auth/signup").header("content-type", "application/json").body(Body::from(r#"{"email":"dev@example.com","password":"pw","first_name":"Ada","last_name":"Lovelace"}"#)).unwrap()).await.unwrap();
        let token = body_json(signup_response).await["session_token"].as_str().unwrap().to_string();

        let response = app.oneshot(HttpRequest::get("/integrations/linear/oauth/start").header("authorization", format!("Bearer {token}")).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    }

    #[tokio::test]
    async fn device_auth_flow_round_trips_from_device_start_through_approve_to_a_real_api_key() {
        let app = test_router();

        // CLI: request a device code.
        let start_response = app.clone().oneshot(HttpRequest::post("/auth/cli/device").header("content-type", "application/json").body(Body::from(r#"{"device_name":"Jesus's MacBook"}"#)).unwrap()).await.unwrap();
        assert_eq!(start_response.status(), StatusCode::OK);
        let start_body = body_json(start_response).await;
        assert!(start_body["verification_uri_complete"].as_str().unwrap().starts_with("http://localhost:3000/cli-auth?code="));
        let device_code = start_body["device_code"].as_str().unwrap().to_string();
        let user_code = start_body["user_code"].as_str().unwrap().to_string();

        // CLI: polling before approval reports authorization_pending.
        let pending = app.clone().oneshot(HttpRequest::post("/auth/cli/device/token").header("content-type", "application/json").body(Body::from(json!({ "device_code": device_code }).to_string())).unwrap()).await.unwrap();
        assert_eq!(body_json(pending).await["error"], "authorization_pending");

        // Browser: the person logs in, then approves.
        let signup_response =
            app.clone().oneshot(HttpRequest::post("/auth/signup").header("content-type", "application/json").body(Body::from(r#"{"email":"dev@example.com","password":"correct horse battery staple","first_name":"Ada","last_name":"Lovelace"}"#)).unwrap()).await.unwrap();
        let session_token = body_json(signup_response).await["session_token"].as_str().unwrap().to_string();

        let approve_response = app
            .clone()
            .oneshot(
                HttpRequest::post("/auth/cli/device/approve")
                    .header("content-type", "application/json")
                    .header("authorization", format!("Bearer {session_token}"))
                    .body(Body::from(json!({ "user_code": user_code, "action": "approve" }).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(approve_response.status(), StatusCode::OK);

        // CLI: the next poll returns a real, usable API key -- and only once.
        let approved = app.clone().oneshot(HttpRequest::post("/auth/cli/device/token").header("content-type", "application/json").body(Body::from(json!({ "device_code": device_code }).to_string())).unwrap()).await.unwrap();
        let approved_body = body_json(approved).await;
        assert_eq!(approved_body["status"], "approved");
        let api_key = approved_body["api_key"].as_str().unwrap();
        assert!(api_key.starts_with("ao_"));

        let list_keys = app.clone().oneshot(HttpRequest::get("/auth/api-keys").header("authorization", format!("Bearer {session_token}")).body(Body::empty()).unwrap()).await.unwrap();
        let keys = body_json(list_keys).await;
        assert_eq!(keys.as_array().unwrap()[0]["name"], "CLI (Jesus's MacBook)");

        let expired = app.oneshot(HttpRequest::post("/auth/cli/device/token").header("content-type", "application/json").body(Body::from(json!({ "device_code": device_code }).to_string())).unwrap()).await.unwrap();
        assert_eq!(body_json(expired).await["error"], "expired_token", "a device_code must not be pollable to \"approved\" twice");
    }

    #[tokio::test]
    async fn denying_a_device_auth_code_reports_access_denied_to_the_cli() {
        let app = test_router();
        let start_response = app.clone().oneshot(HttpRequest::post("/auth/cli/device").header("content-type", "application/json").body(Body::from(r#"{"device_name":"some device"}"#)).unwrap()).await.unwrap();
        let start_body = body_json(start_response).await;
        let device_code = start_body["device_code"].as_str().unwrap().to_string();
        let user_code = start_body["user_code"].as_str().unwrap().to_string();

        let signup_response =
            app.clone().oneshot(HttpRequest::post("/auth/signup").header("content-type", "application/json").body(Body::from(r#"{"email":"dev@example.com","password":"correct horse battery staple","first_name":"Ada","last_name":"Lovelace"}"#)).unwrap()).await.unwrap();
        let session_token = body_json(signup_response).await["session_token"].as_str().unwrap().to_string();

        app.clone()
            .oneshot(
                HttpRequest::post("/auth/cli/device/approve")
                    .header("content-type", "application/json")
                    .header("authorization", format!("Bearer {session_token}"))
                    .body(Body::from(json!({ "user_code": user_code, "action": "deny" }).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        let polled = app.oneshot(HttpRequest::post("/auth/cli/device/token").header("content-type", "application/json").body(Body::from(json!({ "device_code": device_code }).to_string())).unwrap()).await.unwrap();
        assert_eq!(body_json(polled).await["error"], "access_denied");
    }

    #[tokio::test]
    async fn approving_a_device_code_without_a_session_is_rejected() {
        let app = test_router();
        let start_response = app.clone().oneshot(HttpRequest::post("/auth/cli/device").header("content-type", "application/json").body(Body::from(r#"{"device_name":"some device"}"#)).unwrap()).await.unwrap();
        let user_code = body_json(start_response).await["user_code"].as_str().unwrap().to_string();

        let response = app
            .oneshot(HttpRequest::post("/auth/cli/device/approve").header("content-type", "application/json").body(Body::from(json!({ "user_code": user_code, "action": "approve" }).to_string())).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    /// Caught live: the API backend and the web app frontend are commonly
    /// on entirely different public domains in a real deployment -- a
    /// silently-guessed fallback URL (e.g. `http://localhost:3000`) would
    /// mint a device code pointing at a dead link with no indication
    /// anything was wrong. Must fail loudly and specifically instead.
    #[tokio::test]
    async fn device_start_reports_a_clear_error_when_web_app_url_is_not_configured() {
        let accounts = AccountStore::open_in_memory().unwrap();
        let credentials = CredentialStore::open_in_memory().unwrap();
        let secrets: Arc<dyn SecretsProvider + Send + Sync> = Arc::new(EnvSecretsProvider::from_hex(&"ab".repeat(32)).unwrap());
        let teams = TeamStore::open_in_memory().unwrap();
        let app = build_accounts_integrations_router(accounts, credentials, secrets, teams, None);

        let response = app.oneshot(HttpRequest::post("/auth/cli/device").header("content-type", "application/json").body(Body::from(r#"{"device_name":"some device"}"#)).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(body_json(response).await["error"].as_str().unwrap().contains("AGENTOPS_WEB_APP_URL"));
    }
}

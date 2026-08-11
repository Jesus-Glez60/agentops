//! Linear webhook receiver — Phase 6 (1.0+ roadmap), Module 9's core: an
//! assignment to a whitelisted person on a whitelisted Linear team
//! automatically pre-stages that repo's codebrain context (`scan_repo`)
//! and opens a session-correlated task, before the dev ever opens it.
//!
//! **Phase 6c: user-triggered enrollment, not boot-time config.** This
//! module used to load a static `AutoKickoffConfig` from a JSON file
//! (`AGENTOPS_LINEAR_AUTO_KICKOFF_CONFIG`) once at server startup, gated by
//! one global `enabled` boolean, always resolved against a hardcoded
//! `"default"` tenant. That never actually connected to Phase 7's real
//! accounts model (`POST /integrations/{provider}` always stores a
//! credential under the *signed-up user's own* randomly-generated tenant —
//! there's no way to make it "default"), so a real user adding their Linear
//! key the actual designed way would never have it found by the webhook
//! receiver. Auto-kickoff is now a per-account opt-in: `POST
//! /linear/auto-kickoff` (session-authenticated) enrolls the *calling
//! user's own tenant* via `agentops-integration-modules::ModuleStore` — the
//! standard, provider-agnostic enrollment mechanism future integrations
//! should use too, not a Linear-specific one-off.
//!
//! **Doc-pull (`sync-docs`) runs as part of dispatch, non-fatally** —
//! `agentops_mcp::sync_docs` (registry + GitHub search, no interactive
//! prompt) was extracted from `agentops-cli::main::sync_docs` specifically
//! so this unattended receiver could call it too; a failure here logs and
//! is swallowed rather than blocking the scan/task that already succeeded.
//!
//! **Secrets are per-team, not one global secret, and per-tenant now too**
//! — `webhookCreate`/`ensure_webhook_registered` operate one team at a
//! time, and Linear issues a distinct signing secret per webhook. An
//! inbound request carries no tenant hint of its own until a secret
//! matches, so the handler loads every *enabled* enrollment across every
//! tenant (`ModuleStore::list_enabled_for_module`) and tries each
//! candidate's vault-cached secret against the raw body in turn — whichever
//! one verifies is both the team *and* the tenant the request is trusted to
//! be from (Linear's payload also carries its own `data.teamId`,
//! cross-checked as a second, defense-in-depth confirmation, not the
//! primary trust boundary — the verified signature is).

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use agentops_accounts::{AccountStore, User};
use agentops_integrations::CredentialStore;
use agentops_integration_modules::ModuleStore;
use agentops_repo_access::secrets::SecretsProvider;
use anyhow::{Context, Result};
use axum::body::Bytes;
use axum::extract::{Path as AxumPath, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, post};
use axum::{Json, Router};
use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::Sha256;

const SIGNATURE_HEADER: &str = "linear-signature";
/// Reject anything older than this — replay-attack protection, matching
/// Linear's own documented verification flow (`webhookTimestamp` in the
/// JSON body, not a header).
const MAX_TIMESTAMP_AGE_MS: i64 = 60_000;
/// The `agentops-integration-modules` module name Linear auto-kickoff
/// enrolls under — the first consumer of the generic pattern, not a
/// hardcoded special case in the store itself.
const MODULE_NAME: &str = "linear_auto_kickoff";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoKickoffTeamConfig {
    pub linear_team_id: String,
    pub repo_path: PathBuf,
    /// Emails, not Linear user ids — the portable, human-editable
    /// identifier. Resolved against a webhook payload's `assigneeId` via
    /// `agentops_linear::user_email`.
    pub assignee_emails: Vec<String>,
    /// This team's own webhook signing secret. **Never serialized as part
    /// of a module's enrollment config** (`skip_serializing`) — it's
    /// custodied separately, encrypted, in `agentops-integrations::CredentialStore`
    /// under `(tenant, "linear_webhook:{team_id}")`, same as Phase 6b
    /// established. Filled in at load time by joining a `ModuleStore` row
    /// with its corresponding vault entry, never present in the enrollment
    /// config's own JSON.
    #[serde(skip_serializing, default)]
    pub webhook_secret: String,
    /// Local status string -> exact Linear workflow state name, for teams
    /// with custom states beyond the generic 5. Groundwork for when
    /// auto-kickoff itself starts pushing statuses (not yet — nothing in
    /// this module calls `push_status`/`sync_push` during dispatch today).
    #[serde(default)]
    pub status_name_map: HashMap<String, String>,
}

/// HMAC-SHA256 of the raw body against `secret`, compared to the hex-decoded
/// `Linear-Signature` header value via `Mac::verify_slice` (constant-time —
/// never a plain `==` on a MAC, that's a timing side-channel).
fn verify_signature(secret: &str, raw_body: &[u8], header_hex: &str) -> bool {
    let Ok(expected) = decode_hex(header_hex) else { return false };
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret.as_bytes()) else { return false };
    mac.update(raw_body);
    mac.verify_slice(&expected).is_ok()
}

fn decode_hex(s: &str) -> Result<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        anyhow::bail!("hex string has odd length");
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).context("invalid hex character")).collect()
}

fn is_recent(webhook_timestamp_ms: i64) -> bool {
    let now_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
    (now_ms - webhook_timestamp_ms).abs() <= MAX_TIMESTAMP_AGE_MS
}

/// Loads every *enabled* `linear_auto_kickoff` enrollment across every
/// tenant, joining each `ModuleStore` row's JSON config with its
/// corresponding cached webhook secret from the vault — a candidate with no
/// cached secret yet (registration never completed) is silently skipped,
/// not an error, since a partially-enrolled team simply can't verify
/// anything yet.
fn load_candidates(modules: &ModuleStore, credentials: &CredentialStore, secrets: &dyn SecretsProvider) -> Vec<(String, AutoKickoffTeamConfig)> {
    modules
        .list_enabled_for_module(MODULE_NAME)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(tenant, config_json)| {
            let mut team: AutoKickoffTeamConfig = serde_json::from_str(&config_json).ok()?;
            let cache_provider = format!("linear_webhook:{}", team.linear_team_id);
            let secret = credentials.get_credential(secrets, &tenant, &cache_provider).ok().flatten()?.secret.to_string();
            team.webhook_secret = secret;
            Some((tenant, team))
        })
        .collect()
}

/// Finds the first candidate whose own webhook secret verifies this
/// request — see the module doc comment for why per-team, per-tenant
/// secret matching is the actual trust boundary here, not a single shared
/// secret or a self-reported id.
fn find_verified_team(candidates: &[(String, AutoKickoffTeamConfig)], raw_body: &[u8], header_hex: &str) -> Option<(String, AutoKickoffTeamConfig)> {
    candidates.iter().find(|(_, t)| verify_signature(&t.webhook_secret, raw_body, header_hex)).cloned()
}

/// Bounded, in-memory delivery-id dedup — Linear retries undelivered
/// webhooks, and this must not double-dispatch on a retry. Deliberately
/// not durable (a process restart forgets recent deliveries): a real
/// database table for this is more machinery than a first pass needs, and
/// the practical risk is small — a restart-timed retry double-dispatches
/// `scan_repo`/`create_task`, both of which are themselves idempotent
/// (`upsert_node`/`upsert_external_task`), so the worst case is a harmless
/// extra scan, not corrupted state. Flagged as a known limitation, not
/// silently accepted.
pub struct SeenDeliveries {
    set: Mutex<(HashSet<String>, VecDeque<String>)>,
    cap: usize,
}

impl SeenDeliveries {
    pub fn new(cap: usize) -> Self {
        Self { set: Mutex::new((HashSet::new(), VecDeque::new())), cap }
    }

    /// Returns `true` if this is the first time `delivery_id` has been
    /// seen (caller should proceed), `false` if it's a repeat.
    fn record_if_new(&self, delivery_id: &str) -> bool {
        let mut guard = self.set.lock().unwrap();
        if guard.0.contains(delivery_id) {
            return false;
        }
        guard.0.insert(delivery_id.to_string());
        guard.1.push_back(delivery_id.to_string());
        if guard.1.len() > self.cap {
            if let Some(oldest) = guard.1.pop_front() {
                guard.0.remove(&oldest);
            }
        }
        true
    }
}

#[derive(Debug, Serialize)]
pub struct DispatchResult {
    pub dispatched: bool,
    pub reason: &'static str,
}

/// The actual "assignment -> pre-staged context" logic, factored out from
/// the axum handler so it's testable without spinning up HTTP — takes a
/// payload already known (via a verified signature) to belong to
/// `team_config`, and does the assignee-whitelist check + dispatch. No
/// longer takes an `enabled` flag — a request only reaches this function
/// after `find_verified_team` matched a candidate, and candidates are
/// already filtered to enabled enrollments only (`ModuleStore::list_enabled_for_module`),
/// so the equivalent guarantee now lives at the enrollment layer instead of
/// being re-checked here.
pub fn handle_verified_payload(team_config: &AutoKickoffTeamConfig, linear_config: &agentops_linear::LinearConfig, payload: &serde_json::Value) -> Result<DispatchResult> {
    if payload.get("type").and_then(|v| v.as_str()) != Some("Issue") {
        return Ok(DispatchResult { dispatched: false, reason: "not an Issue event" });
    }

    let action = payload.get("action").and_then(|v| v.as_str()).unwrap_or("");
    let assignee_changed_this_update = payload.pointer("/updatedFrom/assigneeId").is_some();
    if !(action == "create" || (action == "update" && assignee_changed_this_update)) {
        return Ok(DispatchResult { dispatched: false, reason: "not a new assignment" });
    }

    let Some(assignee_id) = payload.pointer("/data/assigneeId").and_then(|v| v.as_str()) else {
        return Ok(DispatchResult { dispatched: false, reason: "issue has no assignee" });
    };
    // Defense in depth, not the primary trust boundary (that's the
    // already-verified signature) — a payload genuinely signed by
    // `team_config`'s own secret claiming a different team id would be a
    // Linear-side inconsistency worth refusing to act on anyway.
    if payload.pointer("/data/teamId").and_then(|v| v.as_str()) != Some(team_config.linear_team_id.as_str()) {
        return Ok(DispatchResult { dispatched: false, reason: "payload team id does not match the verified webhook's team" });
    }

    let assignee_email = agentops_linear::user_email(linear_config, assignee_id)?;
    if !team_config.assignee_emails.iter().any(|e| e.eq_ignore_ascii_case(&assignee_email)) {
        return Ok(DispatchResult { dispatched: false, reason: "assignee not whitelisted" });
    }

    let identifier = payload.pointer("/data/identifier").and_then(|v| v.as_str()).ok_or_else(|| anyhow::anyhow!("Issue payload missing identifier"))?;
    let title = payload.pointer("/data/title").and_then(|v| v.as_str()).unwrap_or("(untitled)");
    let session_id = format!("linear-auto-kickoff-{identifier}");

    let repo = agentops_mcp::repo_name(&team_config.repo_path);
    agentops_mcp::scan_and_persist(&team_config.repo_path, true).context("auto-kickoff scan_repo")?;

    // Doc-pull is a soft-fail step, same discipline as `sync_candidates`'s
    // own per-dependency warning-and-continue: a flaky registry/GitHub
    // lookup must not block the task/scan that already succeeded above.
    if let Err(e) = agentops_mcp::sync_docs(&team_config.repo_path, None) {
        eprintln!("auto-kickoff sync_docs failed (non-fatal): {e:#}");
    }

    let store = agentops_mcp::open_store(&team_config.repo_path).context("auto-kickoff open_store")?;
    store
        .upsert_external_task(agentops_graph::NewTask {
            repo,
            title: title.to_string(),
            description: payload.pointer("/data/description").and_then(|v| v.as_str()).map(String::from),
            status: agentops_graph::TaskStatus::Todo,
            priority: None,
            assignee: Some(assignee_email),
            external_source: Some("linear".to_string()),
            external_id: Some(identifier.to_string()),
            session_id: Some(session_id),
        })
        .context("auto-kickoff upsert_external_task")?;

    Ok(DispatchResult { dispatched: true, reason: "context pre-staged" })
}

#[derive(Clone)]
struct WebhookState {
    modules: Arc<Mutex<ModuleStore>>,
    credentials: Arc<Mutex<CredentialStore>>,
    secrets: Arc<dyn SecretsProvider + Send + Sync>,
    seen: Arc<SeenDeliveries>,
}

async fn linear_webhook_handler(State(state): State<Arc<WebhookState>>, headers: HeaderMap, body: Bytes) -> impl IntoResponse {
    let Some(signature) = headers.get(SIGNATURE_HEADER).and_then(|v| v.to_str().ok()) else {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "missing Linear-Signature header" }))).into_response();
    };

    let candidates = {
        let modules = state.modules.lock().unwrap();
        let credentials = state.credentials.lock().unwrap();
        load_candidates(&modules, &credentials, state.secrets.as_ref())
    };
    let Some((tenant, team_config)) = find_verified_team(&candidates, &body, signature) else {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "invalid signature" }))).into_response();
    };

    let payload: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return (StatusCode::BAD_REQUEST, Json(json!({ "error": "invalid JSON body" }))).into_response(),
    };

    let Some(webhook_timestamp) = payload.get("webhookTimestamp").and_then(|v| v.as_i64()) else {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "missing webhookTimestamp" }))).into_response();
    };
    if !is_recent(webhook_timestamp) {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "webhookTimestamp too old — possible replay" }))).into_response();
    }

    if let Some(delivery_id) = headers.get("linear-delivery").and_then(|v| v.to_str().ok()) {
        if !state.seen.record_if_new(delivery_id) {
            return (StatusCode::OK, Json(json!({ "dispatched": false, "reason": "duplicate delivery" }))).into_response();
        }
    }

    let linear_config = {
        let credentials = state.credentials.lock().unwrap();
        match crate::resolve_linear_config(&credentials, state.secrets.as_ref(), &tenant) {
            Ok(c) => c,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
        }
    };

    match handle_verified_payload(&team_config, &linear_config, &payload) {
        Ok(result) => {
            // A real gap this pass's own live test caught: with no
            // success-path log line, the only way to see a dispatch happen
            // was a separate HTTP-traffic inspector (ngrok's), not this
            // process's own log — an operator watching `journalctl`/stdout
            // in production would see nothing on the happy path either.
            println!("linear webhook: dispatched={} reason={:?} tenant={} team={}", result.dispatched, result.reason, tenant, team_config.linear_team_id);
            (StatusCode::OK, Json(result)).into_response()
        }
        Err(e) => {
            eprintln!("linear webhook dispatch failed: {e:#}");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response()
        }
    }
}

pub fn merge_linear_webhook_route(app: Router, modules: Arc<Mutex<ModuleStore>>, credentials: Arc<Mutex<CredentialStore>>, secrets: Arc<dyn SecretsProvider + Send + Sync>) -> Router {
    let state = Arc::new(WebhookState { modules, credentials, secrets, seen: Arc::new(SeenDeliveries::new(1000)) });
    let webhook_router = Router::new().route("/webhooks/linear", post(linear_webhook_handler)).with_state(state);
    app.merge(webhook_router)
}

/// Shared state for every session-authenticated `/linear/*` route this
/// module owns (`/linear/auto-kickoff*`, `/linear/tasks/*`) — a second,
/// independent connection to the same `integrations.sqlite`/`accounts.sqlite`
/// files `run()` already opened for `/auth`/`/integrations`, matching the
/// existing precedent that each router module opens its own store
/// independently (`build_router` already gets its own separate
/// `ConnectionStore`) rather than threading one shared handle across
/// unrelated router-construction functions. SQLite's own file locking
/// handles multiple live connections to the same file correctly; this is a
/// deliberate choice, not an accidental duplicate connection.
#[derive(Clone)]
struct LinearModuleState {
    modules: Arc<Mutex<ModuleStore>>,
    credentials: Arc<Mutex<CredentialStore>>,
    secrets: Arc<dyn SecretsProvider + Send + Sync>,
    accounts: Arc<Mutex<AccountStore>>,
}

async fn require_session(State(state): State<Arc<LinearModuleState>>, mut req: Request, next: Next) -> Response {
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

#[derive(Debug, Deserialize)]
struct EnrollAutoKickoffRequest {
    linear_team_id: String,
    repo_path: PathBuf,
    #[serde(default)]
    assignee_emails: Vec<String>,
    #[serde(default)]
    status_name_map: HashMap<String, String>,
}

/// `POST /linear/auto-kickoff` — the per-account opt-in that replaced
/// boot-time config. Requires the caller already has a stored `linear`
/// credential (`POST /integrations/linear`) and that this deployment has
/// `AGENTOPS_LINEAR_WEBHOOK_URL` configured. Live-registers the webhook,
/// caches the secret in the vault, and enrolls the caller's tenant in the
/// `linear_auto_kickoff` module.
///
/// **Scope simplification, called out not silently assumed**: one Linear
/// team per tenant — `ModuleStore`'s `(tenant, module_name)` key means a
/// second `POST /linear/auto-kickoff` call for the same account replaces
/// its prior enrollment rather than adding a second team. Matches this
/// pass's actual scope (one operator, one team); multiple teams per account
/// is future work.
async fn enroll_auto_kickoff_handler(State(state): State<Arc<LinearModuleState>>, axum::Extension(user): axum::Extension<User>, Json(req): Json<EnrollAutoKickoffRequest>) -> impl IntoResponse {
    let linear_config = {
        let credentials = state.credentials.lock().unwrap();
        match crate::resolve_linear_config(&credentials, state.secrets.as_ref(), &user.tenant) {
            Ok(c) => c,
            Err(e) => return (StatusCode::BAD_REQUEST, Json(json!({ "error": e.to_string() }))).into_response(),
        }
    };

    let Ok(webhook_url) = std::env::var("AGENTOPS_LINEAR_WEBHOOK_URL") else {
        return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "error": "AGENTOPS_LINEAR_WEBHOOK_URL is not configured on this deployment yet — ask the operator to set it" }))).into_response();
    };

    let reg = match agentops_linear::ensure_webhook_registered(&linear_config, &req.linear_team_id, &webhook_url, &["Issue"]) {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_GATEWAY, Json(json!({ "error": format!("registering the Linear webhook failed: {e}") }))).into_response(),
    };

    let cache_provider = format!("linear_webhook:{}", req.linear_team_id);
    {
        let credentials = state.credentials.lock().unwrap();
        let result = credentials.store_credential(
            state.secrets.as_ref(),
            &user.tenant,
            agentops_integrations::NewCredential { provider: &cache_provider, auth_type: agentops_integrations::AuthType::ApiKey, secret: &reg.secret, refresh_token: None, expires_at: None },
        );
        if let Err(e) = result {
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("caching the webhook secret failed: {e}") }))).into_response();
        }
    }

    let assignee_emails = if req.assignee_emails.is_empty() { vec![user.email.clone()] } else { req.assignee_emails.clone() };
    let team_config = AutoKickoffTeamConfig { linear_team_id: req.linear_team_id.clone(), repo_path: req.repo_path.clone(), assignee_emails, webhook_secret: String::new(), status_name_map: req.status_name_map.clone() };
    let config_json = match serde_json::to_string(&team_config) {
        Ok(s) => s,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    };
    {
        let modules = state.modules.lock().unwrap();
        if let Err(e) = modules.enroll(&user.tenant, MODULE_NAME, &config_json) {
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response();
        }
    }

    (StatusCode::OK, Json(json!({ "team_id": req.linear_team_id, "repo_path": req.repo_path, "enabled": true }))).into_response()
}

#[derive(Debug, Serialize)]
struct AutoKickoffView {
    linear_team_id: String,
    repo_path: PathBuf,
    assignee_emails: Vec<String>,
    enabled: bool,
}

async fn list_auto_kickoff_handler(State(state): State<Arc<LinearModuleState>>, axum::Extension(user): axum::Extension<User>) -> impl IntoResponse {
    let listed = { state.modules.lock().unwrap().list_for_tenant(&user.tenant).unwrap_or_default() };
    let views: Vec<AutoKickoffView> = listed
        .into_iter()
        .filter(|e| e.module_name == MODULE_NAME)
        .filter_map(|e| {
            let team: AutoKickoffTeamConfig = serde_json::from_str(&e.config).ok()?;
            Some(AutoKickoffView { linear_team_id: team.linear_team_id, repo_path: team.repo_path, assignee_emails: team.assignee_emails, enabled: e.enabled })
        })
        .collect();
    (StatusCode::OK, Json(views)).into_response()
}

/// `team_id` in the path is documentation/confirmation, not (yet) part of
/// the actual lookup key — see `enroll_auto_kickoff_handler`'s doc comment
/// on the one-team-per-tenant scope simplification.
async fn disable_auto_kickoff_handler(State(state): State<Arc<LinearModuleState>>, axum::Extension(user): axum::Extension<User>, AxumPath(_team_id): AxumPath<String>) -> impl IntoResponse {
    let result = { state.modules.lock().unwrap().disenroll(&user.tenant, MODULE_NAME) };
    match result {
        Ok(true) => (StatusCode::OK, Json(json!({ "disabled": true }))).into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, Json(json!({ "error": "no auto-kickoff enrollment found for this account" }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct PushStatusRequest {
    status_name: String,
}

/// Resolves the calling user's own first `linear_auto_kickoff` enrollment's
/// `repo_path` — not a general multi-repo task API (see this function's
/// callers' own doc comments), just enough to know which repo's graph
/// store a task id lives in.
fn repo_path_for_tenant(modules: &ModuleStore, tenant: &str) -> Option<PathBuf> {
    modules.list_for_tenant(tenant).ok()?.into_iter().find(|e| e.module_name == MODULE_NAME && e.enabled).and_then(|e| serde_json::from_str::<AutoKickoffTeamConfig>(&e.config).ok()).map(|t| t.repo_path)
}

/// Live-verification-only surface: proves `push_status`/`summarize_task_activity`/
/// `post_comment` are reachable with zero caller-supplied secrets — both
/// the Linear key and (for summarize) the Anthropic key resolve from the
/// vault against the calling user's own tenant, the exact thing the vault
/// exists for.
async fn push_task_status_handler(State(state): State<Arc<LinearModuleState>>, axum::Extension(user): axum::Extension<User>, AxumPath(task_id): AxumPath<i64>, Json(req): Json<PushStatusRequest>) -> impl IntoResponse {
    let Some(repo_path) = ({ repo_path_for_tenant(&state.modules.lock().unwrap(), &user.tenant) }) else {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "no enabled linear_auto_kickoff enrollment for this account" }))).into_response();
    };
    let linear_config = {
        let credentials = state.credentials.lock().unwrap();
        match crate::resolve_linear_config(&credentials, state.secrets.as_ref(), &user.tenant) {
            Ok(c) => c,
            Err(e) => return (StatusCode::BAD_REQUEST, Json(json!({ "error": e.to_string() }))).into_response(),
        }
    };
    let store = match agentops_mcp::open_store(&repo_path) {
        Ok(s) => s,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    };
    match agentops_linear::sync_push(store.as_ref(), &linear_config, task_id, Some(&req.status_name)) {
        Ok(()) => (StatusCode::OK, Json(json!({ "pushed": true, "target_state": req.status_name }))).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

async fn summarize_task_handler(State(state): State<Arc<LinearModuleState>>, axum::Extension(user): axum::Extension<User>, AxumPath(task_id): AxumPath<i64>) -> impl IntoResponse {
    let Some(repo_path) = ({ repo_path_for_tenant(&state.modules.lock().unwrap(), &user.tenant) }) else {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "no enabled linear_auto_kickoff enrollment for this account" }))).into_response();
    };
    let store = match agentops_mcp::open_store(&repo_path) {
        Ok(s) => s,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    };
    let repo = agentops_mcp::repo_name(&repo_path);

    let task = match store.get_task(task_id) {
        Ok(Some(t)) => t,
        Ok(None) => return (StatusCode::NOT_FOUND, Json(json!({ "error": format!("task {task_id} not found") }))).into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    };
    let Some(session_id) = &task.session_id else {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "task has no session_id — nothing to summarize" }))).into_response();
    };
    let events = match store.session_events(&repo, session_id) {
        Ok(e) => e,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    };

    let anthropic_config = {
        let credentials = state.credentials.lock().unwrap();
        match crate::resolve_anthropic_config(&credentials, state.secrets.as_ref(), &user.tenant) {
            Ok(c) => c,
            Err(e) => return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "error": e.to_string() }))).into_response(),
        }
    };
    let summaries = match agentops_llm::summarize_task_activity(&anthropic_config, &task.title, &events) {
        Ok(s) => s,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    };

    let mut posted = false;
    if task.external_source.as_deref() == Some("linear") {
        if let Some(external_id) = &task.external_id {
            let linear_config = {
                let credentials = state.credentials.lock().unwrap();
                crate::resolve_linear_config(&credentials, state.secrets.as_ref(), &user.tenant).ok()
            };
            if let Some(linear_config) = linear_config {
                let technical_ok = agentops_linear::post_comment(&linear_config, external_id, &format!("**Technical summary**\n\n{}", summaries.technical));
                let non_technical_ok = agentops_linear::post_comment(&linear_config, external_id, &format!("**Non-technical summary**\n\n{}", summaries.non_technical));
                posted = technical_ok.is_ok() && non_technical_ok.is_ok();
            }
        }
    }

    (StatusCode::OK, Json(json!({ "technical": summaries.technical, "non_technical": summaries.non_technical, "client_friendly": summaries.client_friendly, "posted_to_linear": posted }))).into_response()
}

pub fn merge_linear_module_routes(app: Router, modules: Arc<Mutex<ModuleStore>>, credentials: Arc<Mutex<CredentialStore>>, secrets: Arc<dyn SecretsProvider + Send + Sync>, accounts: Arc<Mutex<AccountStore>>) -> Router {
    let state = Arc::new(LinearModuleState { modules, credentials, secrets, accounts });
    let router = Router::new()
        .route("/linear/auto-kickoff", post(enroll_auto_kickoff_handler).get(list_auto_kickoff_handler))
        .route("/linear/auto-kickoff/{team_id}", delete(disable_auto_kickoff_handler))
        .route("/linear/tasks/{task_id}/push-status", post(push_task_status_handler))
        .route("/linear/tasks/{task_id}/summarize", post(summarize_task_handler))
        .layer(middleware::from_fn_with_state(state.clone(), require_session))
        .with_state(state);
    app.merge(router)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sign(secret: &str, body: &[u8]) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        mac.finalize().into_bytes().iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn verify_signature_accepts_a_correctly_signed_body() {
        let body = br#"{"hello":"world"}"#;
        let sig = sign("shh-its-a-secret", body);
        assert!(verify_signature("shh-its-a-secret", body, &sig));
    }

    #[test]
    fn verify_signature_rejects_a_wrong_secret() {
        let body = br#"{"hello":"world"}"#;
        let sig = sign("shh-its-a-secret", body);
        assert!(!verify_signature("a-different-secret", body, &sig));
    }

    #[test]
    fn verify_signature_rejects_a_tampered_body() {
        let body = br#"{"hello":"world"}"#;
        let sig = sign("shh-its-a-secret", body);
        assert!(!verify_signature("shh-its-a-secret", br#"{"hello":"WORLD"}"#, &sig));
    }

    #[test]
    fn verify_signature_rejects_malformed_hex() {
        assert!(!verify_signature("secret", b"body", "not-hex-at-all!!"));
    }

    #[test]
    fn is_recent_accepts_now_and_rejects_an_hour_ago() {
        let now_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64;
        assert!(is_recent(now_ms));
        assert!(!is_recent(now_ms - 3_600_000));
    }

    #[test]
    fn seen_deliveries_flags_the_second_occurrence_of_the_same_id_as_a_duplicate() {
        let seen = SeenDeliveries::new(100);
        assert!(seen.record_if_new("delivery-1"), "first time must be new");
        assert!(!seen.record_if_new("delivery-1"), "second time must be a duplicate");
        assert!(seen.record_if_new("delivery-2"), "a different id is still new");
    }

    #[test]
    fn seen_deliveries_evicts_the_oldest_once_over_capacity() {
        let seen = SeenDeliveries::new(2);
        assert!(seen.record_if_new("a"));
        assert!(seen.record_if_new("b"));
        assert!(seen.record_if_new("c"));
        // "a" was evicted to make room for "c" — must be treated as new again.
        assert!(seen.record_if_new("a"), "an evicted id must be forgotten, not permanently remembered");
    }

    fn test_team(repo_path: PathBuf) -> AutoKickoffTeamConfig {
        AutoKickoffTeamConfig { linear_team_id: "team-1".into(), repo_path, assignee_emails: vec!["dev@example.com".into()], webhook_secret: "team-1-secret".into(), status_name_map: HashMap::new() }
    }

    fn issue_payload(team_id: &str, assignee_id: &str, action: &str, assignee_changed: bool) -> serde_json::Value {
        let mut payload = json!({
            "type": "Issue",
            "action": action,
            "data": { "id": "issue-uuid", "identifier": "ENG-1", "title": "Fix the thing", "teamId": team_id, "assigneeId": assignee_id },
            "webhookTimestamp": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64,
        });
        if assignee_changed {
            payload["updatedFrom"] = json!({ "assigneeId": null });
        }
        payload
    }

    #[test]
    fn find_verified_team_matches_by_secret_not_by_payload_content() {
        let dir = tempfile::tempdir().unwrap();
        let candidates = vec![("tenant-a".to_string(), test_team(dir.path().to_path_buf()))];
        let body = br#"{"some":"payload"}"#;
        let sig = sign("team-1-secret", body);

        let matched = find_verified_team(&candidates, body, &sig);
        assert!(matched.is_some());
        let (tenant, team) = matched.unwrap();
        assert_eq!(tenant, "tenant-a");
        assert_eq!(team.linear_team_id, "team-1");
    }

    #[test]
    fn find_verified_team_returns_none_when_no_configured_secret_matches() {
        let dir = tempfile::tempdir().unwrap();
        let candidates = vec![("tenant-a".to_string(), test_team(dir.path().to_path_buf()))];
        let body = br#"{"some":"payload"}"#;
        let sig = sign("not-any-configured-secret", body);

        assert!(find_verified_team(&candidates, body, &sig).is_none());
    }

    /// Locks in Phase 6c's replacement for the old global-`enabled`-flag
    /// guarantee: a disabled enrollment must not even show up as a
    /// candidate, so it can never be signature-matched at all.
    #[test]
    fn a_disenrolled_team_produces_no_verifiable_candidate() {
        let modules = ModuleStore::open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let team = test_team(dir.path().to_path_buf());
        let config_json = serde_json::to_string(&team).unwrap();

        let secrets = agentops_repo_access::secrets::EnvSecretsProvider::from_hex(&"11".repeat(32)).unwrap();
        let credentials = CredentialStore::open_in_memory().unwrap();
        credentials
            .store_credential(&secrets, "tenant-a", agentops_integrations::NewCredential { provider: "linear_webhook:team-1", auth_type: agentops_integrations::AuthType::ApiKey, secret: "team-1-secret", refresh_token: None, expires_at: None })
            .unwrap();

        modules.enroll("tenant-a", MODULE_NAME, &config_json).unwrap();
        assert_eq!(load_candidates(&modules, &credentials, &secrets).len(), 1);

        modules.disenroll("tenant-a", MODULE_NAME).unwrap();
        assert!(load_candidates(&modules, &credentials, &secrets).is_empty(), "a disabled enrollment must not be loaded as a candidate at all");
    }

    #[test]
    fn handle_verified_payload_refuses_a_payload_claiming_a_different_team_than_the_verified_one() {
        let dir = tempfile::tempdir().unwrap();
        let team = test_team(dir.path().to_path_buf());
        let linear_config = agentops_linear::LinearConfig { api_key: "unused".into(), api_url: "http://127.0.0.1:1".into() };
        let payload = issue_payload("some-other-team", "user-1", "create", false);

        let result = handle_verified_payload(&team, &linear_config, &payload).unwrap();
        assert!(!result.dispatched);
        assert_eq!(result.reason, "payload team id does not match the verified webhook's team");
    }

    #[test]
    fn handle_verified_payload_skips_an_update_with_no_assignee_change() {
        let dir = tempfile::tempdir().unwrap();
        let team = test_team(dir.path().to_path_buf());
        let linear_config = agentops_linear::LinearConfig { api_key: "unused".into(), api_url: "http://127.0.0.1:1".into() };
        // update, but no `updatedFrom.assigneeId` — some unrelated field changed.
        let payload = issue_payload("team-1", "user-1", "update", false);

        let result = handle_verified_payload(&team, &linear_config, &payload).unwrap();
        assert!(!result.dispatched);
        assert_eq!(result.reason, "not a new assignment");
    }

    #[tokio::test]
    async fn handle_verified_payload_dispatches_scan_and_task_for_a_whitelisted_assignment() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(json!({ "data": { "user": { "email": "dev@example.com" } } })))
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.py"), "def greet():\n    return 'hi'\n").unwrap();
        let team = test_team(dir.path().to_path_buf());
        let linear_config = agentops_linear::LinearConfig { api_key: "unused".into(), api_url: server.uri() };
        let payload = issue_payload("team-1", "user-1", "create", false);

        let result = handle_verified_payload(&team, &linear_config, &payload).unwrap();
        assert!(result.dispatched, "{result:?}");

        let store = agentops_mcp::open_store(dir.path()).unwrap();
        let repo = agentops_mcp::repo_name(dir.path());
        let tasks = store.list_tasks(&repo).unwrap();
        assert_eq!(tasks.len(), 1, "{tasks:?}");
        assert_eq!(tasks[0].external_id.as_deref(), Some("ENG-1"));
        assert_eq!(tasks[0].session_id.as_deref(), Some("linear-auto-kickoff-ENG-1"));
        assert!(store.latest_scan(&repo).unwrap().is_some(), "scan_repo must have actually run");
    }

    fn test_module_state() -> Arc<LinearModuleState> {
        let secrets: Arc<dyn SecretsProvider + Send + Sync> = Arc::new(agentops_repo_access::secrets::EnvSecretsProvider::from_hex(&"22".repeat(32)).unwrap());
        Arc::new(LinearModuleState {
            modules: Arc::new(Mutex::new(ModuleStore::open_in_memory().unwrap())),
            credentials: Arc::new(Mutex::new(CredentialStore::open_in_memory().unwrap())),
            secrets,
            accounts: Arc::new(Mutex::new(AccountStore::open_in_memory().unwrap())),
        })
    }

    async fn signup(state: &Arc<LinearModuleState>) -> (User, String) {
        state.accounts.lock().unwrap().signup("dev@example.com", "correct horse battery staple").unwrap()
    }

    /// `resolve_linear_config` always points a resolved `LinearConfig` at
    /// the real `api.linear.app` (the URL isn't stored per-credential), so
    /// a full live push through this route can't be pointed at a wiremock
    /// server — that path is already covered by `agentops-linear`'s own
    /// `push_status`/`sync_push` wiremock tests, and by this session's real
    /// live verification against actual Linear. What this test proves
    /// instead: the route is reachable, session-authed, and tenant-scoped —
    /// a fresh signup with no enrollment yet gets a clear 404, never a
    /// caller-supplied-key requirement or a cross-tenant leak.
    #[tokio::test]
    async fn push_task_status_route_is_tenant_scoped_and_needs_no_caller_supplied_key() {
        use axum::body::Body;
        use axum::http::Request as HttpRequest;
        use tower::ServiceExt;

        let state = test_module_state();
        let (_user, token) = signup(&state).await;
        let router = merge_linear_module_routes(Router::new(), state.modules.clone(), state.credentials.clone(), state.secrets.clone(), state.accounts.clone());

        let response = router
            .oneshot(
                HttpRequest::post("/linear/tasks/1/push-status")
                    .header("authorization", format!("Bearer {token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"status_name":"Done"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{response:?}");
    }

    #[tokio::test]
    async fn push_task_status_route_rejects_a_request_with_no_session_token() {
        use axum::body::Body;
        use axum::http::Request as HttpRequest;
        use tower::ServiceExt;

        let state = test_module_state();
        let router = merge_linear_module_routes(Router::new(), state.modules.clone(), state.credentials.clone(), state.secrets.clone(), state.accounts.clone());

        let response = router.oneshot(HttpRequest::post("/linear/tasks/1/push-status").header("content-type", "application/json").body(Body::from(r#"{"status_name":"Done"}"#)).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn enroll_auto_kickoff_requires_a_stored_linear_credential_first() {
        use axum::body::Body;
        use axum::http::Request as HttpRequest;
        use tower::ServiceExt;

        let state = test_module_state();
        let (_user, token) = signup(&state).await;
        let router = merge_linear_module_routes(Router::new(), state.modules.clone(), state.credentials.clone(), state.secrets.clone(), state.accounts.clone());

        let response = router
            .oneshot(
                HttpRequest::post("/linear/auto-kickoff")
                    .header("authorization", format!("Bearer {token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"linear_team_id":"team-1","repo_path":"/tmp/repo"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{response:?}");
    }

    #[tokio::test]
    async fn list_auto_kickoff_is_empty_for_a_fresh_account_and_never_crosses_tenants() {
        use axum::body::Body;
        use axum::http::Request as HttpRequest;
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let state = test_module_state();
        // A different tenant's enrollment must never show up here.
        state.modules.lock().unwrap().enroll("someone-elses-tenant", MODULE_NAME, r#"{"linear_team_id":"team-x","repo_path":"/tmp","assignee_emails":[]}"#).unwrap();

        let (_user, token) = signup(&state).await;
        let router = merge_linear_module_routes(Router::new(), state.modules.clone(), state.credentials.clone(), state.secrets.clone(), state.accounts.clone());

        let response = router.oneshot(HttpRequest::get("/linear/auto-kickoff").header("authorization", format!("Bearer {token}")).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let listed: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
        assert!(listed.is_empty(), "{listed:?}");
    }
}

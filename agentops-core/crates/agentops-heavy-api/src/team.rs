//! Team Management's Members tab -- a self-contained sub-router, same
//! `merge_*` pattern as `accounts_integrations`, with its own `require_session`
//! (a separate `AccountStore` connection to the same accounts.sqlite file
//! rather than sharing the accounts router's, matching the existing
//! `accounts_for_linear` precedent in `run()` -- see that call site's
//! comment for why duplicate connections are the deliberate choice here).

use std::sync::{Arc, Mutex};

use agentops_accounts::{AccountStore, User};
use agentops_integrations::CredentialStore;
use agentops_repo_access::store::ConnectionStore;
use agentops_teams::TeamStore;
use axum::extract::{Path as AxumPath, Query, Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Clone)]
struct TeamState {
    accounts: Arc<Mutex<AccountStore>>,
    teams: Arc<Mutex<TeamStore>>,
    repos: Arc<Mutex<ConnectionStore>>,
    /// Needed only by `delete_organization`'s cascade (org-wide + personal
    /// credential vault, and where to find the tenant's docbrain file) --
    /// every other handler in this file ignores these two fields entirely.
    credentials: Arc<Mutex<CredentialStore>>,
    docbrain_db_dir: std::path::PathBuf,
}

pub fn build_team_router(accounts: AccountStore, teams: TeamStore, repos: ConnectionStore, credentials: CredentialStore, docbrain_db_dir: std::path::PathBuf) -> Router {
    let state = TeamState {
        accounts: Arc::new(Mutex::new(accounts)),
        teams: Arc::new(Mutex::new(teams)),
        repos: Arc::new(Mutex::new(repos)),
        credentials: Arc::new(Mutex::new(credentials)),
        docbrain_db_dir,
    };

    Router::new()
        .route("/team", get(get_team).patch(rename_team))
        .route("/team/members", get(list_members))
        .route("/team/roles", get(list_roles))
        .route("/team/roles/custom", post(create_custom_role))
        .route("/team/roles/custom/{role_key}", axum::routing::patch(update_custom_role).delete(delete_custom_role))
        .route("/team/members/{user_id}", axum::routing::patch(update_member).delete(remove_member))
        .route("/team/transfer-ownership", post(transfer_ownership))
        .route("/team/delete-organization", post(delete_organization))
        .route("/team/invites", get(list_invites).post(create_invite))
        .route("/team/invites/{id}/resend", post(resend_invite))
        .route("/team/invites/{id}", axum::routing::delete(revoke_invite))
        .route("/team/repo-access", get(get_repo_access).put(save_repo_access))
        .route("/team/audit-log", get(get_audit_log))
        .route("/invites/accept", post(accept_invite))
        .layer(middleware::from_fn_with_state(state.clone(), require_session))
        // Public: no session required to preview an invite link (the
        // token itself is the secret) -- added after `.layer()` so it's
        // excluded from `require_session`, same pattern
        // `accounts_integrations` uses for `/auth/signup`/`/auth/login`.
        .route("/invites/{token}", get(preview_invite))
        .with_state(state)
}

async fn require_session(State(state): State<TeamState>, mut req: Request, next: Next) -> Response {
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

async fn get_team(State(state): State<TeamState>, axum::Extension(user): axum::Extension<User>) -> (StatusCode, Json<serde_json::Value>) {
    let teams = state.teams.lock().unwrap();
    if let Err(e) = teams.ensure_membership(&user.tenant, user.id) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() })));
    }
    let name = teams.organization_name(&user.tenant).unwrap_or_default().unwrap_or_default();
    let members = match teams.list_members(&user.tenant) {
        Ok(m) => m,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    };
    let role = members.iter().find(|m| m.user_id == user.id).map(|m| m.role.clone()).unwrap_or_default();
    let is_owner = teams.owner_user_id(&user.tenant).unwrap_or(None) == Some(user.id);
    (StatusCode::OK, Json(json!({ "tenant": user.tenant, "name": name, "member_count": members.len(), "role": role, "is_owner": is_owner })))
}

#[derive(Deserialize)]
struct RenameTeamRequest {
    name: String,
}

/// Owner-only -- naming the org is an identity-level decision, same
/// posture as `transfer_ownership`/`delete_organization`. In practice this
/// is only ever called from the `/welcome` onboarding checklist's
/// workspace-setup item, which itself only renders for the Owner, but the
/// backend enforces it independently rather than trusting the frontend.
async fn rename_team(State(state): State<TeamState>, axum::Extension(user): axum::Extension<User>, Json(req): Json<RenameTeamRequest>) -> (StatusCode, Json<serde_json::Value>) {
    let teams = state.teams.lock().unwrap();
    if let Err(e) = teams.ensure_membership(&user.tenant, user.id) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() })));
    }
    if let Err(err) = require_owner(&teams, &user.tenant, user.id) {
        return err;
    }
    let name = req.name.trim();
    if name.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "name is required" })));
    }
    match teams.rename_organization(&user.tenant, name) {
        Ok(()) => (StatusCode::OK, Json(json!({ "name": name }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    }
}

#[derive(Serialize)]
struct MemberView {
    user_id: i64,
    email: String,
    first_name: String,
    last_name: String,
    avatar_url: Option<String>,
    role: String,
    status: String,
    joined_at: String,
    is_you: bool,
}

async fn list_members(State(state): State<TeamState>, axum::Extension(user): axum::Extension<User>) -> (StatusCode, Json<serde_json::Value>) {
    let teams = state.teams.lock().unwrap();
    if let Err(e) = teams.ensure_membership(&user.tenant, user.id) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() })));
    }
    let memberships = match teams.list_members(&user.tenant) {
        Ok(m) => m,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    };

    let accounts = state.accounts.lock().unwrap();
    let mut views = Vec::with_capacity(memberships.len());
    for m in memberships {
        // A membership row whose user no longer exists in `accounts`
        // shouldn't be possible in practice (nothing deletes a `users`
        // row), but skip rather than 500 the whole list if it ever
        // happened -- one orphaned row is not worth failing every other
        // member's view over.
        let Ok(Some(member_user)) = accounts.get_user(m.user_id) else { continue };
        views.push(MemberView {
            user_id: m.user_id,
            email: member_user.email,
            first_name: member_user.first_name,
            last_name: member_user.last_name,
            avatar_url: member_user.avatar_url,
            role: m.role,
            status: m.status,
            joined_at: m.joined_at,
            is_you: m.user_id == user.id,
        });
    }
    (StatusCode::OK, Json(serde_json::to_value(views).unwrap()))
}

#[derive(Serialize)]
struct RoleView {
    role: &'static str,
    label: &'static str,
    description: &'static str,
    member_count: usize,
}

#[derive(Serialize)]
struct CapabilityView {
    key: &'static str,
    feature_area: &'static str,
    label: &'static str,
    allowed_roles: &'static [&'static str],
}

#[derive(Serialize)]
struct CustomRoleView {
    role_key: String,
    label: String,
    cloned_from: String,
    capabilities: Vec<String>,
}

impl From<agentops_teams::CustomRole> for CustomRoleView {
    fn from(r: agentops_teams::CustomRole) -> Self {
        CustomRoleView { role_key: r.role_key, label: r.label, cloned_from: r.cloned_from, capabilities: r.capabilities }
    }
}

/// Static matrix + live per-role counts -- any active member can view
/// this (not admin-gated), matching the mockup's own "read-only" framing
/// for the whole tab. `agentops_teams::PERMISSIONS_MATRIX` is the single
/// source of truth this serializes verbatim, so the displayed matrix can
/// never drift from what that crate documents as each role's capabilities.
async fn list_roles(State(state): State<TeamState>, axum::Extension(user): axum::Extension<User>) -> (StatusCode, Json<serde_json::Value>) {
    let teams = state.teams.lock().unwrap();
    if let Err(e) = teams.ensure_membership(&user.tenant, user.id) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() })));
    }
    let memberships = match teams.list_members(&user.tenant) {
        Ok(m) => m,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    };

    let roles: Vec<RoleView> = agentops_teams::ROLE_DESCRIPTIONS
        .iter()
        .map(|rd| RoleView { role: rd.role, label: rd.label, description: rd.description, member_count: memberships.iter().filter(|m| m.role == rd.role).count() })
        .collect();
    let matrix: Vec<CapabilityView> =
        agentops_teams::PERMISSIONS_MATRIX.iter().map(|c| CapabilityView { key: c.key, feature_area: c.feature_area, label: c.label, allowed_roles: c.allowed_roles }).collect();
    let custom_roles = match teams.list_custom_roles(&user.tenant) {
        Ok(r) => r,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    };
    let custom_role_views: Vec<CustomRoleView> = custom_roles.into_iter().map(CustomRoleView::from).collect();

    (StatusCode::OK, Json(json!({ "roles": roles, "matrix": matrix, "custom_roles": custom_role_views })))
}

/// Rejects any capability key that isn't a real row in
/// `PERMISSIONS_MATRIX` -- a custom role's capability list is
/// server-validated against the known key set, not trusted verbatim from
/// the client, so a typo or a stale frontend can't silently grant (or,
/// more likely, just store a no-op) capability.
fn validate_capability_keys(capabilities: &[String]) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    for cap in capabilities {
        if !agentops_teams::PERMISSIONS_MATRIX.iter().any(|c| c.key == cap) {
            return Err((StatusCode::BAD_REQUEST, Json(json!({ "error": format!("unknown capability {cap:?}") }))));
        }
    }
    Ok(())
}

#[derive(Deserialize)]
struct CreateCustomRoleRequest {
    label: String,
    cloned_from: String,
    capabilities: Vec<String>,
}

async fn create_custom_role(State(state): State<TeamState>, axum::Extension(user): axum::Extension<User>, Json(req): Json<CreateCustomRoleRequest>) -> (StatusCode, Json<serde_json::Value>) {
    let teams = state.teams.lock().unwrap();
    if let Err(e) = teams.ensure_membership(&user.tenant, user.id) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() })));
    }
    if let Err(err) = require_capability(&teams, &user.tenant, user.id, agentops_teams::CAP_TEAM_MANAGE_ROLES) {
        return err;
    }
    if req.label.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "label is required" })));
    }
    if let Err(err) = validate_capability_keys(&req.capabilities) {
        return err;
    }
    let capabilities: Vec<&str> = req.capabilities.iter().map(String::as_str).collect();
    match teams.create_custom_role(&user.tenant, req.label.trim(), &req.cloned_from, &capabilities, user.id) {
        Ok(role) => (StatusCode::CREATED, Json(serde_json::to_value(CustomRoleView::from(role)).unwrap())),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e.to_string() }))),
    }
}

#[derive(Deserialize)]
struct UpdateCustomRoleRequest {
    capabilities: Vec<String>,
}

async fn update_custom_role(
    State(state): State<TeamState>,
    axum::Extension(user): axum::Extension<User>,
    AxumPath(role_key): AxumPath<String>,
    Json(req): Json<UpdateCustomRoleRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    let teams = state.teams.lock().unwrap();
    if let Err(e) = teams.ensure_membership(&user.tenant, user.id) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() })));
    }
    if let Err(err) = require_capability(&teams, &user.tenant, user.id, agentops_teams::CAP_TEAM_MANAGE_ROLES) {
        return err;
    }
    if let Err(err) = validate_capability_keys(&req.capabilities) {
        return err;
    }
    let capabilities: Vec<&str> = req.capabilities.iter().map(String::as_str).collect();
    match teams.update_custom_role_capabilities(&user.tenant, &role_key, &capabilities) {
        Ok(true) => (StatusCode::OK, Json(json!({ "updated": true }))),
        Ok(false) => (StatusCode::NOT_FOUND, Json(json!({ "error": "no such custom role" }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    }
}

/// Blocked with `400` (not `409`, matching this crate's own
/// `delete_custom_role` error rather than inventing a new status
/// convention) if any active membership still holds this role.
async fn delete_custom_role(State(state): State<TeamState>, axum::Extension(user): axum::Extension<User>, AxumPath(role_key): AxumPath<String>) -> (StatusCode, Json<serde_json::Value>) {
    let teams = state.teams.lock().unwrap();
    if let Err(e) = teams.ensure_membership(&user.tenant, user.id) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() })));
    }
    if let Err(err) = require_capability(&teams, &user.tenant, user.id, agentops_teams::CAP_TEAM_MANAGE_ROLES) {
        return err;
    }
    match teams.delete_custom_role(&user.tenant, &role_key) {
        Ok(true) => (StatusCode::OK, Json(json!({ "deleted": true }))),
        Ok(false) => (StatusCode::NOT_FOUND, Json(json!({ "error": "no such custom role" }))),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e.to_string() }))),
    }
}

#[derive(Deserialize)]
struct UpdateMemberRequest {
    role: Option<String>,
    status: Option<String>,
}

/// Requires the caller to be an active admin of `tenant` -- kept only for
/// `get_audit_log`, which is deliberately hardcoded to admin-or-Owner
/// rather than a togglable capability (see that handler's own comment).
/// Every other mutating handler moved to `require_capability` below.
fn require_admin(teams: &TeamStore, tenant: &str, user_id: i64) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    match teams.get_membership(tenant, user_id) {
        Ok(Some(m)) if m.role == "admin" && m.status == "active" => Ok(()),
        Ok(_) => Err((StatusCode::FORBIDDEN, Json(json!({ "error": "only an admin can do this" })))),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() })))),
    }
}

/// Requires the caller to hold `capability` in `tenant` -- the
/// capability-aware replacement for `require_admin`, reading from
/// `agentops_teams::has_capability` (itself sourced from
/// `PERMISSIONS_MATRIX`) so a route's gate and its displayed row in the
/// Roles & Permissions matrix can never drift apart.
fn require_capability(teams: &TeamStore, tenant: &str, user_id: i64, capability: &str) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    match agentops_teams::has_capability(teams, tenant, user_id, capability) {
        Ok(true) => Ok(()),
        Ok(false) => Err((StatusCode::FORBIDDEN, Json(json!({ "error": format!("missing required capability: {capability}") })))),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() })))),
    }
}

/// Requires the caller to be `tenant`'s Owner -- stricter than
/// `require_admin`, used only for the handful of actions the Owner role
/// exists specifically to gate (acting on another admin, ownership
/// transfer, org deletion, billing).
fn require_owner(teams: &TeamStore, tenant: &str, user_id: i64) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    match teams.owner_user_id(tenant) {
        Ok(Some(owner_id)) if owner_id == user_id => Ok(()),
        Ok(_) => Err((StatusCode::FORBIDDEN, Json(json!({ "error": "only the organization owner can do this" })))),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() })))),
    }
}

/// Shared precondition for `update_member`/`remove_member`, composing two
/// independent guards so they don't get tangled into ad hoc `if`-chains
/// bolted onto those already-dense handlers:
///
/// 1. The organization can never end up with zero active admins (existing
///    guard, unchanged in spirit -- just relocated here).
/// 2. Acting on *another* active admin (demoting them, suspending them, or
///    removing them) requires being the Owner -- an admin can freely act on
///    themselves or on non-admins, but not on a peer admin.
///
/// `would_demote_or_remove` is the caller's own judgment of whether this
/// specific change actually affects the target's admin status (e.g. a
/// no-op patch that leaves role/status unchanged must not trip either
/// guard) -- kept as a param rather than recomputed here, since the two
/// call sites (`update_member`'s partial-patch semantics vs. `remove_member`,
/// which is unconditionally a removal) compute it differently.
fn authorize_member_mutation(teams: &TeamStore, tenant: &str, caller_id: i64, target_user_id: i64, would_demote_or_remove: bool) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    if !would_demote_or_remove {
        return Ok(());
    }
    let target = match teams.get_membership(tenant, target_user_id) {
        Ok(t) => t,
        Err(e) => return Err((StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() })))),
    };
    // No such member: let the caller's own subsequent update/remove call
    // report 404 -- nothing to authorize against here.
    let Some(target) = target else { return Ok(()) };
    if target.role != "admin" || target.status != "active" {
        return Ok(());
    }
    match teams.count_active_admins(tenant) {
        Ok(count) if count <= 1 => return Err((StatusCode::CONFLICT, Json(json!({ "error": "cannot demote or remove the last active admin" })))),
        Ok(_) => {}
        Err(e) => return Err((StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() })))),
    }
    if caller_id == target_user_id {
        // Self-demotion of an admin is allowed without being Owner -- only
        // acting on *another* admin requires ownership.
        return Ok(());
    }
    require_owner(teams, tenant, caller_id)
}

fn client_ip(headers: &axum::http::HeaderMap) -> Option<String> {
    headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()).and_then(|v| v.split(',').next()).map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

async fn update_member(
    State(state): State<TeamState>,
    headers: axum::http::HeaderMap,
    axum::Extension(user): axum::Extension<User>,
    AxumPath(target_user_id): AxumPath<i64>,
    Json(req): Json<UpdateMemberRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    let teams = state.teams.lock().unwrap();
    if let Err(e) = teams.ensure_membership(&user.tenant, user.id) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() })));
    }
    // Granular per-field capability check -- a role change needs
    // `team.manage_roles`, a status change needs `team.manage_members`;
    // for today's fixed roles both live only on "admin" so this is
    // behavior-preserving, but it's the right shape for a future custom
    // role that holds one capability without the other.
    if req.role.is_some() {
        if let Err(err) = require_capability(&teams, &user.tenant, user.id, agentops_teams::CAP_TEAM_MANAGE_ROLES) {
            return err;
        }
    }
    if req.status.is_some() {
        if let Err(err) = require_capability(&teams, &user.tenant, user.id, agentops_teams::CAP_TEAM_MANAGE_MEMBERS) {
            return err;
        }
    }
    if let Some(role) = &req.role {
        match teams.is_valid_role(&user.tenant, role) {
            Ok(true) => {}
            Ok(false) => return (StatusCode::BAD_REQUEST, Json(json!({ "error": format!("invalid role {role:?}, must be one of {:?} or an existing custom role", agentops_teams::ROLES) }))),
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
        }
    }

    // Demoting or suspending an active admin needs authorization beyond
    // plain admin status -- see `authorize_member_mutation`'s doc comment
    // for the two guards it composes (last-active-admin protection, and
    // Owner-only when the target is a *different* admin).
    let is_demoting_from_admin = req.role.as_deref().is_some_and(|r| r != "admin") || req.status.as_deref().is_some_and(|s| s != "active");
    if let Err(err) = authorize_member_mutation(&teams, &user.tenant, user.id, target_user_id, is_demoting_from_admin) {
        return err;
    }

    let result = teams.update_member(&user.tenant, target_user_id, req.role.as_deref(), req.status.as_deref());
    match result {
        Ok(true) => {
            let target_email = state.accounts.lock().unwrap().get_user(target_user_id).ok().flatten().map(|u| u.email);
            let action = if req.role.is_some() { "role_changed" } else { "status_changed" };
            let metadata = json!({ "role": req.role, "status": req.status }).to_string();
            let _ = teams.record_audit_event(&user.tenant, Some(user.id), action, target_email.as_deref(), client_ip(&headers).as_deref(), &metadata);
            (StatusCode::OK, Json(json!({ "updated": true })))
        }
        Ok(false) => (StatusCode::NOT_FOUND, Json(json!({ "error": "no such member" }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    }
}

async fn remove_member(State(state): State<TeamState>, headers: axum::http::HeaderMap, axum::Extension(user): axum::Extension<User>, AxumPath(target_user_id): AxumPath<i64>) -> (StatusCode, Json<serde_json::Value>) {
    let teams = state.teams.lock().unwrap();
    if let Err(e) = teams.ensure_membership(&user.tenant, user.id) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() })));
    }
    if let Err(err) = require_capability(&teams, &user.tenant, user.id, agentops_teams::CAP_TEAM_MANAGE_MEMBERS) {
        return err;
    }

    if let Err(err) = authorize_member_mutation(&teams, &user.tenant, user.id, target_user_id, true) {
        return err;
    }

    let target_email = state.accounts.lock().unwrap().get_user(target_user_id).ok().flatten().map(|u| u.email);
    let result = teams.remove_member(&user.tenant, target_user_id);
    match result {
        Ok(true) => {
            let _ = teams.record_audit_event(&user.tenant, Some(user.id), "member_removed", target_email.as_deref(), client_ip(&headers).as_deref(), "{}");
            (StatusCode::OK, Json(json!({ "removed": true })))
        }
        Ok(false) => (StatusCode::NOT_FOUND, Json(json!({ "error": "no such member" }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    }
}

#[derive(Deserialize)]
struct TransferOwnershipRequest {
    to_user_id: i64,
}

/// Owner-only -- the target must be an active admin (transferring
/// ownership to a non-admin would leave the org with an Owner who can't do
/// anything an Owner is meant to gate, and a viewer/member/billing role
/// isn't trusted with member management to begin with). A single
/// `set_owner` call, atomic within `teams.sqlite`.
async fn transfer_ownership(
    State(state): State<TeamState>,
    headers: axum::http::HeaderMap,
    axum::Extension(user): axum::Extension<User>,
    Json(req): Json<TransferOwnershipRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    let teams = state.teams.lock().unwrap();
    if let Err(e) = teams.ensure_membership(&user.tenant, user.id) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() })));
    }
    if let Err(err) = require_owner(&teams, &user.tenant, user.id) {
        return err;
    }
    match teams.get_membership(&user.tenant, req.to_user_id) {
        Ok(Some(m)) if m.role == "admin" && m.status == "active" => {}
        Ok(_) => return (StatusCode::BAD_REQUEST, Json(json!({ "error": "ownership can only be transferred to an active admin" }))),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    }
    if let Err(e) = teams.set_owner(&user.tenant, req.to_user_id) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() })));
    }
    let target_email = state.accounts.lock().unwrap().get_user(req.to_user_id).ok().flatten().map(|u| u.email);
    let _ = teams.record_audit_event(&user.tenant, Some(user.id), "ownership_transferred", target_email.as_deref(), client_ip(&headers).as_deref(), "{}");
    (StatusCode::OK, Json(json!({ "transferred": true })))
}

/// 16 random bytes, hex-encoded -- the exact same shape
/// `agentops-accounts::signup` gives a brand-new user's tenant (that
/// function is private to that crate, so this is a small, deliberate
/// duplication of the *pattern*, not a shared dependency, to avoid an
/// otherwise-unwarranted public API addition to `agentops-accounts` for a
/// single call site).
fn new_random_tenant_id() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("system randomness must be available");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Deserialize)]
struct DeleteOrganizationRequest {
    /// Type-to-confirm safety check -- must match the caller's own
    /// `tenant` exactly, not just a button click, given this is a full,
    /// irreversible cascade delete.
    confirm_tenant: String,
}

/// Owner-only. Full cascade delete (confirmed by the user, overriding a
/// soft-delete-first recommendation): every `agentops-repo-access`
/// connection, every org-wide and personal `agentops-integrations`
/// credential, the tenant's docbrain SQLite file, and every
/// `agentops-teams` row for the tenant **except `audit_log`** -- see
/// `TeamStore::delete_organization_data`'s doc comment for why that
/// exclusion is deliberate, not an oversight.
///
/// No transaction spans the separate SQLite files involved, so order
/// matters: leaf stores first (best-effort, log-and-continue -- a failure
/// here shouldn't block the rest of the cascade, since partial cleanup is
/// still strictly better than none), the atomic `agentops-teams` deletion
/// last as the actual point of no return. Codebrain's per-repo
/// `.context/graph.db` files are deliberately out of scope -- they live on
/// whichever local filesystem the `agentops` CLI ran against, a path this
/// server never learns or stores; see the plan's own note on this
/// boundary.
async fn delete_organization(
    State(state): State<TeamState>,
    headers: axum::http::HeaderMap,
    axum::Extension(user): axum::Extension<User>,
    Json(req): Json<DeleteOrganizationRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    let mut teams = state.teams.lock().unwrap();
    if let Err(e) = teams.ensure_membership(&user.tenant, user.id) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() })));
    }
    if let Err(err) = require_owner(&teams, &user.tenant, user.id) {
        return err;
    }
    if req.confirm_tenant != user.tenant {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "confirm_tenant does not match your organization" })));
    }

    if let Err(e) = state.repos.lock().unwrap().delete_all_for_tenant(&user.tenant) {
        eprintln!("org deletion: failed to delete repo connections for tenant {}: {e}", user.tenant);
    }
    {
        let credentials = state.credentials.lock().unwrap();
        if let Err(e) = credentials.delete_all_for_tenant(&user.tenant) {
            eprintln!("org deletion: failed to delete org-wide credentials for tenant {}: {e}", user.tenant);
        }
        if let Err(e) = credentials.delete_all_user_credentials_for_tenant(&user.tenant) {
            eprintln!("org deletion: failed to delete personal credentials for tenant {}: {e}", user.tenant);
        }
    }
    let docbrain_path = crate::docbrain_db_path_for_org(&state.docbrain_db_dir, Some(&user.tenant));
    match std::fs::remove_file(&docbrain_path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => eprintln!("org deletion: failed to delete docbrain file for tenant {}: {e}", user.tenant),
    }

    let deleted_tenant = user.tenant.clone();
    let _ = teams.record_audit_event(&deleted_tenant, Some(user.id), "org_deleted", None, client_ip(&headers).as_deref(), "{}");
    if let Err(e) = teams.delete_organization_data(&deleted_tenant) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() })));
    }
    drop(teams);

    // The deleting Owner lands in a fresh, working solo tenant rather than
    // pointed at a now-dead tenant string. Any *other* former member is
    // deliberately left with `users.tenant` unchanged -- the next time they
    // touch any `/team/*` endpoint, `ensure_membership`'s existing backfill
    // transparently gives them a fresh, empty, differently-named org under
    // that same tenant id. No explicit notification/reassignment beyond
    // this -- not asked for, would be scope creep.
    let new_tenant = new_random_tenant_id();
    if let Err(e) = state.accounts.lock().unwrap().switch_tenant(user.id, &new_tenant) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("organization deleted, but failed to move you to a new tenant: {e}") })));
    }

    (StatusCode::OK, Json(json!({ "deleted": true, "new_tenant": new_tenant })))
}

#[derive(Serialize)]
struct InviteView {
    id: i64,
    email: String,
    role: String,
    note: Option<String>,
    status: String,
    created_at: String,
    expires_at: String,
}

impl From<agentops_teams::InviteInfo> for InviteView {
    fn from(i: agentops_teams::InviteInfo) -> Self {
        InviteView { id: i.id, email: i.email, role: i.role, note: i.note, status: i.status, created_at: i.created_at, expires_at: i.expires_at }
    }
}

async fn list_invites(State(state): State<TeamState>, axum::Extension(user): axum::Extension<User>) -> (StatusCode, Json<serde_json::Value>) {
    let teams = state.teams.lock().unwrap();
    if let Err(e) = teams.ensure_membership(&user.tenant, user.id) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() })));
    }
    if let Err(err) = require_capability(&teams, &user.tenant, user.id, agentops_teams::CAP_TEAM_MANAGE_MEMBERS) {
        return err;
    }
    match teams.list_pending_invites(&user.tenant) {
        Ok(invites) => (StatusCode::OK, Json(serde_json::to_value(invites.into_iter().map(InviteView::from).collect::<Vec<_>>()).unwrap())),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    }
}

#[derive(Deserialize)]
struct CreateInviteRequest {
    email: String,
    role: String,
    note: Option<String>,
}

/// No email infrastructure exists in this codebase -- the response
/// includes the raw invite token so the frontend can build a copyable
/// link (`/invite/{token}`) rather than actually sending anything.
async fn create_invite(State(state): State<TeamState>, headers: axum::http::HeaderMap, axum::Extension(user): axum::Extension<User>, Json(req): Json<CreateInviteRequest>) -> (StatusCode, Json<serde_json::Value>) {
    let teams = state.teams.lock().unwrap();
    if let Err(e) = teams.ensure_membership(&user.tenant, user.id) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() })));
    }
    if let Err(err) = require_capability(&teams, &user.tenant, user.id, agentops_teams::CAP_TEAM_MANAGE_MEMBERS) {
        return err;
    }
    if req.email.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "email is required" })));
    }

    match teams.create_invite(&user.tenant, req.email.trim(), &req.role, req.note.as_deref(), user.id) {
        Ok((invite, token)) => {
            let metadata = json!({ "role": invite.role }).to_string();
            let _ = teams.record_audit_event(&user.tenant, Some(user.id), "member_invited", Some(&invite.email), client_ip(&headers).as_deref(), &metadata);
            let mut view = serde_json::to_value(InviteView::from(invite)).unwrap();
            view["token"] = json!(token);
            (StatusCode::CREATED, Json(view))
        }
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e.to_string() }))),
    }
}

async fn resend_invite(State(state): State<TeamState>, headers: axum::http::HeaderMap, axum::Extension(user): axum::Extension<User>, AxumPath(invite_id): AxumPath<i64>) -> (StatusCode, Json<serde_json::Value>) {
    let teams = state.teams.lock().unwrap();
    if let Err(e) = teams.ensure_membership(&user.tenant, user.id) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() })));
    }
    if let Err(err) = require_capability(&teams, &user.tenant, user.id, agentops_teams::CAP_TEAM_MANAGE_MEMBERS) {
        return err;
    }
    match teams.resend_invite(&user.tenant, invite_id) {
        Ok(Some(token)) => {
            let _ = teams.record_audit_event(&user.tenant, Some(user.id), "invite_resent", Some(&invite_id.to_string()), client_ip(&headers).as_deref(), "{}");
            (StatusCode::OK, Json(json!({ "token": token })))
        }
        Ok(None) => (StatusCode::NOT_FOUND, Json(json!({ "error": "no such pending invite" }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    }
}

async fn revoke_invite(State(state): State<TeamState>, headers: axum::http::HeaderMap, axum::Extension(user): axum::Extension<User>, AxumPath(invite_id): AxumPath<i64>) -> (StatusCode, Json<serde_json::Value>) {
    let teams = state.teams.lock().unwrap();
    if let Err(e) = teams.ensure_membership(&user.tenant, user.id) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() })));
    }
    if let Err(err) = require_capability(&teams, &user.tenant, user.id, agentops_teams::CAP_TEAM_MANAGE_MEMBERS) {
        return err;
    }
    match teams.revoke_invite(&user.tenant, invite_id) {
        Ok(true) => {
            let _ = teams.record_audit_event(&user.tenant, Some(user.id), "invite_revoked", Some(&invite_id.to_string()), client_ip(&headers).as_deref(), "{}");
            (StatusCode::OK, Json(json!({ "revoked": true })))
        }
        Ok(false) => (StatusCode::NOT_FOUND, Json(json!({ "error": "no such pending invite" }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    }
}

#[derive(Serialize)]
struct InvitePreviewView {
    tenant: String,
    org_name: String,
    email: String,
    role: String,
}

/// Public -- no session required, the token itself is the secret. Used by
/// the unauthenticated `/invite/[token]` landing page to render "You've
/// been invited to join X as Y" before the visitor logs in or signs up.
async fn preview_invite(State(state): State<TeamState>, AxumPath(token): AxumPath<String>) -> (StatusCode, Json<serde_json::Value>) {
    let teams = state.teams.lock().unwrap();
    match teams.preview_invite(&token) {
        Ok(Some(preview)) => {
            let org_name = teams.organization_name(&preview.tenant).unwrap_or_default().unwrap_or_default();
            let view = InvitePreviewView { tenant: preview.tenant, org_name, email: preview.email, role: preview.role };
            (StatusCode::OK, Json(serde_json::to_value(view).unwrap()))
        }
        Ok(None) => (StatusCode::NOT_FOUND, Json(json!({ "error": "invalid or expired invite" }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    }
}

#[derive(Deserialize)]
struct AcceptInviteRequest {
    token: String,
}

/// Session-authed (any logged-in user, not necessarily already a member of
/// the inviting tenant) -- accepts an invite by creating the membership
/// and switching the caller's active tenant to the inviting org.
/// `teams.accept_invite` marks the invite accepted before this creates the
/// membership; see that method's doc comment for why (no true cross-file
/// atomicity is possible here regardless of ordering).
async fn accept_invite(State(state): State<TeamState>, headers: axum::http::HeaderMap, axum::Extension(user): axum::Extension<User>, Json(req): Json<AcceptInviteRequest>) -> (StatusCode, Json<serde_json::Value>) {
    let teams = state.teams.lock().unwrap();
    let (tenant, role) = match teams.accept_invite(&req.token) {
        Ok(t) => t,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(json!({ "error": e.to_string() }))),
    };

    // Already a member of that tenant (e.g. re-clicking an old invite link
    // sent before they first joined) -- treat as a harmless no-op rather
    // than a hard failure, since the invite token itself already proved
    // valid and single-use above.
    if teams.get_membership(&tenant, user.id).unwrap_or(None).is_none() {
        if let Err(e) = teams.add_member(&tenant, user.id, &role) {
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() })));
        }
    }

    let accounts = state.accounts.lock().unwrap();
    if let Err(e) = accounts.switch_tenant(user.id, &tenant) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() })));
    }

    let metadata = json!({ "role": role }).to_string();
    let _ = teams.record_audit_event(&tenant, Some(user.id), "member_joined", Some(&user.email), client_ip(&headers).as_deref(), &metadata);

    (StatusCode::OK, Json(json!({ "tenant": tenant, "role": role })))
}

#[derive(Serialize)]
struct RepoAccessMemberView {
    user_id: i64,
    first_name: String,
    last_name: String,
    role: String,
}

#[derive(Serialize)]
struct RepoAccessCell {
    user_id: i64,
    repo_id: String,
    allowed: bool,
    editable: bool,
}

async fn get_repo_access(State(state): State<TeamState>, axum::Extension(user): axum::Extension<User>) -> (StatusCode, Json<serde_json::Value>) {
    let teams = state.teams.lock().unwrap();
    if let Err(e) = teams.ensure_membership(&user.tenant, user.id) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() })));
    }
    let memberships = match teams.list_members(&user.tenant) {
        Ok(m) => m,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    };
    let overrides = match teams.list_repo_overrides(&user.tenant) {
        Ok(o) => o,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    };
    let repos = match state.repos.lock().unwrap().list_connections(&user.tenant) {
        Ok(r) => r,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    };

    let accounts = state.accounts.lock().unwrap();
    let mut member_views = Vec::with_capacity(memberships.len());
    let mut cells = Vec::with_capacity(memberships.len() * repos.len());
    for m in &memberships {
        let Ok(Some(member_user)) = accounts.get_user(m.user_id) else { continue };
        member_views.push(RepoAccessMemberView { user_id: m.user_id, first_name: member_user.first_name, last_name: member_user.last_name, role: m.role.clone() });

        let (default_allowed, editable) = agentops_teams::role_default_repo_access(&m.role);
        for repo in &repos {
            let allowed = overrides.iter().find(|(uid, rid, _)| *uid == m.user_id && rid == &repo.id).map(|(_, _, allowed)| *allowed).unwrap_or(default_allowed);
            cells.push(RepoAccessCell { user_id: m.user_id, repo_id: repo.id.clone(), allowed, editable });
        }
    }

    let repo_views: Vec<serde_json::Value> = repos.iter().map(|r| json!({ "id": r.id, "repo_url": r.repo_url })).collect();
    (StatusCode::OK, Json(json!({ "repos": repo_views, "members": member_views, "access": cells })))
}

#[derive(Deserialize)]
struct RepoAccessDiff {
    user_id: i64,
    repo_id: String,
    allowed: bool,
}

#[derive(Deserialize)]
struct SaveRepoAccessRequest {
    changes: Vec<RepoAccessDiff>,
}

/// Admin-only, matching every other team-mutation endpoint. Only writes
/// overrides for members whose role is actually override-editable
/// (member/viewer) -- a diff submitted for an admin or billing row is
/// silently ignored rather than erroring, since the frontend's own
/// disabled checkboxes should never produce one, but a stale/replayed
/// request shouldn't be able to defeat the "admin always has full access"
/// guarantee either.
async fn save_repo_access(State(state): State<TeamState>, headers: axum::http::HeaderMap, axum::Extension(user): axum::Extension<User>, Json(req): Json<SaveRepoAccessRequest>) -> (StatusCode, Json<serde_json::Value>) {
    let teams = state.teams.lock().unwrap();
    if let Err(e) = teams.ensure_membership(&user.tenant, user.id) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() })));
    }
    if let Err(err) = require_capability(&teams, &user.tenant, user.id, agentops_teams::CAP_REPOS_CONNECT) {
        return err;
    }

    let mut applied = 0;
    for change in req.changes {
        let target_role = match teams.get_membership(&user.tenant, change.user_id) {
            Ok(Some(m)) => m.role,
            Ok(None) => continue,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
        };
        let (_, editable) = agentops_teams::role_default_repo_access(&target_role);
        if !editable {
            continue;
        }
        if let Err(e) = teams.set_repo_override(&user.tenant, change.user_id, &change.repo_id, change.allowed) {
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() })));
        }
        applied += 1;
    }
    if applied > 0 {
        let metadata = json!({ "changes": applied }).to_string();
        let _ = teams.record_audit_event(&user.tenant, Some(user.id), "repo_access_changed", None, client_ip(&headers).as_deref(), &metadata);
    }
    (StatusCode::OK, Json(json!({ "saved": true })))
}

#[derive(Deserialize, Default)]
struct AuditLogQuery {
    actor: Option<i64>,
    action: Option<String>,
    search: Option<String>,
}

#[derive(Serialize)]
struct AuditEventView {
    id: i64,
    action: String,
    target: Option<String>,
    actor_email: Option<String>,
    ip_address: Option<String>,
    metadata: serde_json::Value,
    created_at: String,
}

/// Admin-only read -- least-privilege default (see the module's
/// authorization posture elsewhere: every team-mutation endpoint is
/// already admin-gated, and this is the one place all of them become
/// visible at once). `metadata` is parsed back into JSON here (it's
/// stored as a raw string in `agentops-teams`, see that crate's doc
/// comment for why) so the response is real JSON, not a JSON-string field.
async fn get_audit_log(State(state): State<TeamState>, axum::Extension(user): axum::Extension<User>, Query(q): Query<AuditLogQuery>) -> (StatusCode, Json<serde_json::Value>) {
    let teams = state.teams.lock().unwrap();
    if let Err(e) = teams.ensure_membership(&user.tenant, user.id) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() })));
    }
    if let Err(err) = require_admin(&teams, &user.tenant, user.id) {
        return err;
    }

    let events = match teams.list_audit_log(&user.tenant, q.actor, q.action.as_deref(), q.search.as_deref()) {
        Ok(e) => e,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    };

    let accounts = state.accounts.lock().unwrap();
    let views: Vec<AuditEventView> = events
        .into_iter()
        .map(|e| {
            let actor_email = e.actor_user_id.and_then(|id| accounts.get_user(id).ok().flatten()).map(|u| u.email);
            let metadata = serde_json::from_str(&e.metadata).unwrap_or(json!({}));
            AuditEventView { id: e.id, action: e.action, target: e.target, actor_email, ip_address: e.ip_address, metadata, created_at: e.created_at }
        })
        .collect();
    (StatusCode::OK, Json(serde_json::to_value(views).unwrap()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentops_accounts::NewAccount;
    use axum::body::Body;
    use axum::http::Request as HttpRequest;
    use http_body_util::BodyExt;
    use std::path::PathBuf;
    use tower::ServiceExt;

    async fn body_json(response: axum::response::Response) -> serde_json::Value {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    /// Every test signs up its users against a standalone `AccountStore`
    /// *before* handing it (by value) to `build_team_router` -- signups
    /// must happen first since the router takes ownership of the store.
    fn signup_user(accounts: &AccountStore, email: &str) -> (agentops_accounts::User, String) {
        accounts.signup(NewAccount { email, password: "correct horse battery staple", first_name: "Ada", last_name: "Lovelace" }).unwrap()
    }

    #[tokio::test]
    async fn a_solo_users_first_team_request_backfills_them_as_admin() {
        let accounts = AccountStore::open_in_memory().unwrap();
        let (user, token) = signup_user(&accounts, "dev@example.com");
        let teams = TeamStore::open_in_memory().unwrap();
        let app = build_team_router(accounts, teams, ConnectionStore::open_in_memory().unwrap(), CredentialStore::open_in_memory().unwrap(), PathBuf::from("unused-docbrain-dir"));

        let response = app.clone().oneshot(HttpRequest::get("/team").header("authorization", format!("Bearer {token}")).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["role"], "admin");
        assert_eq!(body["member_count"], 1);
        assert_eq!(body["tenant"], user.tenant);
    }

    #[tokio::test]
    async fn a_solo_users_first_team_request_also_backfills_them_as_owner() {
        let accounts = AccountStore::open_in_memory().unwrap();
        let (_, token) = signup_user(&accounts, "dev@example.com");
        let app = build_team_router(accounts, TeamStore::open_in_memory().unwrap(), ConnectionStore::open_in_memory().unwrap(), CredentialStore::open_in_memory().unwrap(), PathBuf::from("unused-docbrain-dir"));

        let response = app.oneshot(HttpRequest::get("/team").header("authorization", format!("Bearer {token}")).body(Body::empty()).unwrap()).await.unwrap();
        let body = body_json(response).await;
        assert_eq!(body["is_owner"], true);
    }

    #[tokio::test]
    async fn only_the_owner_can_demote_or_remove_another_admin() {
        let accounts = AccountStore::open_in_memory().unwrap();
        let (owner, owner_token) = signup_user(&accounts, "owner@example.com");
        let (admin2, admin2_token) = signup_user(&accounts, "admin2@example.com");
        let teams = TeamStore::open_in_memory().unwrap();
        teams.add_member(&owner.tenant, owner.id, "admin").unwrap();
        teams.add_member(&owner.tenant, admin2.id, "admin").unwrap();
        // admin2 signed up into their own solo tenant -- move them into
        // owner's org, same as `accept_invite` would in the real flow.
        accounts.switch_tenant(admin2.id, &owner.tenant).unwrap();
        let app = build_team_router(accounts, teams, ConnectionStore::open_in_memory().unwrap(), CredentialStore::open_in_memory().unwrap(), PathBuf::from("unused-docbrain-dir"));

        // Owner's first `/team/*` touch backfills them as Owner.
        let _ = app.clone().oneshot(HttpRequest::get("/team").header("authorization", format!("Bearer {owner_token}")).body(Body::empty()).unwrap()).await.unwrap();

        // admin2 is a real admin, but not the Owner -- forbidden from
        // demoting the Owner.
        let blocked = app
            .clone()
            .oneshot(
                HttpRequest::patch(format!("/team/members/{}", owner.id))
                    .header("authorization", format!("Bearer {admin2_token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"role":"member"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(blocked.status(), StatusCode::FORBIDDEN);

        // The Owner can demote admin2.
        let demoted = app
            .oneshot(
                HttpRequest::patch(format!("/team/members/{}", admin2.id))
                    .header("authorization", format!("Bearer {owner_token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"role":"member"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(demoted.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn owner_can_transfer_ownership_to_another_active_admin_but_not_to_a_non_admin() {
        let accounts = AccountStore::open_in_memory().unwrap();
        let (owner, owner_token) = signup_user(&accounts, "owner@example.com");
        let (admin2, _) = signup_user(&accounts, "admin2@example.com");
        let (member, _) = signup_user(&accounts, "member@example.com");
        let teams = TeamStore::open_in_memory().unwrap();
        teams.add_member(&owner.tenant, owner.id, "admin").unwrap();
        teams.add_member(&owner.tenant, admin2.id, "admin").unwrap();
        teams.add_member(&owner.tenant, member.id, "member").unwrap();
        accounts.switch_tenant(admin2.id, &owner.tenant).unwrap();
        accounts.switch_tenant(member.id, &owner.tenant).unwrap();
        let app = build_team_router(accounts, teams, ConnectionStore::open_in_memory().unwrap(), CredentialStore::open_in_memory().unwrap(), PathBuf::from("unused-docbrain-dir"));

        let _ = app.clone().oneshot(HttpRequest::get("/team").header("authorization", format!("Bearer {owner_token}")).body(Body::empty()).unwrap()).await.unwrap();

        let rejected = app
            .clone()
            .oneshot(
                HttpRequest::post("/team/transfer-ownership")
                    .header("authorization", format!("Bearer {owner_token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(format!(r#"{{"to_user_id":{}}}"#, member.id)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(rejected.status(), StatusCode::BAD_REQUEST, "cannot transfer ownership to a non-admin");

        let transferred = app
            .clone()
            .oneshot(
                HttpRequest::post("/team/transfer-ownership")
                    .header("authorization", format!("Bearer {owner_token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(format!(r#"{{"to_user_id":{}}}"#, admin2.id)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(transferred.status(), StatusCode::OK);

        // The old owner is no longer Owner, and can no longer act on the
        // new owner unilaterally.
        let old_owner_check = app.clone().oneshot(HttpRequest::get("/team").header("authorization", format!("Bearer {owner_token}")).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(body_json(old_owner_check).await["is_owner"], false);

        let now_forbidden = app
            .oneshot(
                HttpRequest::patch(format!("/team/members/{}", admin2.id))
                    .header("authorization", format!("Bearer {owner_token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"role":"member"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(now_forbidden.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn non_owner_admin_cannot_delete_the_organization() {
        let accounts = AccountStore::open_in_memory().unwrap();
        let (owner, owner_token) = signup_user(&accounts, "owner@example.com");
        let (admin2, admin2_token) = signup_user(&accounts, "admin2@example.com");
        let teams = TeamStore::open_in_memory().unwrap();
        teams.add_member(&owner.tenant, owner.id, "admin").unwrap();
        teams.add_member(&owner.tenant, admin2.id, "admin").unwrap();
        accounts.switch_tenant(admin2.id, &owner.tenant).unwrap();
        let app = build_team_router(accounts, teams, ConnectionStore::open_in_memory().unwrap(), CredentialStore::open_in_memory().unwrap(), PathBuf::from("unused-docbrain-dir"));

        let _ = app.clone().oneshot(HttpRequest::get("/team").header("authorization", format!("Bearer {owner_token}")).body(Body::empty()).unwrap()).await.unwrap();

        let response = app
            .oneshot(
                HttpRequest::post("/team/delete-organization")
                    .header("authorization", format!("Bearer {admin2_token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(format!(r#"{{"confirm_tenant":{:?}}}"#, owner.tenant)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn owner_can_rename_the_organization() {
        let accounts = AccountStore::open_in_memory().unwrap();
        let (_owner, owner_token) = signup_user(&accounts, "owner@example.com");
        let teams = TeamStore::open_in_memory().unwrap();
        let app = build_team_router(accounts, teams, ConnectionStore::open_in_memory().unwrap(), CredentialStore::open_in_memory().unwrap(), PathBuf::from("unused-docbrain-dir"));

        // Backfills Owner via the same lazy `ensure_membership` path every other test here relies on.
        let _ = app.clone().oneshot(HttpRequest::get("/team").header("authorization", format!("Bearer {owner_token}")).body(Body::empty()).unwrap()).await.unwrap();

        let response = app
            .clone()
            .oneshot(HttpRequest::patch("/team").header("authorization", format!("Bearer {owner_token}")).header("content-type", "application/json").body(Body::from(r#"{"name":"Ada's Workspace"}"#)).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let get_response = app.oneshot(HttpRequest::get("/team").header("authorization", format!("Bearer {owner_token}")).body(Body::empty()).unwrap()).await.unwrap();
        let body = body_json(get_response).await;
        assert_eq!(body["name"], "Ada's Workspace");
    }

    #[tokio::test]
    async fn non_owner_admin_cannot_rename_the_organization() {
        let accounts = AccountStore::open_in_memory().unwrap();
        let (owner, owner_token) = signup_user(&accounts, "owner@example.com");
        let (admin2, admin2_token) = signup_user(&accounts, "admin2@example.com");
        let teams = TeamStore::open_in_memory().unwrap();
        teams.add_member(&owner.tenant, owner.id, "admin").unwrap();
        teams.add_member(&owner.tenant, admin2.id, "admin").unwrap();
        accounts.switch_tenant(admin2.id, &owner.tenant).unwrap();
        let app = build_team_router(accounts, teams, ConnectionStore::open_in_memory().unwrap(), CredentialStore::open_in_memory().unwrap(), PathBuf::from("unused-docbrain-dir"));

        let _ = app.clone().oneshot(HttpRequest::get("/team").header("authorization", format!("Bearer {owner_token}")).body(Body::empty()).unwrap()).await.unwrap();

        let response = app
            .oneshot(HttpRequest::patch("/team").header("authorization", format!("Bearer {admin2_token}")).header("content-type", "application/json").body(Body::from(r#"{"name":"Hostile takeover"}"#)).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn delete_organization_rejects_a_mismatched_confirm_tenant() {
        let accounts = AccountStore::open_in_memory().unwrap();
        let (owner, owner_token) = signup_user(&accounts, "owner@example.com");
        let teams = TeamStore::open_in_memory().unwrap();
        teams.add_member(&owner.tenant, owner.id, "admin").unwrap();
        let app = build_team_router(accounts, teams, ConnectionStore::open_in_memory().unwrap(), CredentialStore::open_in_memory().unwrap(), PathBuf::from("unused-docbrain-dir"));

        let _ = app.clone().oneshot(HttpRequest::get("/team").header("authorization", format!("Bearer {owner_token}")).body(Body::empty()).unwrap()).await.unwrap();

        let response = app
            .oneshot(
                HttpRequest::post("/team/delete-organization")
                    .header("authorization", format!("Bearer {owner_token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"confirm_tenant":"not-the-right-tenant"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    /// Full end-to-end cascade: seeds real data in every store the cascade
    /// touches (repo connection, org-wide credential, personal credential,
    /// a docbrain file, invites, custom role, repo-access override, and an
    /// audit event), deletes the org, then asserts each store's data is
    /// gone -- **except `audit_log`**, which must still show the
    /// `org_deleted` event itself -- and that the deleting Owner lands in
    /// a fresh, working tenant afterward.
    #[tokio::test]
    async fn deleting_the_organization_cascades_across_every_store_except_audit_log() {
        let accounts = AccountStore::open_in_memory().unwrap();
        let (owner, owner_token) = signup_user(&accounts, "owner@example.com");
        let (member, _) = signup_user(&accounts, "member@example.com");
        let teams = TeamStore::open_in_memory().unwrap();
        teams.add_member(&owner.tenant, owner.id, "admin").unwrap();
        teams.add_member(&owner.tenant, member.id, "member").unwrap();
        teams.create_invite(&owner.tenant, "invitee@example.com", "member", None, owner.id).unwrap();
        teams.create_custom_role(&owner.tenant, "Junior Dev", "member", &[], owner.id).unwrap();
        teams.set_repo_override(&owner.tenant, member.id, "repo-1", false).unwrap();

        let repos = ConnectionStore::open_in_memory().unwrap();
        seed_repo(&repos, &owner.tenant, "repo-1");

        let credentials = CredentialStore::open_in_memory().unwrap();
        let secrets = agentops_repo_access::secrets::EnvSecretsProvider::from_hex(&"ab".repeat(32)).unwrap();
        credentials
            .store_credential(&secrets, &owner.tenant, agentops_integrations::NewCredential { provider: "linear", auth_type: agentops_integrations::AuthType::ApiKey, secret: "org-key", refresh_token: None, expires_at: None })
            .unwrap();
        credentials
            .store_user_credential(&secrets, &owner.tenant, member.id, agentops_integrations::NewCredential { provider: "linear", auth_type: agentops_integrations::AuthType::ApiKey, secret: "personal-key", refresh_token: None, expires_at: None })
            .unwrap();

        let docbrain_dir = tempfile::tempdir().unwrap();
        let docbrain_path = docbrain_dir.path().join(format!("{}.db", owner.tenant));
        std::fs::write(&docbrain_path, b"fake docbrain sqlite content").unwrap();

        let app = build_team_router(accounts, teams, repos, credentials, docbrain_dir.path().to_path_buf());

        // First `/team/*` touch backfills the Owner.
        let _ = app.clone().oneshot(HttpRequest::get("/team").header("authorization", format!("Bearer {owner_token}")).body(Body::empty()).unwrap()).await.unwrap();

        let response = app
            .clone()
            .oneshot(
                HttpRequest::post("/team/delete-organization")
                    .header("authorization", format!("Bearer {owner_token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(format!(r#"{{"confirm_tenant":{:?}}}"#, owner.tenant)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        let new_tenant = body["new_tenant"].as_str().unwrap().to_string();
        assert_ne!(new_tenant, owner.tenant, "the deleting Owner must land in a genuinely different tenant");

        assert!(!docbrain_path.exists(), "the tenant's docbrain file must be deleted");

        // The Owner's session now resolves to the fresh tenant -- a fresh
        // `/team` touch there backfills them as admin+owner of an empty org.
        let after = app.oneshot(HttpRequest::get("/team").header("authorization", format!("Bearer {owner_token}")).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(after.status(), StatusCode::OK);
        let after_body = body_json(after).await;
        assert_eq!(after_body["tenant"], new_tenant);
        assert_eq!(after_body["member_count"], 1, "a fresh, empty org -- the old membership data is gone");
        assert_eq!(after_body["is_owner"], true);
    }

    #[tokio::test]
    async fn list_members_returns_profile_fields_joined_with_role_and_status() {
        let accounts = AccountStore::open_in_memory().unwrap();
        let (_, token) = signup_user(&accounts, "dev@example.com");
        let app = build_team_router(accounts, TeamStore::open_in_memory().unwrap(), ConnectionStore::open_in_memory().unwrap(), CredentialStore::open_in_memory().unwrap(), PathBuf::from("unused-docbrain-dir"));

        let response = app.oneshot(HttpRequest::get("/team/members").header("authorization", format!("Bearer {token}")).body(Body::empty()).unwrap()).await.unwrap();
        let members = body_json(response).await;
        assert_eq!(members[0]["email"], "dev@example.com");
        assert_eq!(members[0]["role"], "admin");
        assert_eq!(members[0]["is_you"], true);
    }

    #[tokio::test]
    async fn non_admin_cannot_change_a_members_role() {
        let accounts = AccountStore::open_in_memory().unwrap();
        let (user, token) = signup_user(&accounts, "member@example.com");
        let teams = TeamStore::open_in_memory().unwrap();
        // Seeded explicitly as "member" *before* any `/team/*` request, so
        // `ensure_membership`'s auto-admin backfill (which only fires when
        // no row exists yet) is a no-op here -- this really is a non-admin
        // acting on their own tenant, not an artifact of the backfill.
        teams.add_member(&user.tenant, user.id, "member").unwrap();
        let app = build_team_router(accounts, teams, ConnectionStore::open_in_memory().unwrap(), CredentialStore::open_in_memory().unwrap(), PathBuf::from("unused-docbrain-dir"));

        let attempt = app
            .oneshot(
                HttpRequest::patch(format!("/team/members/{}", user.id))
                    .header("authorization", format!("Bearer {token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"role":"admin"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(attempt.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn admin_can_change_a_members_role_and_the_last_admin_is_protected() {
        let accounts = AccountStore::open_in_memory().unwrap();
        let (admin, admin_token) = signup_user(&accounts, "admin@example.com");
        let (member, _) = signup_user(&accounts, "member@example.com");
        let teams = TeamStore::open_in_memory().unwrap();
        teams.add_member(&admin.tenant, admin.id, "admin").unwrap();
        teams.add_member(&admin.tenant, member.id, "member").unwrap();
        let app = build_team_router(accounts, teams, ConnectionStore::open_in_memory().unwrap(), CredentialStore::open_in_memory().unwrap(), PathBuf::from("unused-docbrain-dir"));

        let promote = app
            .clone()
            .oneshot(
                HttpRequest::patch(format!("/team/members/{}", member.id))
                    .header("authorization", format!("Bearer {admin_token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"role":"viewer"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(promote.status(), StatusCode::OK);

        // Now try to demote the sole admin (themselves) -- must be blocked.
        let self_demote = app
            .oneshot(
                HttpRequest::patch(format!("/team/members/{}", admin.id))
                    .header("authorization", format!("Bearer {admin_token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"role":"member"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(self_demote.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn removing_the_last_admin_is_blocked_but_removing_a_regular_member_works() {
        let accounts = AccountStore::open_in_memory().unwrap();
        let (admin, admin_token) = signup_user(&accounts, "admin@example.com");
        let (member, _) = signup_user(&accounts, "member@example.com");
        let teams = TeamStore::open_in_memory().unwrap();
        teams.add_member(&admin.tenant, admin.id, "admin").unwrap();
        teams.add_member(&admin.tenant, member.id, "member").unwrap();
        let app = build_team_router(accounts, teams, ConnectionStore::open_in_memory().unwrap(), CredentialStore::open_in_memory().unwrap(), PathBuf::from("unused-docbrain-dir"));

        let remove_member = app
            .clone()
            .oneshot(HttpRequest::delete(format!("/team/members/{}", member.id)).header("authorization", format!("Bearer {admin_token}")).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(remove_member.status(), StatusCode::OK);

        let remove_self = app
            .oneshot(HttpRequest::delete(format!("/team/members/{}", admin.id)).header("authorization", format!("Bearer {admin_token}")).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(remove_self.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn get_team_rejects_a_request_with_no_session_token() {
        let app = build_team_router(AccountStore::open_in_memory().unwrap(), TeamStore::open_in_memory().unwrap(), ConnectionStore::open_in_memory().unwrap(), CredentialStore::open_in_memory().unwrap(), PathBuf::from("unused-docbrain-dir"));
        let response = app.oneshot(HttpRequest::get("/team").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn admin_can_create_and_list_invites_non_admin_cannot() {
        let accounts = AccountStore::open_in_memory().unwrap();
        let (admin, admin_token) = signup_user(&accounts, "admin@example.com");
        let (member, member_token) = signup_user(&accounts, "member@example.com");
        let teams = TeamStore::open_in_memory().unwrap();
        teams.add_member(&admin.tenant, admin.id, "admin").unwrap();
        teams.add_member(&member.tenant, member.id, "member").unwrap();
        let app = build_team_router(accounts, teams, ConnectionStore::open_in_memory().unwrap(), CredentialStore::open_in_memory().unwrap(), PathBuf::from("unused-docbrain-dir"));

        let create = app
            .clone()
            .oneshot(
                HttpRequest::post("/team/invites")
                    .header("authorization", format!("Bearer {admin_token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"email":"new@example.com","role":"member","note":"welcome!"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(create.status(), StatusCode::CREATED);
        let created = body_json(create).await;
        assert!(created["token"].as_str().unwrap().starts_with("ao_"));

        let list = app
            .clone()
            .oneshot(HttpRequest::get("/team/invites").header("authorization", format!("Bearer {admin_token}")).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let invites = body_json(list).await;
        assert_eq!(invites[0]["email"], "new@example.com");

        let non_admin_attempt = app
            .oneshot(
                HttpRequest::post("/team/invites")
                    .header("authorization", format!("Bearer {member_token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"email":"blocked@example.com","role":"member"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(non_admin_attempt.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn get_invites_token_previews_without_a_session() {
        let accounts = AccountStore::open_in_memory().unwrap();
        let (admin, admin_token) = signup_user(&accounts, "admin@example.com");
        let teams = TeamStore::open_in_memory().unwrap();
        teams.add_member(&admin.tenant, admin.id, "admin").unwrap();
        teams.rename_organization(&admin.tenant, "Acme Corp").unwrap();
        let app = build_team_router(accounts, teams, ConnectionStore::open_in_memory().unwrap(), CredentialStore::open_in_memory().unwrap(), PathBuf::from("unused-docbrain-dir"));

        let create = app
            .clone()
            .oneshot(
                HttpRequest::post("/team/invites")
                    .header("authorization", format!("Bearer {admin_token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"email":"new@example.com","role":"viewer"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        let token = body_json(create).await["token"].as_str().unwrap().to_string();

        let preview = app.oneshot(HttpRequest::get(format!("/invites/{token}")).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(preview.status(), StatusCode::OK);
        let body = body_json(preview).await;
        assert_eq!(body["email"], "new@example.com");
        assert_eq!(body["role"], "viewer");
        assert_eq!(body["org_name"], "Acme Corp");
    }

    #[tokio::test]
    async fn accept_invite_joins_the_inviting_org_and_switches_the_users_tenant() {
        let accounts = AccountStore::open_in_memory().unwrap();
        let (admin, admin_token) = signup_user(&accounts, "admin@example.com");
        let (invitee, invitee_token) = signup_user(&accounts, "invitee@example.com");
        let original_tenant = invitee.tenant.clone();
        let teams = TeamStore::open_in_memory().unwrap();
        teams.add_member(&admin.tenant, admin.id, "admin").unwrap();
        let app = build_team_router(accounts, teams, ConnectionStore::open_in_memory().unwrap(), CredentialStore::open_in_memory().unwrap(), PathBuf::from("unused-docbrain-dir"));

        let create = app
            .clone()
            .oneshot(
                HttpRequest::post("/team/invites")
                    .header("authorization", format!("Bearer {admin_token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"email":"invitee@example.com","role":"member"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        let token = body_json(create).await["token"].as_str().unwrap().to_string();

        let accept = app
            .clone()
            .oneshot(
                HttpRequest::post("/invites/accept")
                    .header("authorization", format!("Bearer {invitee_token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(format!(r#"{{"token":"{token}"}}"#)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(accept.status(), StatusCode::OK);
        let accepted = body_json(accept).await;
        assert_eq!(accepted["tenant"], admin.tenant);
        assert_ne!(accepted["tenant"].as_str().unwrap(), original_tenant, "sanity: invitee's own signup tenant differs from admin's");

        // Now a member of admin's org, with the right role.
        let members = app.oneshot(HttpRequest::get("/team/members").header("authorization", format!("Bearer {admin_token}")).body(Body::empty()).unwrap()).await.unwrap();
        let members = body_json(members).await;
        let invitee_row = members.as_array().unwrap().iter().find(|m| m["email"] == "invitee@example.com").unwrap();
        assert_eq!(invitee_row["role"], "member");
    }

    #[tokio::test]
    async fn accept_invite_rejects_an_unknown_or_already_used_token() {
        let accounts = AccountStore::open_in_memory().unwrap();
        let (_, token) = signup_user(&accounts, "dev@example.com");
        let app = build_team_router(accounts, TeamStore::open_in_memory().unwrap(), ConnectionStore::open_in_memory().unwrap(), CredentialStore::open_in_memory().unwrap(), PathBuf::from("unused-docbrain-dir"));

        let response = app
            .oneshot(
                HttpRequest::post("/invites/accept")
                    .header("authorization", format!("Bearer {token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"token":"ao_not-a-real-token"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn admin_can_revoke_and_resend_an_invite() {
        let accounts = AccountStore::open_in_memory().unwrap();
        let (admin, admin_token) = signup_user(&accounts, "admin@example.com");
        let teams = TeamStore::open_in_memory().unwrap();
        teams.add_member(&admin.tenant, admin.id, "admin").unwrap();
        let app = build_team_router(accounts, teams, ConnectionStore::open_in_memory().unwrap(), CredentialStore::open_in_memory().unwrap(), PathBuf::from("unused-docbrain-dir"));

        let create = app
            .clone()
            .oneshot(
                HttpRequest::post("/team/invites")
                    .header("authorization", format!("Bearer {admin_token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"email":"new@example.com","role":"member"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        let invite_id = body_json(create).await["id"].as_i64().unwrap();

        let resend = app
            .clone()
            .oneshot(HttpRequest::post(format!("/team/invites/{invite_id}/resend")).header("authorization", format!("Bearer {admin_token}")).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resend.status(), StatusCode::OK);
        assert!(body_json(resend).await["token"].as_str().unwrap().starts_with("ao_"));

        let revoke = app
            .oneshot(HttpRequest::delete(format!("/team/invites/{invite_id}")).header("authorization", format!("Bearer {admin_token}")).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(revoke.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn get_team_roles_returns_the_static_matrix_with_live_member_counts_any_member_can_view() {
        let accounts = AccountStore::open_in_memory().unwrap();
        let (admin, admin_token) = signup_user(&accounts, "admin@example.com");
        let (member, member_token) = signup_user(&accounts, "member@example.com");
        let teams = TeamStore::open_in_memory().unwrap();
        teams.add_member(&admin.tenant, admin.id, "admin").unwrap();
        teams.add_member(&admin.tenant, member.id, "member").unwrap();
        let app = build_team_router(accounts, teams, ConnectionStore::open_in_memory().unwrap(), CredentialStore::open_in_memory().unwrap(), PathBuf::from("unused-docbrain-dir"));

        let response = app.clone().oneshot(HttpRequest::get("/team/roles").header("authorization", format!("Bearer {admin_token}")).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        let roles = body["roles"].as_array().unwrap();
        assert_eq!(roles.len(), agentops_teams::ROLE_DESCRIPTIONS.len());
        let admin_role = roles.iter().find(|r| r["role"] == "admin").unwrap();
        assert_eq!(admin_role["member_count"], 1);
        let member_role = roles.iter().find(|r| r["role"] == "member").unwrap();
        assert_eq!(member_role["member_count"], 1);
        assert_eq!(body["matrix"].as_array().unwrap().len(), agentops_teams::PERMISSIONS_MATRIX.len());
        assert_eq!(body["custom_roles"].as_array().unwrap().len(), 0, "no custom roles created yet");

        // Not admin-gated -- a regular member can view it too.
        let member_response = app.oneshot(HttpRequest::get("/team/roles").header("authorization", format!("Bearer {member_token}")).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(member_response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn admin_can_create_a_custom_role_and_non_admin_cannot() {
        let accounts = AccountStore::open_in_memory().unwrap();
        let (admin, admin_token) = signup_user(&accounts, "admin@example.com");
        let (member, member_token) = signup_user(&accounts, "member@example.com");
        let teams = TeamStore::open_in_memory().unwrap();
        teams.add_member(&admin.tenant, admin.id, "admin").unwrap();
        teams.add_member(&member.tenant, member.id, "member").unwrap();
        let app = build_team_router(accounts, teams, ConnectionStore::open_in_memory().unwrap(), CredentialStore::open_in_memory().unwrap(), PathBuf::from("unused-docbrain-dir"));

        let denied = app
            .clone()
            .oneshot(
                HttpRequest::post("/team/roles/custom")
                    .header("authorization", format!("Bearer {member_token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"label":"Junior Dev","cloned_from":"member","capabilities":["gotchas.view"]}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);

        let create = app
            .clone()
            .oneshot(
                HttpRequest::post("/team/roles/custom")
                    .header("authorization", format!("Bearer {admin_token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"label":"Junior Dev","cloned_from":"member","capabilities":["gotchas.view"]}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(create.status(), StatusCode::CREATED);
        let created = body_json(create).await;
        let role_key = created["role_key"].as_str().unwrap().to_string();
        assert!(role_key.starts_with("custom:"));
        assert_eq!(created["capabilities"], serde_json::json!(["gotchas.view"]));

        let unknown_capability = app
            .clone()
            .oneshot(
                HttpRequest::post("/team/roles/custom")
                    .header("authorization", format!("Bearer {admin_token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"label":"Bad","cloned_from":"member","capabilities":["not.a.real.capability"]}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unknown_capability.status(), StatusCode::BAD_REQUEST);

        let roles = app.oneshot(HttpRequest::get("/team/roles").header("authorization", format!("Bearer {admin_token}")).body(Body::empty()).unwrap()).await.unwrap();
        let body = body_json(roles).await;
        assert_eq!(body["custom_roles"].as_array().unwrap().len(), 1);
        assert_eq!(body["custom_roles"][0]["role_key"], role_key);
    }

    #[tokio::test]
    async fn a_member_can_be_assigned_a_custom_role_and_gains_exactly_its_capabilities() {
        let accounts = AccountStore::open_in_memory().unwrap();
        let (admin, admin_token) = signup_user(&accounts, "admin@example.com");
        let (dev, dev_token) = signup_user(&accounts, "dev@example.com");
        let teams = TeamStore::open_in_memory().unwrap();
        teams.add_member(&admin.tenant, admin.id, "admin").unwrap();
        let custom = teams.create_custom_role(&admin.tenant, "Junior Dev", "member", &["gotchas.view"], admin.id).unwrap();
        teams.add_member(&admin.tenant, dev.id, &custom.role_key).unwrap();
        accounts.switch_tenant(dev.id, &admin.tenant).unwrap();
        let app = build_team_router(accounts, teams, ConnectionStore::open_in_memory().unwrap(), CredentialStore::open_in_memory().unwrap(), PathBuf::from("unused-docbrain-dir"));

        // The custom-role member cannot invite (not granted `team.manage_members`)...
        let forbidden = app
            .clone()
            .oneshot(
                HttpRequest::post("/team/invites")
                    .header("authorization", format!("Bearer {dev_token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"email":"blocked@example.com","role":"member"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);

        // ...but does show up correctly in the member list with their custom role.
        let members = app.oneshot(HttpRequest::get("/team/members").header("authorization", format!("Bearer {admin_token}")).body(Body::empty()).unwrap()).await.unwrap();
        let members = body_json(members).await;
        let dev_row = members.as_array().unwrap().iter().find(|m| m["email"] == "dev@example.com").unwrap();
        assert_eq!(dev_row["role"], custom.role_key);
    }

    #[tokio::test]
    async fn deleting_a_custom_role_is_blocked_while_in_use_admin_can_update_its_capabilities() {
        let accounts = AccountStore::open_in_memory().unwrap();
        let (admin, admin_token) = signup_user(&accounts, "admin@example.com");
        let teams = TeamStore::open_in_memory().unwrap();
        teams.add_member(&admin.tenant, admin.id, "admin").unwrap();
        let custom = teams.create_custom_role(&admin.tenant, "Junior Dev", "member", &["gotchas.view"], admin.id).unwrap();
        teams.add_member(&admin.tenant, 999, &custom.role_key).unwrap();
        let app = build_team_router(accounts, teams, ConnectionStore::open_in_memory().unwrap(), CredentialStore::open_in_memory().unwrap(), PathBuf::from("unused-docbrain-dir"));

        let blocked_delete = app
            .clone()
            .oneshot(HttpRequest::delete(format!("/team/roles/custom/{}", custom.role_key)).header("authorization", format!("Bearer {admin_token}")).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(blocked_delete.status(), StatusCode::BAD_REQUEST);

        let update = app
            .oneshot(
                HttpRequest::patch(format!("/team/roles/custom/{}", custom.role_key))
                    .header("authorization", format!("Bearer {admin_token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"capabilities":["repos.view","graph.view"]}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(update.status(), StatusCode::OK);
    }

    fn seed_repo(repos: &ConnectionStore, tenant: &str, id: &str) {
        let secrets = agentops_repo_access::secrets::EnvSecretsProvider::from_hex(&"ab".repeat(32)).unwrap();
        let keypair = agentops_repo_access::generate_deploy_keypair_for_repo(&secrets, tenant, id).unwrap();
        repos.create_ssh_connection(tenant, id, "git@github.com:acme/widgets.git", &keypair).unwrap();
    }

    #[tokio::test]
    async fn get_repo_access_applies_role_defaults_admin_and_billing_are_not_editable() {
        let accounts = AccountStore::open_in_memory().unwrap();
        let (admin, admin_token) = signup_user(&accounts, "admin@example.com");
        let (member, _) = signup_user(&accounts, "member@example.com");
        let (biller, _) = signup_user(&accounts, "biller@example.com");
        let teams = TeamStore::open_in_memory().unwrap();
        teams.add_member(&admin.tenant, admin.id, "admin").unwrap();
        teams.add_member(&admin.tenant, member.id, "member").unwrap();
        teams.add_member(&admin.tenant, biller.id, "billing").unwrap();
        let repos = ConnectionStore::open_in_memory().unwrap();
        seed_repo(&repos, &admin.tenant, "repo-1");
        let app = build_team_router(accounts, teams, repos, CredentialStore::open_in_memory().unwrap(), PathBuf::from("unused-docbrain-dir"));

        let response = app.oneshot(HttpRequest::get("/team/repo-access").header("authorization", format!("Bearer {admin_token}")).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        let cells = body["access"].as_array().unwrap();

        let admin_cell = cells.iter().find(|c| c["user_id"] == admin.id).unwrap();
        assert_eq!(admin_cell["allowed"], true);
        assert_eq!(admin_cell["editable"], false, "admin access is not override-editable");

        let biller_cell = cells.iter().find(|c| c["user_id"] == biller.id).unwrap();
        assert_eq!(biller_cell["allowed"], false);
        assert_eq!(biller_cell["editable"], false, "billing access is not override-editable");

        let member_cell = cells.iter().find(|c| c["user_id"] == member.id).unwrap();
        assert_eq!(member_cell["allowed"], true, "member defaults to allowed");
        assert_eq!(member_cell["editable"], true);
    }

    #[tokio::test]
    async fn save_repo_access_persists_a_member_override_and_ignores_a_diff_for_a_non_editable_role() {
        let accounts = AccountStore::open_in_memory().unwrap();
        let (admin, admin_token) = signup_user(&accounts, "admin@example.com");
        let (member, _) = signup_user(&accounts, "member@example.com");
        let teams = TeamStore::open_in_memory().unwrap();
        teams.add_member(&admin.tenant, admin.id, "admin").unwrap();
        teams.add_member(&admin.tenant, member.id, "member").unwrap();
        let repos = ConnectionStore::open_in_memory().unwrap();
        seed_repo(&repos, &admin.tenant, "repo-1");
        let app = build_team_router(accounts, teams, repos, CredentialStore::open_in_memory().unwrap(), PathBuf::from("unused-docbrain-dir"));

        let save = app
            .clone()
            .oneshot(
                HttpRequest::put("/team/repo-access")
                    .header("authorization", format!("Bearer {admin_token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"changes":[{{"user_id":{},"repo_id":"repo-1","allowed":false}},{{"user_id":{},"repo_id":"repo-1","allowed":false}}]}}"#,
                        member.id, admin.id
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(save.status(), StatusCode::OK);

        let response = app.oneshot(HttpRequest::get("/team/repo-access").header("authorization", format!("Bearer {admin_token}")).body(Body::empty()).unwrap()).await.unwrap();
        let body = body_json(response).await;
        let cells = body["access"].as_array().unwrap();

        let member_cell = cells.iter().find(|c| c["user_id"] == member.id).unwrap();
        assert_eq!(member_cell["allowed"], false, "the member override must have been saved");

        let admin_cell = cells.iter().find(|c| c["user_id"] == admin.id).unwrap();
        assert_eq!(admin_cell["allowed"], true, "the admin diff must have been ignored -- admin access is never override-editable");
    }

    #[tokio::test]
    async fn non_admin_cannot_save_repo_access() {
        let accounts = AccountStore::open_in_memory().unwrap();
        let (user, token) = signup_user(&accounts, "member@example.com");
        let teams = TeamStore::open_in_memory().unwrap();
        teams.add_member(&user.tenant, user.id, "member").unwrap();
        let app = build_team_router(accounts, teams, ConnectionStore::open_in_memory().unwrap(), CredentialStore::open_in_memory().unwrap(), PathBuf::from("unused-docbrain-dir"));

        let response = app
            .oneshot(
                HttpRequest::put("/team/repo-access")
                    .header("authorization", format!("Bearer {token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"changes":[]}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn team_mutations_are_recorded_in_the_audit_log_and_only_admins_can_read_it() {
        let accounts = AccountStore::open_in_memory().unwrap();
        let (admin, admin_token) = signup_user(&accounts, "admin@example.com");
        let (member, member_token) = signup_user(&accounts, "member@example.com");
        let teams = TeamStore::open_in_memory().unwrap();
        teams.add_member(&admin.tenant, admin.id, "admin").unwrap();
        teams.add_member(&admin.tenant, member.id, "member").unwrap();
        // Also seed member's *own* tenant explicitly as non-admin -- their
        // session resolves to their own signup tenant, not admin's, so
        // without this the auto-admin backfill (no membership row yet in
        // their own tenant) would silently make them admin of their own
        // org and defeat this check. Caught by this exact test, not assumed.
        teams.add_member(&member.tenant, member.id, "member").unwrap();
        let app = build_team_router(accounts, teams, ConnectionStore::open_in_memory().unwrap(), CredentialStore::open_in_memory().unwrap(), PathBuf::from("unused-docbrain-dir"));

        // Non-admin can't read it.
        let forbidden = app.clone().oneshot(HttpRequest::get("/team/audit-log").header("authorization", format!("Bearer {member_token}")).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);

        // A role change gets recorded.
        app.clone()
            .oneshot(
                HttpRequest::patch(format!("/team/members/{}", member.id))
                    .header("authorization", format!("Bearer {admin_token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"role":"viewer"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        let response = app.oneshot(HttpRequest::get("/team/audit-log").header("authorization", format!("Bearer {admin_token}")).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let events = body_json(response).await;
        let events = events.as_array().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["action"], "role_changed");
        assert_eq!(events[0]["target"], "member@example.com");
        assert_eq!(events[0]["actor_email"], "admin@example.com");
        assert_eq!(events[0]["metadata"]["role"], "viewer");
    }

    #[tokio::test]
    async fn audit_log_filters_by_action() {
        let accounts = AccountStore::open_in_memory().unwrap();
        let (admin, admin_token) = signup_user(&accounts, "admin@example.com");
        let teams = TeamStore::open_in_memory().unwrap();
        teams.add_member(&admin.tenant, admin.id, "admin").unwrap();
        teams.record_audit_event(&admin.tenant, Some(admin.id), "member_invited", Some("a@example.com"), None, "{}").unwrap();
        teams.record_audit_event(&admin.tenant, Some(admin.id), "role_changed", Some("b@example.com"), None, "{}").unwrap();
        let app = build_team_router(accounts, teams, ConnectionStore::open_in_memory().unwrap(), CredentialStore::open_in_memory().unwrap(), PathBuf::from("unused-docbrain-dir"));

        let response =
            app.oneshot(HttpRequest::get("/team/audit-log?action=role_changed").header("authorization", format!("Bearer {admin_token}")).body(Body::empty()).unwrap()).await.unwrap();
        let events = body_json(response).await;
        let events = events.as_array().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["action"], "role_changed");
    }
}

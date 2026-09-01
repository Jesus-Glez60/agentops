//! Organization/membership concerns for Profile + Team Management --
//! deliberately a separate crate/store from `agentops-accounts` (identity
//! and auth only), the same separation-of-concerns `agentops-repo-access`
//! already established for repo connections rather than folding everything
//! into one crate. `tenant` here is exactly the same string as
//! `agentops_accounts::User::tenant`: an organization *is* a tenant, and
//! (since `agentops-accounts`' tenant-uniqueness-relax migration) a tenant
//! can now have more than one member.
//!
//! `users` and `memberships` live in separate SQLite files, so there is no
//! SQL join between them anywhere in this crate -- callers that need
//! member names/emails alongside role/status (the Members tab) fetch
//! `User`s from `agentops-accounts` and merge in Rust at the HTTP layer.

use std::path::Path;

use agentops_security::api_key::{generate_api_key, hash_api_key};
use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension};

mod capabilities;
pub use capabilities::{effective_repo_access, has_capability, role_default_repo_access};

pub struct TeamStore {
    conn: Connection,
}

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS organizations (
        tenant TEXT PRIMARY KEY,
        name   TEXT NOT NULL DEFAULT ''
    );
    CREATE TABLE IF NOT EXISTS memberships (
        id        INTEGER PRIMARY KEY AUTOINCREMENT,
        tenant    TEXT NOT NULL,
        user_id   INTEGER NOT NULL,
        role      TEXT NOT NULL,
        status    TEXT NOT NULL DEFAULT 'active',
        joined_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
        UNIQUE(tenant, user_id)
    );
    CREATE INDEX IF NOT EXISTS idx_memberships_tenant ON memberships(tenant);
    CREATE INDEX IF NOT EXISTS idx_memberships_user ON memberships(user_id);
    CREATE TABLE IF NOT EXISTS invites (
        id                 INTEGER PRIMARY KEY AUTOINCREMENT,
        tenant             TEXT NOT NULL,
        email              TEXT NOT NULL,
        role               TEXT NOT NULL,
        note               TEXT,
        token_hash         TEXT NOT NULL UNIQUE,
        invited_by_user_id INTEGER NOT NULL,
        status             TEXT NOT NULL DEFAULT 'pending',
        created_at         TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
        expires_at         TEXT NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_invites_tenant ON invites(tenant);
    CREATE TABLE IF NOT EXISTS repo_access_overrides (
        tenant   TEXT NOT NULL,
        user_id  INTEGER NOT NULL,
        repo_id  TEXT NOT NULL,
        allowed  INTEGER NOT NULL,
        PRIMARY KEY (tenant, user_id, repo_id)
    );
    CREATE TABLE IF NOT EXISTS custom_roles (
        id                 INTEGER PRIMARY KEY AUTOINCREMENT,
        tenant             TEXT NOT NULL,
        role_key           TEXT NOT NULL,
        label              TEXT NOT NULL,
        cloned_from        TEXT NOT NULL,
        capabilities       TEXT NOT NULL DEFAULT '',
        created_by_user_id INTEGER NOT NULL,
        created_at         TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
        UNIQUE(tenant, role_key)
    );
    CREATE INDEX IF NOT EXISTS idx_custom_roles_tenant ON custom_roles(tenant);
    CREATE TABLE IF NOT EXISTS audit_log (
        id            INTEGER PRIMARY KEY AUTOINCREMENT,
        tenant        TEXT NOT NULL,
        actor_user_id INTEGER,
        action        TEXT NOT NULL,
        target        TEXT,
        ip_address    TEXT,
        metadata      TEXT NOT NULL DEFAULT '{}',
        created_at    TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
    );
    CREATE INDEX IF NOT EXISTS idx_audit_log_tenant_created ON audit_log(tenant, created_at DESC);
";

/// Additive `organizations.owner_user_id` column, added after this crate's
/// initial ship -- same "check `PRAGMA table_info`, `ALTER TABLE ADD COLUMN`
/// if missing" idiom `agentops-accounts` already established for its own
/// post-ship columns (`migrate_add_profile_columns`), so a pre-existing
/// `teams.sqlite` with no `owner_user_id` column gets it transparently on
/// next open rather than failing schema init.
fn migrate_add_owner_column(conn: &Connection) -> Result<()> {
    let mut existing_columns = std::collections::HashSet::new();
    {
        let mut stmt = conn.prepare("PRAGMA table_info(organizations)")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            existing_columns.insert(row.get::<_, String>(1)?);
        }
    }
    if !existing_columns.contains("owner_user_id") {
        conn.execute("ALTER TABLE organizations ADD COLUMN owner_user_id INTEGER", [])?;
    }
    Ok(())
}

/// Additive `organizations.mcp_access_mode` column -- same idiom as
/// `migrate_add_owner_column` just above. `DEFAULT 'advisor'`: a brand-new
/// org (and every pre-existing one, transparently, on next open) starts
/// read-only-for-write-tools until an admin explicitly opts in to `'full'`
/// -- matches the deployment-level `AGENTOPS_ACCESS_MODE` env var's own
/// existing default, just made per-tenant and admin-toggleable instead of
/// operator-only.
fn migrate_add_mcp_access_mode_column(conn: &Connection) -> Result<()> {
    let mut existing_columns = std::collections::HashSet::new();
    {
        let mut stmt = conn.prepare("PRAGMA table_info(organizations)")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            existing_columns.insert(row.get::<_, String>(1)?);
        }
    }
    if !existing_columns.contains("mcp_access_mode") {
        conn.execute("ALTER TABLE organizations ADD COLUMN mcp_access_mode TEXT NOT NULL DEFAULT 'advisor'", [])?;
    }
    Ok(())
}

/// Invite links are valid for this long from creation/resend -- long
/// enough for a real person to notice an email/Slack message and click it,
/// short enough that a copy-pasted link left in some old chat log isn't a
/// standing door into the organization.
const INVITE_LIFETIME_DAYS: i64 = 7;

/// Fixed, non-tenant-scoped role enum -- there is no custom-role concept
/// here (matches the Team Management mockup's own "read-only, contact
/// support for custom roles" copy), so an invalid role string passed to
/// `add_member`/`update_member` is a caller bug to reject loudly, not user
/// input to tolerate.
pub const ROLES: &[&str] = &["admin", "member", "viewer", "billing"];

pub struct RoleDescription {
    pub role: &'static str,
    pub label: &'static str,
    pub description: &'static str,
}

/// One row per role, in the same order as `ROLES` -- rendered as the
/// Roles & Permissions tab's role cards.
pub const ROLE_DESCRIPTIONS: &[RoleDescription] = &[
    RoleDescription { role: "admin", label: "Admin", description: "Full access to all features, repositories, settings, and member management." },
    RoleDescription { role: "member", label: "Member", description: "Standard developer access — read/write to shared resources, no admin controls." },
    RoleDescription { role: "viewer", label: "Viewer", description: "Read-only access to documentation, search, and gotcha reports. Cannot modify anything." },
    RoleDescription { role: "billing", label: "Billing", description: "Access limited to billing, invoices, and seat management. No code or data access." },
];

pub struct Capability {
    pub key: &'static str,
    pub feature_area: &'static str,
    pub label: &'static str,
    pub allowed_roles: &'static [&'static str],
}

// Capability key constants -- one per `PERMISSIONS_MATRIX` row, used by
// route handlers instead of ad hoc string literals so a typo shows up as a
// compile error, not a route that silently 403s everyone (or nobody).
pub const CAP_REPOS_CONNECT: &str = "repos.connect";
pub const CAP_REPOS_REINDEX: &str = "repos.reindex";
pub const CAP_REPOS_VIEW: &str = "repos.view";
pub const CAP_SEARCH_QUERY: &str = "search.query";
pub const CAP_GRAPH_VIEW: &str = "graph.view";
pub const CAP_GOTCHAS_VIEW: &str = "gotchas.view";
pub const CAP_GOTCHAS_COMMENT: &str = "gotchas.comment";
pub const CAP_GOTCHAS_RESOLVE: &str = "gotchas.resolve";
pub const CAP_API_KEYS_PERSONAL: &str = "api_keys.personal";
pub const CAP_API_KEYS_ORG: &str = "api_keys.org";
pub const CAP_TEAM_MANAGE_MEMBERS: &str = "team.manage_members";
pub const CAP_TEAM_MANAGE_ROLES: &str = "team.manage_roles";
pub const CAP_TEAM_BILLING: &str = "team.billing";
pub const CAP_INTEGRATIONS_MANAGE: &str = "integrations.manage";
pub const CAP_MCP_MANAGE_ACCESS_MODE: &str = "mcp.manage_access_mode";

/// The permissions matrix -- **the single source of truth** for what each
/// role can do, for both the displayed Roles & Permissions matrix and
/// route-level enforcement via `has_capability` (`capabilities.rs`) -- one
/// definition, so the two can never drift apart.
pub const PERMISSIONS_MATRIX: &[Capability] = &[
    Capability { key: CAP_REPOS_CONNECT, feature_area: "Repositories", label: "Connect / disconnect repository", allowed_roles: &["admin"] },
    Capability { key: CAP_REPOS_REINDEX, feature_area: "Repositories", label: "Trigger manual re-index", allowed_roles: &["admin", "member"] },
    Capability { key: CAP_REPOS_VIEW, feature_area: "Repositories", label: "View repository overview & index status", allowed_roles: &["admin", "member", "viewer"] },
    Capability { key: CAP_SEARCH_QUERY, feature_area: "Search & Knowledge Graph", label: "Run semantic search queries", allowed_roles: &["admin", "member", "viewer"] },
    Capability { key: CAP_GRAPH_VIEW, feature_area: "Search & Knowledge Graph", label: "View knowledge graph & node details", allowed_roles: &["admin", "member", "viewer"] },
    Capability { key: CAP_GOTCHAS_VIEW, feature_area: "Gotchas", label: "View gotcha list & detail", allowed_roles: &["admin", "member", "viewer"] },
    Capability { key: CAP_GOTCHAS_COMMENT, feature_area: "Gotchas", label: "Comment on gotchas", allowed_roles: &["admin", "member"] },
    Capability { key: CAP_GOTCHAS_RESOLVE, feature_area: "Gotchas", label: "Resolve or dismiss gotchas", allowed_roles: &["admin", "member"] },
    Capability { key: CAP_API_KEYS_PERSONAL, feature_area: "API & Keys", label: "Generate personal API keys", allowed_roles: &["admin", "member"] },
    Capability { key: CAP_API_KEYS_ORG, feature_area: "API & Keys", label: "View & revoke org-wide API keys", allowed_roles: &["admin"] },
    Capability { key: CAP_TEAM_MANAGE_MEMBERS, feature_area: "Team & Billing", label: "Invite & remove members", allowed_roles: &["admin"] },
    Capability { key: CAP_TEAM_MANAGE_ROLES, feature_area: "Team & Billing", label: "Assign & change member roles", allowed_roles: &["admin"] },
    Capability { key: CAP_TEAM_BILLING, feature_area: "Team & Billing", label: "Manage billing & subscriptions", allowed_roles: &["admin", "billing"] },
    Capability { key: CAP_INTEGRATIONS_MANAGE, feature_area: "Integrations", label: "Connect, view & remove org-wide integration credentials", allowed_roles: &["admin"] },
    Capability { key: CAP_MCP_MANAGE_ACCESS_MODE, feature_area: "MCP", label: "Change MCP access mode (Advisor/Full)", allowed_roles: &["admin"] },
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Membership {
    pub user_id: i64,
    pub tenant: String,
    pub role: String,
    pub status: String,
    pub joined_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InviteInfo {
    pub id: i64,
    pub tenant: String,
    pub email: String,
    pub role: String,
    pub note: Option<String>,
    pub invited_by_user_id: i64,
    pub status: String,
    pub created_at: String,
    pub expires_at: String,
}

/// What the public, unauthenticated `GET /invites/{token}` preview
/// exposes -- deliberately narrower than `InviteInfo` (no `id`, no
/// `invited_by_user_id`, no `note`), since anyone with the link can see
/// this before logging in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvitePreview {
    pub tenant: String,
    pub email: String,
    pub role: String,
}

/// A tenant-scoped custom role: clone one of the 4 fixed `ROLES` and give
/// it its own label + capability set. `role_key` is always `custom:{id}`
/// -- the `custom:` prefix is what lets every existing bare-string
/// `ROLES.contains`/`role == "admin"` check keep working unmodified for
/// the 4 fixed roles, since a custom role's `role` column value never
/// collides with one of those literals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomRole {
    pub id: i64,
    pub tenant: String,
    pub role_key: String,
    pub label: String,
    pub cloned_from: String,
    pub capabilities: Vec<String>,
    pub created_by_user_id: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEvent {
    pub id: i64,
    pub tenant: String,
    pub actor_user_id: Option<i64>,
    pub action: String,
    pub target: Option<String>,
    pub ip_address: Option<String>,
    pub metadata: String,
    pub created_at: String,
}

impl TeamStore {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path).context("opening teams store")?;
        conn.execute_batch(SCHEMA).context("initializing teams schema")?;
        migrate_add_owner_column(&conn).context("migrating organizations.owner_user_id")?;
        migrate_add_mcp_access_mode_column(&conn).context("migrating organizations.mcp_access_mode")?;
        Ok(Self { conn })
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().context("opening in-memory teams store")?;
        conn.execute_batch(SCHEMA).context("initializing teams schema")?;
        migrate_add_owner_column(&conn).context("migrating organizations.owner_user_id")?;
        migrate_add_mcp_access_mode_column(&conn).context("migrating organizations.mcp_access_mode")?;
        Ok(Self { conn })
    }

    /// Registers `tenant` as an organization if it doesn't already have a
    /// row -- idempotent (`ON CONFLICT ... DO NOTHING`), called at signup
    /// so every tenant has an entry here even before anyone bothers naming
    /// it. Calling this again for an already-registered tenant is a no-op,
    /// not an error.
    pub fn ensure_organization(&self, tenant: &str, default_name: &str) -> Result<()> {
        self.conn.execute("INSERT INTO organizations (tenant, name) VALUES (?1, ?2) ON CONFLICT(tenant) DO NOTHING", rusqlite::params![tenant, default_name])?;
        Ok(())
    }

    pub fn organization_name(&self, tenant: &str) -> Result<Option<String>> {
        self.conn.query_row("SELECT name FROM organizations WHERE tenant = ?1", [tenant], |r| r.get(0)).optional().map_err(Into::into)
    }

    /// Upsert, not a plain `UPDATE` -- a caller renaming a tenant that has
    /// no `organizations` row yet (nothing has called `ensure_organization`
    /// for it) would otherwise silently no-op (0 rows affected, no error),
    /// permanently losing the name once something else later calls
    /// `ensure_organization` with its own default. Caught by this crate's
    /// own HTTP-layer test, not assumed.
    pub fn rename_organization(&self, tenant: &str, name: &str) -> Result<()> {
        self.conn.execute("INSERT INTO organizations (tenant, name) VALUES (?1, ?2) ON CONFLICT(tenant) DO UPDATE SET name = excluded.name", rusqlite::params![tenant, name])?;
        Ok(())
    }

    /// `None` until someone has been made Owner -- a tenant registered via
    /// `ensure_organization` (e.g. at signup) has no owner until
    /// `ensure_membership`'s first-member backfill (see `team.rs`) calls
    /// `set_owner` for it.
    pub fn owner_user_id(&self, tenant: &str) -> Result<Option<i64>> {
        self.conn.query_row("SELECT owner_user_id FROM organizations WHERE tenant = ?1", [tenant], |r| r.get(0)).optional().map(|v| v.flatten()).map_err(Into::into)
    }

    /// Upsert, same reasoning as `rename_organization`: a tenant with no
    /// `organizations` row yet must not silently no-op.
    pub fn set_owner(&self, tenant: &str, user_id: i64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO organizations (tenant, owner_user_id) VALUES (?1, ?2) ON CONFLICT(tenant) DO UPDATE SET owner_user_id = excluded.owner_user_id",
            rusqlite::params![tenant, user_id],
        )?;
        Ok(())
    }

    /// Never actually `None` in practice -- the column's own `DEFAULT
    /// 'advisor'` supplies a value even for a tenant whose `organizations`
    /// row was inserted via `ensure_organization`'s explicit `(tenant,
    /// name)` column list (SQLite still applies the default to columns
    /// omitted from an `INSERT`'s column list). `Option` in the return type
    /// only to cover the genuinely-missing-row case (a tenant nothing has
    /// called `ensure_organization` for yet) the same way `owner_user_id`/
    /// `organization_name` do.
    pub fn mcp_access_mode(&self, tenant: &str) -> Result<Option<String>> {
        self.conn.query_row("SELECT mcp_access_mode FROM organizations WHERE tenant = ?1", [tenant], |r| r.get(0)).optional().map_err(Into::into)
    }

    /// Upsert, same reasoning as `rename_organization`/`set_owner`. Caller
    /// (the HTTP layer) is responsible for validating `mode` is `"advisor"`
    /// or `"full"` -- this layer stores whatever string it's given, same
    /// posture `rename_organization` takes on `name`.
    pub fn set_mcp_access_mode(&self, tenant: &str, mode: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO organizations (tenant, mcp_access_mode) VALUES (?1, ?2) ON CONFLICT(tenant) DO UPDATE SET mcp_access_mode = excluded.mcp_access_mode",
            rusqlite::params![tenant, mode],
        )?;
        Ok(())
    }

    pub fn add_member(&self, tenant: &str, user_id: i64, role: &str) -> Result<()> {
        anyhow::ensure!(self.is_valid_role(tenant, role)?, "invalid role {role:?}, must be one of {ROLES:?} or an existing custom role");
        self.conn.execute("INSERT INTO memberships (tenant, user_id, role) VALUES (?1, ?2, ?3)", rusqlite::params![tenant, user_id, role])?;
        Ok(())
    }

    /// Backfills a membership row for `user_id` in `tenant` if they don't
    /// have one yet (making them admin), then backfills Owner for the
    /// tenant if it doesn't have one yet and this user qualifies (active
    /// admin) -- covers both a genuinely brand-new tenant (this call
    /// creates its very first membership) and a pre-existing tenant from
    /// before the Owner column existed. Idempotent: a no-op for a user who
    /// already has a membership row in a tenant that already has an owner.
    ///
    /// **Every caller that gates on `has_capability`/membership must call
    /// this first** -- originally lived only in `agentops-heavy-api/src/team.rs`
    /// (the first consumer), then had to move here once a second consumer
    /// (`/repos/*`'s session path) needed the identical backfill and a live
    /// test caught a brand-new user being universally 403'd from `/repos/*`
    /// because only `/team/*` ever ran this logic.
    pub fn ensure_membership(&self, tenant: &str, user_id: i64) -> Result<()> {
        self.ensure_organization(tenant, "")?;
        if self.get_membership(tenant, user_id)?.is_none() {
            self.add_member(tenant, user_id, "admin")?;
        }
        if self.owner_user_id(tenant)?.is_none() {
            if let Some(m) = self.get_membership(tenant, user_id)? {
                if m.role == "admin" && m.status == "active" {
                    self.set_owner(tenant, user_id)?;
                }
            }
        }
        Ok(())
    }

    /// A role string is valid for `tenant` if it's one of the 4 fixed
    /// `ROLES`, or an existing `custom_roles` row for that tenant --
    /// replaces every bare `ROLES.contains(&role)` validation check
    /// (`add_member`/`update_member`/`create_invite` here, plus one in
    /// `agentops-heavy-api/src/team.rs`) now that custom roles exist.
    pub fn is_valid_role(&self, tenant: &str, role: &str) -> Result<bool> {
        if ROLES.contains(&role) {
            return Ok(true);
        }
        Ok(self.get_custom_role(tenant, role)?.is_some())
    }

    pub fn list_members(&self, tenant: &str) -> Result<Vec<Membership>> {
        let mut stmt = self.conn.prepare("SELECT user_id, tenant, role, status, joined_at FROM memberships WHERE tenant = ?1 ORDER BY joined_at ASC")?;
        let rows = stmt.query_map([tenant], |r| Ok(Membership { user_id: r.get(0)?, tenant: r.get(1)?, role: r.get(2)?, status: r.get(3)?, joined_at: r.get(4)? }))?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    pub fn get_membership(&self, tenant: &str, user_id: i64) -> Result<Option<Membership>> {
        self.conn
            .query_row("SELECT user_id, tenant, role, status, joined_at FROM memberships WHERE tenant = ?1 AND user_id = ?2", rusqlite::params![tenant, user_id], |r| {
                Ok(Membership { user_id: r.get(0)?, tenant: r.get(1)?, role: r.get(2)?, status: r.get(3)?, joined_at: r.get(4)? })
            })
            .optional()
            .map_err(Into::into)
    }

    /// `None` fields left unchanged, same "PATCH" convention as
    /// `agentops_accounts::ProfileUpdate`. Returns whether a row actually
    /// matched (`tenant`+`user_id`), so callers can tell "not a member"
    /// apart from "member, nothing changed."
    pub fn update_member(&self, tenant: &str, user_id: i64, role: Option<&str>, status: Option<&str>) -> Result<bool> {
        if let Some(role) = role {
            anyhow::ensure!(self.is_valid_role(tenant, role)?, "invalid role {role:?}, must be one of {ROLES:?} or an existing custom role");
        }
        let affected = self.conn.execute(
            "UPDATE memberships SET role = COALESCE(?1, role), status = COALESCE(?2, status) WHERE tenant = ?3 AND user_id = ?4",
            rusqlite::params![role, status, tenant, user_id],
        )?;
        Ok(affected > 0)
    }

    pub fn remove_member(&self, tenant: &str, user_id: i64) -> Result<bool> {
        let affected = self.conn.execute("DELETE FROM memberships WHERE tenant = ?1 AND user_id = ?2", rusqlite::params![tenant, user_id])?;
        Ok(affected > 0)
    }

    /// Counts *active* admins -- the HTTP layer uses this to block
    /// removing or demoting the last one, so an organization can never end
    /// up with zero members able to manage it.
    pub fn count_active_admins(&self, tenant: &str) -> Result<i64> {
        self.conn.query_row("SELECT COUNT(*) FROM memberships WHERE tenant = ?1 AND role = 'admin' AND status = 'active'", [tenant], |r| r.get(0)).map_err(Into::into)
    }

    /// Creates a pending invite and returns the raw token exactly once --
    /// same "shown once" contract as a session token/API key, reusing the
    /// same generate/hash primitive (`agentops_security::api_key`). No
    /// email is sent (no email infrastructure exists in this codebase);
    /// the caller (HTTP layer) turns this into a copyable link.
    pub fn create_invite(&self, tenant: &str, email: &str, role: &str, note: Option<&str>, invited_by_user_id: i64) -> Result<(InviteInfo, String)> {
        anyhow::ensure!(self.is_valid_role(tenant, role)?, "invalid role {role:?}, must be one of {ROLES:?} or an existing custom role");
        let (raw, hash) = generate_api_key().context("generating invite token")?;
        self.conn.execute(
            "INSERT INTO invites (tenant, email, role, note, token_hash, invited_by_user_id, expires_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, datetime('now', ?7))",
            rusqlite::params![tenant, email, role, note, hash, invited_by_user_id, format!("+{INVITE_LIFETIME_DAYS} days")],
        )?;
        let id = self.conn.last_insert_rowid();
        let invite = self.get_invite_by_id(id)?.context("invite vanished immediately after insert")?;
        Ok((invite, raw))
    }

    pub fn list_pending_invites(&self, tenant: &str) -> Result<Vec<InviteInfo>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, tenant, email, role, note, invited_by_user_id, status, created_at, expires_at FROM invites WHERE tenant = ?1 AND status = 'pending' ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([tenant], Self::invite_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    fn get_invite_by_id(&self, id: i64) -> Result<Option<InviteInfo>> {
        self.conn
            .query_row("SELECT id, tenant, email, role, note, invited_by_user_id, status, created_at, expires_at FROM invites WHERE id = ?1", [id], Self::invite_from_row)
            .optional()
            .map_err(Into::into)
    }

    fn invite_from_row(r: &rusqlite::Row) -> rusqlite::Result<InviteInfo> {
        Ok(InviteInfo { id: r.get(0)?, tenant: r.get(1)?, email: r.get(2)?, role: r.get(3)?, note: r.get(4)?, invited_by_user_id: r.get(5)?, status: r.get(6)?, created_at: r.get(7)?, expires_at: r.get(8)? })
    }

    /// Rotates the token and expiry of a still-pending invite -- the old
    /// link stops working the moment this is called (its hash no longer
    /// matches anything), so "resend" can't accidentally leave two live
    /// links for the same invite. Scoped to `tenant` + `pending` status,
    /// same guessable-id protection as the membership mutators.
    pub fn resend_invite(&self, tenant: &str, invite_id: i64) -> Result<Option<String>> {
        let (raw, hash) = generate_api_key().context("generating invite token")?;
        let affected = self.conn.execute(
            "UPDATE invites SET token_hash = ?1, expires_at = datetime('now', ?2) WHERE id = ?3 AND tenant = ?4 AND status = 'pending'",
            rusqlite::params![hash, format!("+{INVITE_LIFETIME_DAYS} days"), invite_id, tenant],
        )?;
        Ok((affected > 0).then_some(raw))
    }

    /// Revokes a pending invite -- `status = 'revoked'` rather than a
    /// `DELETE`, so it's still visible in an eventual audit log as "this
    /// invite existed and was revoked," not silently gone.
    pub fn revoke_invite(&self, tenant: &str, invite_id: i64) -> Result<bool> {
        let affected = self.conn.execute("UPDATE invites SET status = 'revoked' WHERE id = ?1 AND tenant = ?2 AND status = 'pending'", rusqlite::params![invite_id, tenant])?;
        Ok(affected > 0)
    }

    /// Public preview for the unauthenticated `GET /invites/{token}`
    /// landing page -- `None` for an unknown, expired, or non-pending
    /// (already accepted/revoked) token, same "don't distinguish the
    /// reason" posture as `AccountStore::login`'s generic error.
    pub fn preview_invite(&self, raw_token: &str) -> Result<Option<InvitePreview>> {
        let hash = hash_api_key(raw_token);
        let row: Option<(String, String, String, String, String)> =
            self.conn.query_row("SELECT tenant, email, role, status, expires_at FROM invites WHERE token_hash = ?1", [&hash], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))).optional()?;
        let Some((tenant, email, role, status, expires_at)) = row else { return Ok(None) };
        if status != "pending" {
            return Ok(None);
        }
        let now: String = self.conn.query_row("SELECT datetime('now')", [], |r| r.get(0))?;
        if expires_at < now {
            return Ok(None);
        }
        Ok(Some(InvitePreview { tenant, email, role }))
    }

    /// Validates the token (same checks as `preview_invite`) and marks the
    /// invite accepted -- returns `(tenant, role)` so the HTTP layer can
    /// create the membership and switch the accepting user's active tenant
    /// (both cross-store operations this crate can't do itself: the
    /// membership lives here, but `users.tenant` lives in
    /// `agentops-accounts`). Marking accepted happens here, before the
    /// caller creates the membership -- `teams.sqlite` and
    /// `accounts.sqlite` are separate files, so nothing spanning both can
    /// be truly atomic regardless of ordering; this ordering means a crash
    /// between the two calls leaves an accepted invite with no membership
    /// (recoverable by re-inviting) rather than a still-pending invite the
    /// same person can never re-accept (token already consumed either way).
    pub fn accept_invite(&self, raw_token: &str) -> Result<(String, String)> {
        let hash = hash_api_key(raw_token);
        let row: Option<(i64, String, String, String, String)> =
            self.conn.query_row("SELECT id, tenant, role, status, expires_at FROM invites WHERE token_hash = ?1", [&hash], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))).optional()?;
        let Some((id, tenant, role, status, expires_at)) = row else {
            anyhow::bail!("invalid or expired invite");
        };
        if status != "pending" {
            anyhow::bail!("invalid or expired invite");
        }
        let now: String = self.conn.query_row("SELECT datetime('now')", [], |r| r.get(0))?;
        if expires_at < now {
            anyhow::bail!("invalid or expired invite");
        }
        self.conn.execute("UPDATE invites SET status = 'accepted' WHERE id = ?1", [id])?;
        Ok((tenant, role))
    }

    /// All per-user repo-access overrides for `tenant` -- the HTTP layer
    /// combines these with role defaults (admin/billing aren't
    /// override-editable at all; member/viewer default to allowed unless
    /// overridden) to build the full grid, since "default access" is a
    /// role-based UI/business-logic concern this crate doesn't need to
    /// know about.
    /// Single-cell lookup, for the `/repos` filtering case (one repo per
    /// row being checked) -- `list_repo_overrides` stays the right choice
    /// for building the whole grid at once (Team Management's Repository
    /// Access tab), but a full-tenant scan per repo per request would be
    /// wasteful here.
    pub fn get_repo_override(&self, tenant: &str, user_id: i64, repo_id: &str) -> Result<Option<bool>> {
        self.conn
            .query_row("SELECT allowed FROM repo_access_overrides WHERE tenant = ?1 AND user_id = ?2 AND repo_id = ?3", rusqlite::params![tenant, user_id, repo_id], |r| r.get::<_, i64>(0))
            .optional()
            .map(|v| v.map(|x| x != 0))
            .map_err(Into::into)
    }

    pub fn list_repo_overrides(&self, tenant: &str) -> Result<Vec<(i64, String, bool)>> {
        let mut stmt = self.conn.prepare("SELECT user_id, repo_id, allowed FROM repo_access_overrides WHERE tenant = ?1")?;
        let rows = stmt.query_map([tenant], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)? != 0)))?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    /// Upserts one override -- `INSERT ... ON CONFLICT DO UPDATE` since the
    /// composite primary key (`tenant`, `user_id`, `repo_id`) means a
    /// second save for the same cell should replace, not duplicate or
    /// error.
    pub fn set_repo_override(&self, tenant: &str, user_id: i64, repo_id: &str, allowed: bool) -> Result<()> {
        self.conn.execute(
            "INSERT INTO repo_access_overrides (tenant, user_id, repo_id, allowed) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(tenant, user_id, repo_id) DO UPDATE SET allowed = excluded.allowed",
            rusqlite::params![tenant, user_id, repo_id, allowed as i64],
        )?;
        Ok(())
    }

    /// Removes an override, reverting that cell back to the role default --
    /// distinct from `set_repo_override(..., false)`, which is an
    /// *explicit* "no access" override, not "no opinion, use the default."
    pub fn clear_repo_override(&self, tenant: &str, user_id: i64, repo_id: &str) -> Result<()> {
        self.conn.execute("DELETE FROM repo_access_overrides WHERE tenant = ?1 AND user_id = ?2 AND repo_id = ?3", rusqlite::params![tenant, user_id, repo_id])?;
        Ok(())
    }

    /// Appends one event -- **insert-only**, deliberately: this type has
    /// no update/delete method, matching standard audit-log practice
    /// (append-only, no mutation path even at the application layer)
    /// without needing full WORM/hash-chain infrastructure, which is
    /// overkill for a self-hosted SQLite app at this stage. `metadata` is
    /// a caller-supplied JSON string, not parsed or validated here -- this
    /// crate doesn't need a `serde_json` dependency just to pass a blob
    /// through unchanged.
    pub fn record_audit_event(&self, tenant: &str, actor_user_id: Option<i64>, action: &str, target: Option<&str>, ip_address: Option<&str>, metadata_json: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO audit_log (tenant, actor_user_id, action, target, ip_address, metadata) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![tenant, actor_user_id, action, target, ip_address, metadata_json],
        )?;
        Ok(())
    }

    /// Most recent first, optionally filtered by exact `actor_user_id`,
    /// exact `action`, and/or a case-insensitive substring match against
    /// `target`. Capped at 200 rows -- this pass has no pagination UI, so
    /// a hard cap (rather than unbounded) keeps a long-lived org's audit
    /// tab from ever loading an unbounded response.
    ///
    /// Orders by `id DESC` as a tiebreaker after `created_at DESC` --
    /// `created_at`'s `CURRENT_TIMESTAMP` only has one-second resolution,
    /// so two events recorded within the same second (a real, not just
    /// theoretical, case: e.g. a repo-access bulk save records several
    /// events back to back) would otherwise sort in an unspecified order.
    /// Caught by this crate's own test, not assumed.
    pub fn list_audit_log(&self, tenant: &str, actor_user_id: Option<i64>, action: Option<&str>, search: Option<&str>) -> Result<Vec<AuditEvent>> {
        let mut sql = "SELECT id, tenant, actor_user_id, action, target, ip_address, metadata, created_at FROM audit_log WHERE tenant = ?1".to_string();
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(tenant.to_string())];
        if let Some(actor_user_id) = actor_user_id {
            params.push(Box::new(actor_user_id));
            sql += &format!(" AND actor_user_id = ?{}", params.len());
        }
        if let Some(action) = action {
            params.push(Box::new(action.to_string()));
            sql += &format!(" AND action = ?{}", params.len());
        }
        if let Some(search) = search {
            params.push(Box::new(format!("%{search}%")));
            sql += &format!(" AND target LIKE ?{} COLLATE NOCASE", params.len());
        }
        sql += " ORDER BY created_at DESC, id DESC LIMIT 200";

        let mut stmt = self.conn.prepare(&sql)?;
        let param_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();
        let rows = stmt.query_map(param_refs.as_slice(), |r| {
            Ok(AuditEvent { id: r.get(0)?, tenant: r.get(1)?, actor_user_id: r.get(2)?, action: r.get(3)?, target: r.get(4)?, ip_address: r.get(5)?, metadata: r.get(6)?, created_at: r.get(7)? })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    /// `role_key` is assigned from the row's own `id` (`custom:{id}`), so
    /// creation is a two-step insert-then-update rather than a single
    /// `INSERT` -- the key can't be computed before a row (and its
    /// autoincrement id) exists. Both statements run synchronously against
    /// this store's single connection under the caller's own lock (see
    /// `TeamState` in `agentops-heavy-api`), so there's no window for a
    /// second `create_custom_role` call to observe or collide with the
    /// transient pre-update row.
    pub fn create_custom_role(&self, tenant: &str, label: &str, cloned_from: &str, capabilities: &[&str], created_by_user_id: i64) -> Result<CustomRole> {
        anyhow::ensure!(ROLES.contains(&cloned_from), "cloned_from must be one of the base roles {ROLES:?}, got {cloned_from:?}");
        let capabilities_joined = capabilities.join(",");
        self.conn.execute(
            "INSERT INTO custom_roles (tenant, role_key, label, cloned_from, capabilities, created_by_user_id) VALUES (?1, '', ?2, ?3, ?4, ?5)",
            rusqlite::params![tenant, label, cloned_from, capabilities_joined, created_by_user_id],
        )?;
        let id = self.conn.last_insert_rowid();
        let role_key = format!("custom:{id}");
        self.conn.execute("UPDATE custom_roles SET role_key = ?1 WHERE id = ?2", rusqlite::params![role_key, id])?;
        self.get_custom_role(tenant, &role_key)?.context("custom role vanished immediately after insert")
    }

    pub fn list_custom_roles(&self, tenant: &str) -> Result<Vec<CustomRole>> {
        let mut stmt =
            self.conn.prepare("SELECT id, tenant, role_key, label, cloned_from, capabilities, created_by_user_id, created_at FROM custom_roles WHERE tenant = ?1 ORDER BY created_at ASC")?;
        let rows = stmt.query_map([tenant], Self::custom_role_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    pub fn get_custom_role(&self, tenant: &str, role_key: &str) -> Result<Option<CustomRole>> {
        self.conn
            .query_row(
                "SELECT id, tenant, role_key, label, cloned_from, capabilities, created_by_user_id, created_at FROM custom_roles WHERE tenant = ?1 AND role_key = ?2",
                rusqlite::params![tenant, role_key],
                Self::custom_role_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    fn custom_role_from_row(r: &rusqlite::Row) -> rusqlite::Result<CustomRole> {
        let capabilities_joined: String = r.get(5)?;
        let capabilities = if capabilities_joined.is_empty() { Vec::new() } else { capabilities_joined.split(',').map(String::from).collect() };
        Ok(CustomRole { id: r.get(0)?, tenant: r.get(1)?, role_key: r.get(2)?, label: r.get(3)?, cloned_from: r.get(4)?, capabilities, created_by_user_id: r.get(6)?, created_at: r.get(7)? })
    }

    pub fn update_custom_role_capabilities(&self, tenant: &str, role_key: &str, capabilities: &[&str]) -> Result<bool> {
        let capabilities_joined = capabilities.join(",");
        let affected = self.conn.execute("UPDATE custom_roles SET capabilities = ?1 WHERE tenant = ?2 AND role_key = ?3", rusqlite::params![capabilities_joined, tenant, role_key])?;
        Ok(affected > 0)
    }

    /// Blocked (returns `Err`, not `Ok(false)`) if any active membership
    /// still uses this `role_key` -- same "pre-check before destructive
    /// action" pattern `count_active_admins` already established, so a
    /// role can't be deleted out from under members currently holding it.
    pub fn delete_custom_role(&self, tenant: &str, role_key: &str) -> Result<bool> {
        let in_use: i64 = self.conn.query_row("SELECT COUNT(*) FROM memberships WHERE tenant = ?1 AND role = ?2", rusqlite::params![tenant, role_key], |r| r.get(0))?;
        anyhow::ensure!(in_use == 0, "cannot delete a role that is still assigned to {in_use} member(s)");
        let affected = self.conn.execute("DELETE FROM custom_roles WHERE tenant = ?1 AND role_key = ?2", rusqlite::params![tenant, role_key])?;
        Ok(affected > 0)
    }

    /// Deletes every row this crate owns for `tenant`, in one transaction --
    /// `repo_access_overrides`, `invites`, `custom_roles`, `memberships`,
    /// then the `organizations` row itself. The point-of-no-return step of
    /// the org-deletion cascade (`POST /team/delete-organization`), run
    /// last after the other stores' leaf data is already gone.
    ///
    /// **`audit_log` is deliberately excluded.** Deleting the audit trail
    /// as part of the very action it's supposed to record means the org's
    /// most sensitive event -- its own deletion -- would be the one event
    /// guaranteed to leave no trace, defeating the point of having an
    /// audit log. Those rows are left in place (orphaned from the now-gone
    /// `organizations` row), not wiped.
    ///
    /// Requires `&mut self` (a real rusqlite transaction) -- the one method
    /// on this store that does, since every other method here is a single
    /// statement. The HTTP-layer caller locks the store mutably just for
    /// this one call.
    pub fn delete_organization_data(&mut self, tenant: &str) -> Result<()> {
        let tx = self.conn.transaction().context("starting org-deletion transaction")?;
        tx.execute("DELETE FROM repo_access_overrides WHERE tenant = ?1", [tenant]).context("deleting repo_access_overrides")?;
        tx.execute("DELETE FROM invites WHERE tenant = ?1", [tenant]).context("deleting invites")?;
        tx.execute("DELETE FROM custom_roles WHERE tenant = ?1", [tenant]).context("deleting custom_roles")?;
        tx.execute("DELETE FROM memberships WHERE tenant = ?1", [tenant]).context("deleting memberships")?;
        tx.execute("DELETE FROM organizations WHERE tenant = ?1", [tenant]).context("deleting organizations row")?;
        tx.commit().context("committing org-deletion transaction")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_then_list_members_round_trips() {
        let store = TeamStore::open_in_memory().unwrap();
        store.add_member("tenant-a", 1, "admin").unwrap();
        store.add_member("tenant-a", 2, "member").unwrap();

        let members = store.list_members("tenant-a").unwrap();
        assert_eq!(members.len(), 2);
        assert_eq!(members[0].role, "admin");
        assert_eq!(members[1].role, "member");
    }

    #[test]
    fn list_members_is_scoped_to_the_tenant() {
        let store = TeamStore::open_in_memory().unwrap();
        store.add_member("tenant-a", 1, "admin").unwrap();
        store.add_member("tenant-b", 2, "admin").unwrap();

        assert_eq!(store.list_members("tenant-a").unwrap().len(), 1);
        assert_eq!(store.list_members("tenant-b").unwrap().len(), 1);
    }

    #[test]
    fn add_member_rejects_an_invalid_role() {
        let store = TeamStore::open_in_memory().unwrap();
        let err = store.add_member("tenant-a", 1, "superuser").unwrap_err();
        assert!(err.to_string().contains("invalid role"));
    }

    #[test]
    fn the_same_user_cannot_be_added_to_the_same_tenant_twice() {
        let store = TeamStore::open_in_memory().unwrap();
        store.add_member("tenant-a", 1, "admin").unwrap();
        assert!(store.add_member("tenant-a", 1, "member").is_err());
    }

    #[test]
    fn update_member_changes_only_the_fields_passed() {
        let store = TeamStore::open_in_memory().unwrap();
        store.add_member("tenant-a", 1, "member").unwrap();

        assert!(store.update_member("tenant-a", 1, Some("admin"), None).unwrap());
        let m = store.get_membership("tenant-a", 1).unwrap().unwrap();
        assert_eq!(m.role, "admin");
        assert_eq!(m.status, "active", "status untouched by a role-only update");

        assert!(store.update_member("tenant-a", 1, None, Some("suspended")).unwrap());
        let m = store.get_membership("tenant-a", 1).unwrap().unwrap();
        assert_eq!(m.role, "admin", "role untouched by a status-only update");
        assert_eq!(m.status, "suspended");
    }

    #[test]
    fn update_member_returns_false_for_a_non_member() {
        let store = TeamStore::open_in_memory().unwrap();
        assert!(!store.update_member("tenant-a", 999, Some("admin"), None).unwrap());
    }

    #[test]
    fn remove_member_is_scoped_to_the_tenant() {
        let store = TeamStore::open_in_memory().unwrap();
        store.add_member("tenant-a", 1, "admin").unwrap();

        assert!(!store.remove_member("tenant-b", 1).unwrap(), "wrong tenant must not remove it");
        assert!(store.remove_member("tenant-a", 1).unwrap());
        assert_eq!(store.list_members("tenant-a").unwrap().len(), 0);
    }

    #[test]
    fn count_active_admins_ignores_other_roles_and_suspended_admins() {
        let store = TeamStore::open_in_memory().unwrap();
        store.add_member("tenant-a", 1, "admin").unwrap();
        store.add_member("tenant-a", 2, "admin").unwrap();
        store.add_member("tenant-a", 3, "member").unwrap();
        assert_eq!(store.count_active_admins("tenant-a").unwrap(), 2);

        store.update_member("tenant-a", 2, None, Some("suspended")).unwrap();
        assert_eq!(store.count_active_admins("tenant-a").unwrap(), 1);
    }

    #[test]
    fn ensure_membership_backfills_admin_and_owner_for_a_brand_new_tenant() {
        let store = TeamStore::open_in_memory().unwrap();
        assert!(store.get_membership("tenant-a", 1).unwrap().is_none());

        store.ensure_membership("tenant-a", 1).unwrap();

        let m = store.get_membership("tenant-a", 1).unwrap().unwrap();
        assert_eq!(m.role, "admin");
        assert_eq!(store.owner_user_id("tenant-a").unwrap(), Some(1));
    }

    #[test]
    fn ensure_membership_is_a_no_op_for_an_existing_member_and_never_reassigns_an_existing_owner() {
        let store = TeamStore::open_in_memory().unwrap();
        store.add_member("tenant-a", 1, "admin").unwrap();
        store.add_member("tenant-a", 2, "member").unwrap();
        store.set_owner("tenant-a", 1).unwrap();

        // A second, non-owner member touching this later must not steal ownership.
        store.ensure_membership("tenant-a", 2).unwrap();
        assert_eq!(store.owner_user_id("tenant-a").unwrap(), Some(1));
        assert_eq!(store.get_membership("tenant-a", 2).unwrap().unwrap().role, "member", "must not be re-added or promoted");
    }

    #[test]
    fn ensure_membership_backfills_owner_for_a_pre_existing_tenant_that_predates_the_owner_column() {
        let store = TeamStore::open_in_memory().unwrap();
        // Simulates a tenant/membership created before Owner existed --
        // membership present, but nobody is Owner yet.
        store.add_member("tenant-a", 1, "admin").unwrap();
        assert_eq!(store.owner_user_id("tenant-a").unwrap(), None);

        store.ensure_membership("tenant-a", 1).unwrap();
        assert_eq!(store.owner_user_id("tenant-a").unwrap(), Some(1));
    }

    #[test]
    fn owner_user_id_is_none_until_set_and_set_owner_upserts() {
        let store = TeamStore::open_in_memory().unwrap();
        assert_eq!(store.owner_user_id("tenant-a").unwrap(), None, "no organizations row yet");

        store.ensure_organization("tenant-a", "Ada's Org").unwrap();
        assert_eq!(store.owner_user_id("tenant-a").unwrap(), None, "row exists but no owner set");

        store.set_owner("tenant-a", 1).unwrap();
        assert_eq!(store.owner_user_id("tenant-a").unwrap(), Some(1));
        assert_eq!(store.organization_name("tenant-a").unwrap(), Some("Ada's Org".to_string()), "set_owner must not clobber the name");

        store.set_owner("tenant-a", 2).unwrap();
        assert_eq!(store.owner_user_id("tenant-a").unwrap(), Some(2), "transfer overwrites");
    }

    #[test]
    fn set_owner_upserts_even_with_no_prior_organizations_row() {
        let store = TeamStore::open_in_memory().unwrap();
        store.set_owner("tenant-a", 1).unwrap();
        assert_eq!(store.owner_user_id("tenant-a").unwrap(), Some(1));
    }

    #[test]
    fn mcp_access_mode_defaults_to_advisor_once_a_row_exists_and_set_upserts() {
        let store = TeamStore::open_in_memory().unwrap();
        assert_eq!(store.mcp_access_mode("tenant-a").unwrap(), None, "no organizations row yet");

        store.ensure_organization("tenant-a", "Ada's Org").unwrap();
        assert_eq!(store.mcp_access_mode("tenant-a").unwrap(), Some("advisor".to_string()), "the column's own DEFAULT applies even though ensure_organization's INSERT doesn't list it");

        store.set_mcp_access_mode("tenant-a", "full").unwrap();
        assert_eq!(store.mcp_access_mode("tenant-a").unwrap(), Some("full".to_string()));
        assert_eq!(store.organization_name("tenant-a").unwrap(), Some("Ada's Org".to_string()), "must not clobber the name");
    }

    #[test]
    fn set_mcp_access_mode_upserts_even_with_no_prior_organizations_row() {
        let store = TeamStore::open_in_memory().unwrap();
        store.set_mcp_access_mode("tenant-a", "full").unwrap();
        assert_eq!(store.mcp_access_mode("tenant-a").unwrap(), Some("full".to_string()));
    }

    #[test]
    fn ensure_organization_is_idempotent_and_rename_overwrites_the_name() {
        let store = TeamStore::open_in_memory().unwrap();
        store.ensure_organization("tenant-a", "Ada's Org").unwrap();
        store.ensure_organization("tenant-a", "A Different Default").unwrap();
        assert_eq!(store.organization_name("tenant-a").unwrap(), Some("Ada's Org".to_string()), "ensure_organization must not overwrite an existing row");

        store.rename_organization("tenant-a", "Acme Corp").unwrap();
        assert_eq!(store.organization_name("tenant-a").unwrap(), Some("Acme Corp".to_string()));
    }

    #[test]
    fn create_invite_returns_the_raw_token_once_and_it_previews_correctly() {
        let store = TeamStore::open_in_memory().unwrap();
        let (invite, raw) = store.create_invite("tenant-a", "new@example.com", "member", Some("welcome!"), 1).unwrap();
        assert_eq!(invite.status, "pending");
        assert!(raw.starts_with("ao_"));

        let preview = store.preview_invite(&raw).unwrap().unwrap();
        assert_eq!(preview.tenant, "tenant-a");
        assert_eq!(preview.email, "new@example.com");
        assert_eq!(preview.role, "member");
    }

    #[test]
    fn create_invite_rejects_an_invalid_role() {
        let store = TeamStore::open_in_memory().unwrap();
        let err = store.create_invite("tenant-a", "new@example.com", "superuser", None, 1).unwrap_err();
        assert!(err.to_string().contains("invalid role"));
    }

    #[test]
    fn list_pending_invites_excludes_accepted_and_revoked_ones() {
        let store = TeamStore::open_in_memory().unwrap();
        let (pending, _) = store.create_invite("tenant-a", "a@example.com", "member", None, 1).unwrap();
        let (_, accepted_raw) = store.create_invite("tenant-a", "b@example.com", "member", None, 1).unwrap();
        let (revoked, _) = store.create_invite("tenant-a", "c@example.com", "member", None, 1).unwrap();

        store.accept_invite(&accepted_raw).unwrap();
        store.revoke_invite("tenant-a", revoked.id).unwrap();

        let pending_list = store.list_pending_invites("tenant-a").unwrap();
        assert_eq!(pending_list.len(), 1);
        assert_eq!(pending_list[0].id, pending.id);
    }

    #[test]
    fn accept_invite_is_single_use() {
        let store = TeamStore::open_in_memory().unwrap();
        let (_, raw) = store.create_invite("tenant-a", "new@example.com", "member", None, 1).unwrap();

        let (tenant, role) = store.accept_invite(&raw).unwrap();
        assert_eq!((tenant.as_str(), role.as_str()), ("tenant-a", "member"));

        let err = store.accept_invite(&raw).unwrap_err();
        assert!(err.to_string().contains("invalid or expired invite"));
    }

    #[test]
    fn accept_invite_rejects_an_unknown_token() {
        let store = TeamStore::open_in_memory().unwrap();
        let err = store.accept_invite("ao_not-a-real-token").unwrap_err();
        assert!(err.to_string().contains("invalid or expired invite"));
    }

    #[test]
    fn resend_invite_rotates_the_token_so_the_old_link_stops_working() {
        let store = TeamStore::open_in_memory().unwrap();
        let (invite, old_raw) = store.create_invite("tenant-a", "new@example.com", "member", None, 1).unwrap();

        let new_raw = store.resend_invite("tenant-a", invite.id).unwrap().unwrap();
        assert_ne!(old_raw, new_raw);

        assert!(store.preview_invite(&old_raw).unwrap().is_none(), "the old token must stop working");
        assert!(store.preview_invite(&new_raw).unwrap().is_some());
    }

    #[test]
    fn resend_invite_returns_none_for_a_non_pending_or_unknown_invite() {
        let store = TeamStore::open_in_memory().unwrap();
        assert!(store.resend_invite("tenant-a", 999).unwrap().is_none());

        let (invite, raw) = store.create_invite("tenant-a", "new@example.com", "member", None, 1).unwrap();
        store.accept_invite(&raw).unwrap();
        assert!(store.resend_invite("tenant-a", invite.id).unwrap().is_none(), "an already-accepted invite can't be resent");
    }

    #[test]
    fn revoke_invite_is_scoped_to_the_tenant_and_only_pending_invites() {
        let store = TeamStore::open_in_memory().unwrap();
        let (invite, _) = store.create_invite("tenant-a", "new@example.com", "member", None, 1).unwrap();

        assert!(!store.revoke_invite("tenant-b", invite.id).unwrap(), "wrong tenant must not revoke it");
        assert!(store.revoke_invite("tenant-a", invite.id).unwrap());
        assert!(!store.revoke_invite("tenant-a", invite.id).unwrap(), "revoking an already-revoked invite is a no-op, not an error, but returns false");
    }

    #[test]
    fn preview_invite_returns_none_for_expired_invites() {
        let store = TeamStore::open_in_memory().unwrap();
        let (_, raw) = store.create_invite("tenant-a", "new@example.com", "member", None, 1).unwrap();
        let hash = agentops_security::api_key::hash_api_key(&raw);
        store.conn.execute("UPDATE invites SET expires_at = datetime('now', '-1 minute') WHERE token_hash = ?1", [&hash]).unwrap();

        assert!(store.preview_invite(&raw).unwrap().is_none());
        assert!(store.accept_invite(&raw).is_err());
    }

    #[test]
    fn set_repo_override_upserts_and_list_returns_it() {
        let store = TeamStore::open_in_memory().unwrap();
        store.set_repo_override("tenant-a", 1, "repo-1", false).unwrap();
        assert_eq!(store.list_repo_overrides("tenant-a").unwrap(), vec![(1, "repo-1".to_string(), false)]);

        // Second save for the same cell replaces, not duplicates.
        store.set_repo_override("tenant-a", 1, "repo-1", true).unwrap();
        assert_eq!(store.list_repo_overrides("tenant-a").unwrap(), vec![(1, "repo-1".to_string(), true)]);
    }

    #[test]
    fn get_repo_override_returns_none_when_unset() {
        let store = TeamStore::open_in_memory().unwrap();
        assert_eq!(store.get_repo_override("tenant-a", 1, "repo-1").unwrap(), None);
        store.set_repo_override("tenant-a", 1, "repo-1", false).unwrap();
        assert_eq!(store.get_repo_override("tenant-a", 1, "repo-1").unwrap(), Some(false));
    }

    #[test]
    fn clear_repo_override_removes_it() {
        let store = TeamStore::open_in_memory().unwrap();
        store.set_repo_override("tenant-a", 1, "repo-1", false).unwrap();
        store.clear_repo_override("tenant-a", 1, "repo-1").unwrap();
        assert!(store.list_repo_overrides("tenant-a").unwrap().is_empty());
    }

    #[test]
    fn list_repo_overrides_is_scoped_to_the_tenant() {
        let store = TeamStore::open_in_memory().unwrap();
        store.set_repo_override("tenant-a", 1, "repo-1", false).unwrap();
        store.set_repo_override("tenant-b", 2, "repo-1", false).unwrap();
        assert_eq!(store.list_repo_overrides("tenant-a").unwrap().len(), 1);
        assert_eq!(store.list_repo_overrides("tenant-b").unwrap().len(), 1);
    }

    #[test]
    fn record_then_list_audit_events_is_most_recent_first_and_tenant_scoped() {
        let store = TeamStore::open_in_memory().unwrap();
        store.record_audit_event("tenant-a", Some(1), "member_invited", Some("new@example.com"), Some("127.0.0.1"), r#"{"role":"member"}"#).unwrap();
        store.record_audit_event("tenant-a", Some(1), "role_changed", Some("member@example.com"), None, "{}").unwrap();
        store.record_audit_event("tenant-b", Some(2), "member_invited", Some("other@example.com"), None, "{}").unwrap();

        let events = store.list_audit_log("tenant-a", None, None, None).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].action, "role_changed", "most recent first");
        assert_eq!(events[1].action, "member_invited");
        assert_eq!(events[1].metadata, r#"{"role":"member"}"#);
    }

    #[test]
    fn list_audit_log_filters_by_actor_action_and_target_search() {
        let store = TeamStore::open_in_memory().unwrap();
        store.record_audit_event("tenant-a", Some(1), "member_invited", Some("alice@example.com"), None, "{}").unwrap();
        store.record_audit_event("tenant-a", Some(2), "member_invited", Some("bob@example.com"), None, "{}").unwrap();
        store.record_audit_event("tenant-a", Some(1), "role_changed", Some("alice@example.com"), None, "{}").unwrap();

        assert_eq!(store.list_audit_log("tenant-a", Some(1), None, None).unwrap().len(), 2);
        assert_eq!(store.list_audit_log("tenant-a", None, Some("role_changed"), None).unwrap().len(), 1);
        assert_eq!(store.list_audit_log("tenant-a", None, None, Some("alice")).unwrap().len(), 2);
        assert_eq!(store.list_audit_log("tenant-a", Some(2), Some("role_changed"), None).unwrap().len(), 0);
    }

    #[test]
    fn record_audit_event_allows_a_system_initiated_null_actor() {
        let store = TeamStore::open_in_memory().unwrap();
        store.record_audit_event("tenant-a", None, "system_cleanup", None, None, "{}").unwrap();
        let events = store.list_audit_log("tenant-a", None, None, None).unwrap();
        assert_eq!(events[0].actor_user_id, None);
    }

    #[test]
    fn create_custom_role_assigns_a_custom_prefixed_key_and_round_trips_capabilities() {
        let store = TeamStore::open_in_memory().unwrap();
        let role = store.create_custom_role("tenant-a", "Junior Dev", "member", &[CAP_GOTCHAS_VIEW, CAP_SEARCH_QUERY], 1).unwrap();
        assert!(role.role_key.starts_with("custom:"));
        assert_eq!(role.label, "Junior Dev");
        assert_eq!(role.cloned_from, "member");
        assert_eq!(role.capabilities, vec![CAP_GOTCHAS_VIEW.to_string(), CAP_SEARCH_QUERY.to_string()]);

        let fetched = store.get_custom_role("tenant-a", &role.role_key).unwrap().unwrap();
        assert_eq!(fetched, role);
    }

    #[test]
    fn create_custom_role_rejects_a_cloned_from_that_is_not_a_base_role() {
        let store = TeamStore::open_in_memory().unwrap();
        let err = store.create_custom_role("tenant-a", "Bad", "superuser", &[], 1).unwrap_err();
        assert!(err.to_string().contains("cloned_from must be one of the base roles"));
    }

    #[test]
    fn list_custom_roles_is_scoped_to_the_tenant() {
        let store = TeamStore::open_in_memory().unwrap();
        store.create_custom_role("tenant-a", "A", "member", &[], 1).unwrap();
        store.create_custom_role("tenant-b", "B", "member", &[], 1).unwrap();
        assert_eq!(store.list_custom_roles("tenant-a").unwrap().len(), 1);
        assert_eq!(store.list_custom_roles("tenant-b").unwrap().len(), 1);
    }

    #[test]
    fn update_custom_role_capabilities_replaces_the_list() {
        let store = TeamStore::open_in_memory().unwrap();
        let role = store.create_custom_role("tenant-a", "Junior Dev", "member", &[CAP_GOTCHAS_VIEW], 1).unwrap();
        assert!(store.update_custom_role_capabilities("tenant-a", &role.role_key, &[CAP_REPOS_VIEW, CAP_GRAPH_VIEW]).unwrap());

        let fetched = store.get_custom_role("tenant-a", &role.role_key).unwrap().unwrap();
        assert_eq!(fetched.capabilities, vec![CAP_REPOS_VIEW.to_string(), CAP_GRAPH_VIEW.to_string()]);
    }

    #[test]
    fn is_valid_role_accepts_fixed_roles_and_existing_custom_roles_only() {
        let store = TeamStore::open_in_memory().unwrap();
        let role = store.create_custom_role("tenant-a", "Junior Dev", "member", &[], 1).unwrap();
        assert!(store.is_valid_role("tenant-a", "admin").unwrap());
        assert!(store.is_valid_role("tenant-a", &role.role_key).unwrap());
        assert!(!store.is_valid_role("tenant-a", "custom:not-real").unwrap());
        assert!(!store.is_valid_role("tenant-a", "superuser").unwrap());

        // A custom role from a *different* tenant is not valid here, even
        // with the right-looking key.
        store.create_custom_role("tenant-b", "Other", "member", &[], 1).unwrap();
        let other_role = store.list_custom_roles("tenant-b").unwrap().into_iter().next().unwrap();
        assert!(!store.is_valid_role("tenant-a", &other_role.role_key).unwrap());
    }

    #[test]
    fn add_member_accepts_a_valid_custom_role() {
        let store = TeamStore::open_in_memory().unwrap();
        let role = store.create_custom_role("tenant-a", "Junior Dev", "member", &[], 1).unwrap();
        store.add_member("tenant-a", 5, &role.role_key).unwrap();
        assert_eq!(store.get_membership("tenant-a", 5).unwrap().unwrap().role, role.role_key);
    }

    #[test]
    fn delete_custom_role_is_blocked_while_a_member_still_holds_it() {
        let store = TeamStore::open_in_memory().unwrap();
        let role = store.create_custom_role("tenant-a", "Junior Dev", "member", &[], 1).unwrap();
        store.add_member("tenant-a", 5, &role.role_key).unwrap();

        let err = store.delete_custom_role("tenant-a", &role.role_key).unwrap_err();
        assert!(err.to_string().contains("still assigned"));

        store.remove_member("tenant-a", 5).unwrap();
        assert!(store.delete_custom_role("tenant-a", &role.role_key).unwrap());
        assert!(store.get_custom_role("tenant-a", &role.role_key).unwrap().is_none());
    }

    #[test]
    fn delete_custom_role_returns_false_for_an_unknown_role() {
        let store = TeamStore::open_in_memory().unwrap();
        assert!(!store.delete_custom_role("tenant-a", "custom:999").unwrap());
    }

    #[test]
    fn delete_organization_data_removes_everything_except_audit_log_and_leaves_other_tenants_alone() {
        let mut store = TeamStore::open_in_memory().unwrap();
        store.ensure_organization("tenant-a", "Acme").unwrap();
        store.add_member("tenant-a", 1, "admin").unwrap();
        store.add_member("tenant-a", 2, "member").unwrap();
        store.set_owner("tenant-a", 1).unwrap();
        store.create_invite("tenant-a", "new@example.com", "member", None, 1).unwrap();
        let custom = store.create_custom_role("tenant-a", "Junior Dev", "member", &[], 1).unwrap();
        store.set_repo_override("tenant-a", 2, "repo-1", false).unwrap();
        store.record_audit_event("tenant-a", Some(1), "member_invited", None, None, "{}").unwrap();

        // A second, unrelated tenant that must survive untouched.
        store.ensure_organization("tenant-b", "Globex").unwrap();
        store.add_member("tenant-b", 3, "admin").unwrap();
        store.record_audit_event("tenant-b", Some(3), "member_invited", None, None, "{}").unwrap();

        store.delete_organization_data("tenant-a").unwrap();

        assert_eq!(store.organization_name("tenant-a").unwrap(), None, "organizations row gone");
        assert!(store.list_members("tenant-a").unwrap().is_empty(), "memberships gone");
        assert!(store.list_pending_invites("tenant-a").unwrap().is_empty(), "invites gone");
        assert!(store.list_custom_roles("tenant-a").unwrap().is_empty(), "custom_roles gone");
        assert!(store.list_repo_overrides("tenant-a").unwrap().is_empty(), "repo_access_overrides gone");
        assert_eq!(store.owner_user_id("tenant-a").unwrap(), None);
        assert!(store.get_custom_role("tenant-a", &custom.role_key).unwrap().is_none());

        // The audit trail survives -- specifically the point of excluding it.
        assert_eq!(store.list_audit_log("tenant-a", None, None, None).unwrap().len(), 1, "audit_log must survive the org's own deletion");

        // A different tenant is completely untouched.
        assert_eq!(store.list_members("tenant-b").unwrap().len(), 1);
        assert_eq!(store.organization_name("tenant-b").unwrap(), Some("Globex".to_string()));
        assert_eq!(store.list_audit_log("tenant-b", None, None, None).unwrap().len(), 1);
    }
}

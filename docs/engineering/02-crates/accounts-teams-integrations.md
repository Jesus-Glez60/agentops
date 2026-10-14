# Crates: accounts, teams and integrations

Identity, organizations and roles, the encrypted credentials vault, and Linear sync. Read this before touching authentication, permissions or anything that stores a third-party secret; trust boundaries are explained in [09-auth-teams-security.md](../09-auth-teams-security.md).

Index and field definitions: [02-crate-reference.md](../02-crate-reference.md).

## agentops-accounts

- **Path**: `agentops-core/crates/agentops-accounts`
- **Purpose**: Minimal user accounts and sessions, 2FA, personal API keys and the CLI device-authorization flow. No email verification or password reset; the module doc flags both as follow-up work.
- **Entry points**:
  - `AccountStore`: signup and login, sessions, profile and preferences, personal API keys, 2FA enrollment, `create_device_auth_code`, `resolve_device_auth_code`, `poll_device_auth_code`.
  - Types: `User`, `NewAccount`, `SessionInfo`, `ApiKeyInfo`, `TwoFactorEnrollment`, `ProfileUpdate`, `PreferencesUpdate`, `DeviceAuthPollResult`.
- **Tables**: `users`, `sessions`, `login_challenges`, `two_factor`, `user_api_keys`, `device_auth_codes` (file `accounts.sqlite`, env `AGENTOPS_ACCOUNTS_DB`).
- **Depends on**: `repo-access` (`SecretsProvider`), `security`.
- **Used by**: `heavy-api`.
- **Invariants** (module doc unless noted):
  - Session tokens, login challenge tokens and personal API keys all reuse `agentops_security::api_key`: random, opaque, hashed at rest, looked up by hash.
  - 2FA TOTP secrets are encrypted with the same AES-256-GCM per-tenant scheme as `agentops-integrations`.
  - A password-verified login without its 2FA code gets a short-lived challenge token, not a session (decision "2FA login flow uses separate challenge token, not real session").
  - Device codes live `DEVICE_CODE_LIFETIME_MINUTES = 15` (`src/lib.rs:223`).
  - Signup is `open` or `first-user-only` (`AGENTOPS_SIGNUP_MODE`). An `invite_token` gates open signup but is not consumed by it (decision "Signup's invite_token gates open signup, but must NOT accept/consume the invite itself").
- **Known history**: gotchas 53984 (`CREATE TABLE IF NOT EXISTS` did not add new columns to an existing `accounts.sqlite`), 54081 (session cookie `secure` flag keyed on `NODE_ENV`).
- **Tests**: 39 (`src/lib.rs`).

## agentops-teams

- **Path**: `agentops-core/crates/agentops-teams`
- **Purpose**: Organizations, memberships, fixed and custom roles, invites, the permission matrix, per-repo access overrides, the audit log and the org's MCP access mode.
- **Entry points**:
  - `TeamStore` (organizations, memberships, invites, custom roles, audit log, `mcp_access_mode`, ownership).
  - Roles: `ROLES = ["admin", "member", "viewer"]`, `ROLE_DESCRIPTIONS`; `migrate_drop_billing_role` removed the old `billing` role (ADR 0043). The owner is a column on `organizations`, not a role.
  - `PERMISSIONS_MATRIX` with 16 capabilities (`CAP_*` constants). `has_capability` (`src/capabilities.rs`) is the single enforcement point.
- **Tables**: `organizations`, `memberships`, `custom_roles`, `invites`, `repo_access_overrides`, `audit_log` (file `teams.sqlite`, env `AGENTOPS_TEAMS_DB`).
- **Depends on**: `security`.
- **Used by**: `heavy-api`.
- **Invariants**:
  - An organization is a tenant: `tenant` here equals `agentops_accounts::User::tenant` (module doc; decision "Team model reuses tenant as organization ID, relaxes users.tenant uniqueness").
  - `users` and `memberships` are in different SQLite files, so they are never joined in SQL. The HTTP layer merges them in Rust (module doc).
  - Route gates call `has_capability`, which reads the same matrix the Roles & Permissions tab shows (decision "Permissions matrix is the single source of truth for both display and enforcement").
  - Roles are validated against both the hardcoded list and the `custom_roles` table (decision "Roles are fixed + custom, validated at both hardcoded enum and database table").
  - Membership rows are created lazily on first touch (`ensure_membership`). Every capability consumer must call it first (gotcha 53997; decision "New tenants/orgs get membership row lazily via first-touch backfill, not at signup").
  - The audit log is insert-only and visible to Owner/Admin only, outside the matrix (decisions "Audit log is insert-only by design", "Audit log visibility is hardcoded Owner/Admin-only, not part of toggleable matrix").
  - `organizations.mcp_access_mode` defaults to `'advisor'` (`src/lib.rs:127`). Changing it needs `CAP_MCP_MANAGE_ACCESS_MODE`.
- **Known issues**: none. The unenforced `gotchas.resolve` and `team.billing` capabilities were removed (ADR 0043).
- **Tests**: 50 (`src/lib.rs` 42, `src/capabilities.rs` 8).

## agentops-integrations

- **Path**: `agentops-core/crates/agentops-integrations`
- **Purpose**: Generic per-tenant credentials vault for third-party secrets (Linear, LLM keys, Firecrawl), with org-level and personal credentials.
- **Entry points**: `CredentialStore`, `NewCredential`, `CredentialSummary` (no secret), `DecryptedCredential`, `AuthType`.
- **Tables**: `credentials`, `user_credentials` (file `integrations.sqlite`, env `AGENTOPS_INTEGRATIONS_DB`).
- **Depends on**: `repo-access` (`SecretsProvider::integration_key`).
- **Used by**: `heavy-api`.
- **Invariants** (module doc):
  - Encrypted, not hashed, because AgentOps must send the plaintext to the third party. AES-256-GCM with a per-tenant key derived from the master key.
  - Personal credentials use a different key from org credentials (decision "Personal integration credentials encrypted under a genuinely different key").
  - Not KMS-backed. The module doc calls this fine for one self-hosted deployment, not for real multi-tenant customers.
- **Tests**: 18 (`src/lib.rs`).

## agentops-linear

- **Path**: `agentops-core/crates/agentops-linear`
- **Purpose**: Two-way sync between native tasks and Linear issues, and Linear webhook registration.
- **Entry points**: `pull_issues`, `push_status`, `sync_push`, `post_comment`, `user_email`, `ensure_webhook_registered`, `LinearConfig`, `WebhookRegistration`.
- **Depends on**: `graph`.
- **Used by**: `cli` (`task sync-linear`), `heavy-api` (webhook receiver, module routes).
- **Invariants**:
  - `external_id` is Linear's human-readable identifier (e.g. `ENG-123`), not its UUID (module doc).
  - `sync-linear --push` must push before pulling; only one task's local edit survives per sync (gotcha 54096).
  - Calls use `ureq`, following `agentops-llm`'s precedent (module doc).
- **Known issues**: none (the module doc was corrected for `findings.md` F14).
- **Decisions**: "Module 7 task manager and Linear sync: what shipped and what's unverified", "Phase 6 core implemented and live-verified: Linear webhook auto-kickoff".
- **Tests**: 12 (`src/lib.rs`).

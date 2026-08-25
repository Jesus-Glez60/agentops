//! Minimal user accounts + sessions (Phase 7, 1.0+ roadmap) — deliberately
//! not full-featured: no email verification or password reset (flagged as
//! follow-up work, not silently dropped). Exists so "when login happens" —
//! the trigger `agentops-integrations`' credentials vault is meant to key
//! off — is a real thing, not a placeholder concept.
//!
//! Session tokens reuse `agentops_security::api_key`'s existing
//! generate/verify primitive directly — a session token *is* exactly a
//! high-entropy random API key in every way that matters (random, opaque,
//! hashed at rest, looked up by hash), so this doesn't need a second
//! mechanism invented just for sessions. Login challenge tokens (the
//! short-lived "password verified, waiting on a 2FA code" handle) reuse
//! the same primitive again for the same reason.
//!
//! 2FA (TOTP) secrets are encrypted at rest with the same AES-256-GCM /
//! per-tenant `SecretsProvider::integration_key` scheme
//! `agentops-integrations` already established for third-party
//! credentials — extended here, not duplicated with a second encryption
//! scheme, since the threat model (recoverable-in-plaintext secret, keyed
//! per tenant) is identical.

use std::path::Path;

use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, Key, KeyInit, Nonce};
use agentops_repo_access::secrets::SecretsProvider;
use agentops_security::api_key::{generate_api_key, hash_api_key};
use anyhow::{Context, Result};
use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use rusqlite::{Connection, OptionalExtension};
use totp_rs::{Algorithm, Secret, TOTP};
use zeroize::Zeroizing;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub id: i64,
    pub email: String,
    pub first_name: String,
    pub last_name: String,
    pub tenant: String,
    pub created_at: String,
    pub avatar_url: Option<String>,
    pub bio: String,
    pub location: String,
    pub handle: Option<String>,
    pub theme_pref: String,
    pub default_search_scope: String,
    pub show_gotcha_callouts: bool,
    pub graph_layout_algorithm: String,
}

/// Fields a user can edit about themselves on the Account tab -- `None`
/// means "leave this field unchanged" (a `PATCH`, not a full replace), so
/// callers only send what actually changed.
#[derive(Debug, Default)]
pub struct ProfileUpdate<'a> {
    pub first_name: Option<&'a str>,
    pub last_name: Option<&'a str>,
    pub handle: Option<&'a str>,
    pub bio: Option<&'a str>,
    pub location: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiKeyInfo {
    pub id: i64,
    pub name: String,
    pub key_prefix: String,
    pub last_used_at: Option<String>,
    pub created_at: String,
}

/// Everything the Security tab's enroll dialog needs to render the QR code
/// (`qr_data_uri`, a ready-to-use `data:image/png;base64,...` string) and
/// the "can't scan? enter this code manually" fallback (`secret_base32`,
/// `otpauth_uri`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TwoFactorEnrollment {
    pub secret_base32: String,
    pub otpauth_uri: String,
    pub qr_data_uri: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInfo {
    pub id: i64,
    pub user_agent: String,
    pub ip_address: String,
    pub created_at: String,
    pub last_seen_at: String,
    pub token_hash: String,
}

/// Same "`None` = unchanged" shape as `ProfileUpdate`, kept separate since
/// preferences and identity fields are edited from different UI cards and
/// have no reason to share a struct.
#[derive(Debug, Default)]
pub struct PreferencesUpdate<'a> {
    pub theme_pref: Option<&'a str>,
    pub default_search_scope: Option<&'a str>,
    pub show_gotcha_callouts: Option<bool>,
    pub graph_layout_algorithm: Option<&'a str>,
}

/// Params for `AccountStore::signup` -- grouped into a struct (matching
/// `agentops_integrations::NewCredential`'s convention) now that it's grown
/// past the two-`&str` shape `login`/`verify_session` still use.
pub struct NewAccount<'a> {
    pub email: &'a str,
    pub password: &'a str,
    pub first_name: &'a str,
    pub last_name: &'a str,
}

pub struct AccountStore {
    conn: Connection,
}

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS users (
        id            INTEGER PRIMARY KEY AUTOINCREMENT,
        email         TEXT NOT NULL UNIQUE,
        password_hash TEXT NOT NULL,
        first_name    TEXT NOT NULL,
        last_name     TEXT NOT NULL,
        tenant        TEXT NOT NULL,
        created_at    TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
        avatar_url              TEXT,
        bio                     TEXT NOT NULL DEFAULT '',
        location                TEXT NOT NULL DEFAULT '',
        handle                  TEXT,
        theme_pref              TEXT NOT NULL DEFAULT 'dark',
        default_search_scope    TEXT NOT NULL DEFAULT 'all',
        show_gotcha_callouts    INTEGER NOT NULL DEFAULT 1,
        graph_layout_algorithm  TEXT NOT NULL DEFAULT 'force'
    );
    CREATE INDEX IF NOT EXISTS idx_users_tenant ON users(tenant);
    CREATE TABLE IF NOT EXISTS sessions (
        id           INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id      INTEGER NOT NULL REFERENCES users(id),
        token_hash   TEXT NOT NULL UNIQUE,
        expires_at   TEXT NOT NULL,
        created_at   TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
        user_agent   TEXT NOT NULL DEFAULT '',
        ip_address   TEXT NOT NULL DEFAULT '',
        last_seen_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
    );
    CREATE TABLE IF NOT EXISTS user_api_keys (
        id           INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id      INTEGER NOT NULL REFERENCES users(id),
        name         TEXT NOT NULL,
        key_hash     TEXT NOT NULL UNIQUE,
        key_prefix   TEXT NOT NULL,
        last_used_at TEXT,
        created_at   TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
        revoked_at   TEXT
    );
    CREATE TABLE IF NOT EXISTS two_factor (
        user_id      INTEGER PRIMARY KEY REFERENCES users(id),
        secret_blob  BLOB NOT NULL,
        enabled      INTEGER NOT NULL DEFAULT 0,
        backup_codes TEXT NOT NULL DEFAULT '',
        created_at   TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
        confirmed_at TEXT
    );
    CREATE TABLE IF NOT EXISTS login_challenges (
        id         INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id    INTEGER NOT NULL REFERENCES users(id),
        token_hash TEXT NOT NULL UNIQUE,
        expires_at TEXT NOT NULL,
        created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
    );
";

/// Login challenges (the short-lived "password verified, waiting on a 2FA
/// code" handle) live for this long -- long enough for a human to read a
/// notification and type a 6-digit code, short enough that a leaked
/// challenge token is useless within minutes. No session capability exists
/// yet at this stage, so this is a much tighter window than
/// `SESSION_LIFETIME_DAYS`.
const LOGIN_CHALLENGE_LIFETIME_MINUTES: i64 = 5;

/// Sessions are valid for this long from creation — no refresh/renewal
/// mechanism yet (a real follow-up, not built here); a session simply stops
/// working after 30 days and the user logs in again.
const SESSION_LIFETIME_DAYS: i64 = 30;

/// Column list shared by every query that constructs a full `User` — kept
/// as one constant so `user_from_row`'s fixed positional `row.get(N)`
/// calls can never drift out of sync with what was actually selected.
const USER_COLUMNS: &str = "id, email, first_name, last_name, tenant, created_at, avatar_url, bio, location, handle, theme_pref, default_search_scope, show_gotcha_callouts, graph_layout_algorithm";
/// Same columns, `u.`-prefixed for the `sessions JOIN users` query.
const USER_COLUMNS_JOINED: &str = "u.id, u.email, u.first_name, u.last_name, u.tenant, u.created_at, u.avatar_url, u.bio, u.location, u.handle, u.theme_pref, u.default_search_scope, u.show_gotcha_callouts, u.graph_layout_algorithm";

fn user_from_row(row: &rusqlite::Row) -> rusqlite::Result<User> {
    Ok(User {
        id: row.get(0)?,
        email: row.get(1)?,
        first_name: row.get(2)?,
        last_name: row.get(3)?,
        tenant: row.get(4)?,
        created_at: row.get(5)?,
        avatar_url: row.get(6)?,
        bio: row.get(7)?,
        location: row.get(8)?,
        handle: row.get(9)?,
        theme_pref: row.get(10)?,
        default_search_scope: row.get(11)?,
        show_gotcha_callouts: row.get::<_, i64>(12)? != 0,
        graph_layout_algorithm: row.get(13)?,
    })
}

impl AccountStore {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path).context("opening accounts store")?;
        conn.execute_batch(SCHEMA).context("initializing accounts schema")?;
        migrate_add_name_columns(&conn).context("migrating accounts schema")?;
        migrate_add_profile_columns(&conn).context("migrating accounts schema")?;
        migrate_relax_tenant_uniqueness(&conn).context("migrating accounts schema")?;
        migrate_add_session_metadata_columns(&conn).context("migrating accounts schema")?;
        Ok(Self { conn })
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().context("opening in-memory accounts store")?;
        conn.execute_batch(SCHEMA).context("initializing accounts schema")?;
        Ok(Self { conn })
    }

    /// Whether any user has ever signed up on this instance — used to
    /// gate open signup after the first account exists (see
    /// `AGENTOPS_SIGNUP_MODE`) and to steer first-run UX to the signup tab.
    pub fn any_account_exists(&self) -> Result<bool> {
        let count: i64 = self.conn.query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))?;
        Ok(count > 0)
    }

    /// Creates a new user + an immediately-valid session — errors if
    /// `email` is already registered.
    pub fn signup(&self, new_account: NewAccount) -> Result<(User, String)> {
        let NewAccount { email, password, first_name, last_name } = new_account;
        let password_hash = hash_password(password)?;
        let tenant = new_random_id();

        self.conn
            .execute(
                "INSERT INTO users (email, password_hash, first_name, last_name, tenant) VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![email, password_hash, first_name, last_name, tenant],
            )
            .context("email already registered, or insert failed")?;
        let user_id = self.conn.last_insert_rowid();

        let user = self.get_user_by_id(user_id)?.context("user vanished immediately after insert")?;
        let raw_token = self.create_session(user_id)?;
        Ok((user, raw_token))
    }

    /// Verifies `email`/`password` and issues a fresh session — deliberately
    /// returns the same generic error for "no such email" and "wrong
    /// password" (never reveal which one via the error message — a
    /// standard login-endpoint precaution against email enumeration).
    ///
    /// Callers that need to check for 2FA *before* issuing a session (the
    /// HTTP `/auth/login` handler) should use `verify_credentials` +
    /// `has_2fa_enabled` + `issue_session`/`create_login_challenge`
    /// instead — this method is kept as the simple all-in-one path for
    /// callers (and every existing test) that don't care about 2FA.
    pub fn login(&self, email: &str, password: &str) -> Result<(User, String)> {
        let user = self.verify_credentials(email, password)?;
        let raw_token = self.issue_session(user.id)?;
        Ok((user, raw_token))
    }

    /// Same email/password check as `login`, without issuing a session --
    /// the split point the 2FA-aware login flow needs (check credentials,
    /// *then* decide whether a 2FA challenge or a real session comes
    /// next), extracted rather than duplicated.
    pub fn verify_credentials(&self, email: &str, password: &str) -> Result<User> {
        let row: Option<(User, String)> = self
            .conn
            .query_row(&format!("SELECT {USER_COLUMNS}, password_hash FROM users WHERE email = ?1"), [email], |r| Ok((user_from_row(r)?, r.get(14)?)))
            .optional()?;
        let Some((user, password_hash)) = row else {
            anyhow::bail!("invalid email or password");
        };
        if !verify_password(password, &password_hash) {
            anyhow::bail!("invalid email or password");
        }
        Ok(user)
    }

    /// Issues a fresh session for an already-verified user -- public
    /// wrapper over `create_session` for callers outside this module that
    /// have already established identity some other way than `login`
    /// (e.g. completing a 2FA challenge).
    pub fn issue_session(&self, user_id: i64) -> Result<String> {
        self.create_session(user_id)
    }

    fn create_session(&self, user_id: i64) -> Result<String> {
        let (raw, hash) = generate_api_key().context("generating session token")?;
        self.conn.execute(
            "INSERT INTO sessions (user_id, token_hash, expires_at) VALUES (?1, ?2, datetime('now', ?3))",
            rusqlite::params![user_id, hash, format!("+{SESSION_LIFETIME_DAYS} days")],
        )?;
        Ok(raw)
    }

    /// Resolves a raw session token (as presented in an `Authorization`
    /// header) to the `User` it belongs to — errors if the token is
    /// unknown *or* expired (same error either way, same
    /// don't-reveal-which-case reasoning as `login`).
    pub fn verify_session(&self, raw_token: &str) -> Result<User> {
        let hash = hash_api_key(raw_token);
        let row: Option<(User, String)> = self
            .conn
            .query_row(
                &format!("SELECT {USER_COLUMNS_JOINED}, s.expires_at FROM sessions s JOIN users u ON u.id = s.user_id WHERE s.token_hash = ?1"),
                [&hash],
                |r| Ok((user_from_row(r)?, r.get(14)?)),
            )
            .optional()?;
        let Some((user, expires_at)) = row else {
            anyhow::bail!("invalid or expired session");
        };

        let now: String = self.conn.query_row("SELECT datetime('now')", [], |r| r.get(0))?;
        if expires_at < now {
            anyhow::bail!("invalid or expired session");
        }

        Ok(user)
    }

    /// Deletes the session matching `raw_token`, if any — used by a real
    /// logout endpoint so the token stops verifying immediately, rather
    /// than just being forgotten client-side while still valid server-side
    /// until its natural 30-day expiry.
    pub fn revoke_session(&self, raw_token: &str) -> Result<()> {
        let hash = hash_api_key(raw_token);
        self.conn.execute("DELETE FROM sessions WHERE token_hash = ?1", [&hash])?;
        Ok(())
    }

    /// Attaches device/network metadata to an already-created session --
    /// kept as a separate call rather than a `create_session` parameter so
    /// `signup`/`login`'s existing signatures (and every test calling them)
    /// stay untouched; the HTTP handler calls this once, right after
    /// signup/login succeeds, with whatever it read off the request.
    pub fn record_session_metadata(&self, raw_token: &str, user_agent: &str, ip_address: &str) -> Result<()> {
        let hash = hash_api_key(raw_token);
        self.conn.execute("UPDATE sessions SET user_agent = ?1, ip_address = ?2 WHERE token_hash = ?3", rusqlite::params![user_agent, ip_address, hash])?;
        Ok(())
    }

    /// Bumps `last_seen_at` to now -- called from `require_session` on every
    /// authenticated request, so "last active" in the Active Sessions UI
    /// reflects actual recent use, not just when the token was issued.
    pub fn touch_session(&self, raw_token: &str) -> Result<()> {
        let hash = hash_api_key(raw_token);
        self.conn.execute("UPDATE sessions SET last_seen_at = datetime('now') WHERE token_hash = ?1", [&hash])?;
        Ok(())
    }

    /// All of `user_id`'s sessions, most-recently-active first.
    /// `token_hash` is included so the caller (which knows the *current*
    /// request's raw token) can determine which row is "this session" by
    /// re-hashing and comparing -- never exposed to the client directly.
    pub fn list_sessions(&self, user_id: i64) -> Result<Vec<SessionInfo>> {
        let mut stmt = self.conn.prepare("SELECT id, user_agent, ip_address, created_at, last_seen_at, token_hash FROM sessions WHERE user_id = ?1 ORDER BY last_seen_at DESC")?;
        let rows = stmt.query_map([user_id], |r| {
            Ok(SessionInfo { id: r.get(0)?, user_agent: r.get(1)?, ip_address: r.get(2)?, created_at: r.get(3)?, last_seen_at: r.get(4)?, token_hash: r.get(5)? })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(anyhow::Error::from)
    }

    /// Deletes one session by id -- scoped to `user_id` (not just `id`) so
    /// a user can never revoke another user's session by guessing/enumerating
    /// ids. Returns whether a row actually matched.
    pub fn revoke_session_by_id(&self, user_id: i64, session_id: i64) -> Result<bool> {
        let affected = self.conn.execute("DELETE FROM sessions WHERE id = ?1 AND user_id = ?2", rusqlite::params![session_id, user_id])?;
        Ok(affected > 0)
    }

    /// Deletes every session belonging to `user_id` except the one matching
    /// `keep_token_hash` (the caller's own current session) -- "log out
    /// everywhere else" from the Active Sessions card. Returns how many were revoked.
    pub fn revoke_all_other_sessions(&self, user_id: i64, keep_token_hash: &str) -> Result<usize> {
        let affected = self.conn.execute("DELETE FROM sessions WHERE user_id = ?1 AND token_hash != ?2", rusqlite::params![user_id, keep_token_hash])?;
        Ok(affected)
    }

    /// Generates a new personal API key for `user_id` -- returns the raw
    /// key exactly once (nothing else in this store ever produces it
    /// again, same "shown once" contract as a session token). `key_prefix`
    /// is the first 10 chars of the raw key (`ao_` + 7 hex chars) so the
    /// UI can render a masked identifier like `ao_a1b2c3d••••••••` without
    /// ever storing or re-deriving the full key.
    pub fn create_user_api_key(&self, user_id: i64, name: &str) -> Result<(ApiKeyInfo, String)> {
        let (raw, hash) = generate_api_key().context("generating API key")?;
        let prefix = raw[..10.min(raw.len())].to_string();
        self.conn.execute("INSERT INTO user_api_keys (user_id, name, key_hash, key_prefix) VALUES (?1, ?2, ?3, ?4)", rusqlite::params![user_id, name, hash, prefix])?;
        let id = self.conn.last_insert_rowid();
        let created_at: String = self.conn.query_row("SELECT created_at FROM user_api_keys WHERE id = ?1", [id], |r| r.get(0))?;
        Ok((ApiKeyInfo { id, name: name.to_string(), key_prefix: prefix, last_used_at: None, created_at }, raw))
    }

    /// Active (non-revoked) keys only, most recent first -- a revoked key
    /// simply disappears from the list rather than showing a
    /// strikethrough/"revoked" row, since there's nothing left to do with
    /// it once it's gone.
    pub fn list_user_api_keys(&self, user_id: i64) -> Result<Vec<ApiKeyInfo>> {
        let mut stmt = self.conn.prepare("SELECT id, name, key_prefix, last_used_at, created_at FROM user_api_keys WHERE user_id = ?1 AND revoked_at IS NULL ORDER BY created_at DESC")?;
        let rows = stmt.query_map([user_id], |r| Ok(ApiKeyInfo { id: r.get(0)?, name: r.get(1)?, key_prefix: r.get(2)?, last_used_at: r.get(3)?, created_at: r.get(4)? }))?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(anyhow::Error::from)
    }

    /// Scoped to `user_id`, same guessable-id protection as `revoke_session_by_id`.
    pub fn revoke_user_api_key(&self, user_id: i64, key_id: i64) -> Result<bool> {
        let affected = self.conn.execute("UPDATE user_api_keys SET revoked_at = datetime('now') WHERE id = ?1 AND user_id = ?2 AND revoked_at IS NULL", rusqlite::params![key_id, user_id])?;
        Ok(affected > 0)
    }

    /// `true` once `confirm_2fa_enrollment` has succeeded -- a row that
    /// exists but has `enabled = 0` (mid-enrollment, abandoned before the
    /// confirm step) must not gate login, so this checks the flag, not
    /// just row existence.
    pub fn has_2fa_enabled(&self, user_id: i64) -> Result<bool> {
        let enabled: Option<i64> = self.conn.query_row("SELECT enabled FROM two_factor WHERE user_id = ?1", [user_id], |r| r.get(0)).optional()?;
        Ok(enabled == Some(1))
    }

    /// Starts (or restarts) enrollment: generates a fresh TOTP secret,
    /// encrypts it at rest, and returns everything the UI needs to render
    /// the QR code + manual-entry fallback. Not yet "enabled" -- a row
    /// exists but `confirm_2fa_enrollment` must succeed with a real code
    /// from an authenticator app before login starts requiring it.
    /// Re-enrolling (calling this again) discards any prior unconfirmed
    /// secret and backup codes via `ON CONFLICT`.
    pub fn begin_2fa_enrollment(&self, secrets: &dyn SecretsProvider, user_id: i64) -> Result<TwoFactorEnrollment> {
        let user = self.get_user_by_id(user_id)?.context("user not found")?;
        let secret = Secret::generate_secret();
        let secret_base32 = secret.to_encoded().to_string();
        let totp = build_totp(&secret_base32, &user.email)?;
        let otpauth_uri = totp.get_url();
        let qr_base64 = totp.get_qr_base64().map_err(|e| anyhow::anyhow!("generating 2FA QR code: {e}"))?;

        let encrypted = encrypt(secrets, &user.tenant, &secret_base32)?;
        self.conn.execute(
            "INSERT INTO two_factor (user_id, secret_blob, enabled, backup_codes, confirmed_at) VALUES (?1, ?2, 0, '', NULL)
             ON CONFLICT(user_id) DO UPDATE SET secret_blob = excluded.secret_blob, enabled = 0, backup_codes = '', confirmed_at = NULL",
            rusqlite::params![user_id, encrypted],
        )?;
        Ok(TwoFactorEnrollment { secret_base32, otpauth_uri, qr_data_uri: format!("data:image/png;base64,{qr_base64}") })
    }

    /// Confirms enrollment with a real code from the authenticator app,
    /// flips `enabled`, and generates a fresh set of backup codes --
    /// returned raw exactly once (same "shown once" contract as an API
    /// key), never retrievable again after this call returns.
    pub fn confirm_2fa_enrollment(&self, secrets: &dyn SecretsProvider, user_id: i64, code: &str) -> Result<Vec<String>> {
        let user = self.get_user_by_id(user_id)?.context("user not found")?;
        let secret_base32 = self.decrypt_2fa_secret(secrets, &user.tenant, user_id)?;
        let totp = build_totp(&secret_base32, &user.email)?;
        if !totp.check_current(code).map_err(|e| anyhow::anyhow!("checking 2FA code: {e}"))? {
            anyhow::bail!("invalid verification code");
        }

        let backup_codes = generate_backup_codes()?;
        let stored = hash_backup_codes(&backup_codes)?;
        self.conn.execute("UPDATE two_factor SET enabled = 1, backup_codes = ?1, confirmed_at = datetime('now') WHERE user_id = ?2", rusqlite::params![stored, user_id])?;
        Ok(backup_codes)
    }

    /// Checks `code` against the current TOTP value, then (if that fails)
    /// against unused backup codes -- a matched backup code is consumed
    /// (marked used) so it can't be replayed. Used both by the Security
    /// tab (re-auth for a sensitive action, not built yet) and by
    /// `complete_login_challenge`.
    pub fn verify_2fa_code(&self, secrets: &dyn SecretsProvider, user_id: i64, code: &str) -> Result<bool> {
        let user = self.get_user_by_id(user_id)?.context("user not found")?;
        let secret_base32 = self.decrypt_2fa_secret(secrets, &user.tenant, user_id)?;
        let totp = build_totp(&secret_base32, &user.email)?;
        if totp.check_current(code).map_err(|e| anyhow::anyhow!("checking 2FA code: {e}"))? {
            return Ok(true);
        }

        let stored: String = self.conn.query_row("SELECT backup_codes FROM two_factor WHERE user_id = ?1", [user_id], |r| r.get(0))?;
        let mut lines: Vec<String> = stored.lines().map(str::to_string).collect();
        for line in lines.iter_mut() {
            if let Some(hash) = line.strip_prefix("UNUSED:") {
                if verify_password(code, hash) {
                    *line = format!("USED:{hash}");
                    self.conn.execute("UPDATE two_factor SET backup_codes = ?1 WHERE user_id = ?2", rusqlite::params![lines.join("\n"), user_id])?;
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    /// Requires the current password (not a 2FA code -- disabling 2FA is
    /// itself a sensitive action, and asking for the thing being removed
    /// as proof-of-identity would be circular) before turning it off.
    pub fn disable_2fa(&self, user_id: i64, password: &str) -> Result<()> {
        let stored_hash: String = self.conn.query_row("SELECT password_hash FROM users WHERE id = ?1", [user_id], |r| r.get(0)).context("user not found")?;
        if !verify_password(password, &stored_hash) {
            anyhow::bail!("password is incorrect");
        }
        self.conn.execute("DELETE FROM two_factor WHERE user_id = ?1", [user_id])?;
        Ok(())
    }

    /// Same password-gate as `disable_2fa`, for the same reason. Fails if
    /// 2FA isn't enabled -- regenerating backup codes for a 2FA setup that
    /// doesn't exist isn't a real state to support silently.
    pub fn regenerate_backup_codes(&self, user_id: i64, password: &str) -> Result<Vec<String>> {
        let stored_hash: String = self.conn.query_row("SELECT password_hash FROM users WHERE id = ?1", [user_id], |r| r.get(0)).context("user not found")?;
        if !verify_password(password, &stored_hash) {
            anyhow::bail!("password is incorrect");
        }
        if !self.has_2fa_enabled(user_id)? {
            anyhow::bail!("two-factor authentication is not enabled");
        }
        let backup_codes = generate_backup_codes()?;
        let stored = hash_backup_codes(&backup_codes)?;
        self.conn.execute("UPDATE two_factor SET backup_codes = ?1 WHERE user_id = ?2", rusqlite::params![stored, user_id])?;
        Ok(backup_codes)
    }

    fn decrypt_2fa_secret(&self, secrets: &dyn SecretsProvider, tenant: &str, user_id: i64) -> Result<String> {
        let blob: Vec<u8> = self.conn.query_row("SELECT secret_blob FROM two_factor WHERE user_id = ?1", [user_id], |r| r.get(0)).context("2FA is not set up for this user")?;
        Ok(decrypt(secrets, tenant, &blob)?.to_string())
    }

    /// Creates a short-lived challenge for "password verified, waiting on
    /// a 2FA code" -- deliberately a *different* token/table than a real
    /// session, so nothing session-scoped works until the 2FA step also
    /// succeeds; a leaked challenge token grants no capability beyond
    /// "try a 2FA code for this one user."
    pub fn create_login_challenge(&self, user_id: i64) -> Result<String> {
        let (raw, hash) = generate_api_key().context("generating login challenge token")?;
        self.conn.execute(
            "INSERT INTO login_challenges (user_id, token_hash, expires_at) VALUES (?1, ?2, datetime('now', ?3))",
            rusqlite::params![user_id, hash, format!("+{LOGIN_CHALLENGE_LIFETIME_MINUTES} minutes")],
        )?;
        Ok(raw)
    }

    /// Verifies the challenge token + 2FA code together and, on success,
    /// issues a real session -- the challenge is single-use (deleted on
    /// success) but survives a wrong code so the user can retry within
    /// the challenge's lifetime rather than having to log in again.
    pub fn complete_login_challenge(&self, secrets: &dyn SecretsProvider, raw_challenge_token: &str, code: &str) -> Result<(User, String)> {
        let hash = hash_api_key(raw_challenge_token);
        let row: Option<(i64, String)> = self.conn.query_row("SELECT user_id, expires_at FROM login_challenges WHERE token_hash = ?1", [&hash], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
        let Some((user_id, expires_at)) = row else {
            anyhow::bail!("invalid or expired login challenge");
        };
        let now: String = self.conn.query_row("SELECT datetime('now')", [], |r| r.get(0))?;
        if expires_at < now {
            anyhow::bail!("invalid or expired login challenge");
        }

        if !self.verify_2fa_code(secrets, user_id, code)? {
            anyhow::bail!("invalid verification code");
        }

        self.conn.execute("DELETE FROM login_challenges WHERE token_hash = ?1", [&hash])?;
        let user = self.get_user_by_id(user_id)?.context("user vanished during login challenge completion")?;
        let session_token = self.issue_session(user_id)?;
        Ok((user, session_token))
    }

    /// Public wrapper over `get_user_by_id` -- for callers outside this
    /// crate that already have a `user_id` from another source (e.g. a
    /// `agentops-teams` membership row) and need the profile fields to go
    /// with it, without themselves being able to do the SQL join (`users`
    /// and `memberships` live in separate SQLite files).
    pub fn get_user(&self, id: i64) -> Result<Option<User>> {
        self.get_user_by_id(id)
    }

    /// Changes which organization `user_id` is "currently acting as" --
    /// the operation behind accepting a team invite (`agentops-teams`
    /// creates the membership row; this is the other half, since
    /// `users.tenant` lives here). Not exposed as a general "switch org"
    /// self-service action yet (a user can only be a member of one tenant
    /// at a time in this pass -- see the tenant-uniqueness-relax
    /// migration's doc comment for the "default/last-active org" framing),
    /// just this one accept-invite call site.
    pub fn switch_tenant(&self, user_id: i64, new_tenant: &str) -> Result<()> {
        self.conn.execute("UPDATE users SET tenant = ?1 WHERE id = ?2", rusqlite::params![new_tenant, user_id])?;
        Ok(())
    }

    fn get_user_by_id(&self, id: i64) -> Result<Option<User>> {
        self.conn.query_row(&format!("SELECT {USER_COLUMNS} FROM users WHERE id = ?1"), [id], user_from_row).optional().map_err(anyhow::Error::from)
    }

    /// Verifies `current_password` before writing `new_password`'s hash --
    /// same generic-error posture as `login` isn't needed here (the caller
    /// is already an authenticated session, not an anonymous attacker
    /// probing for valid emails), so this returns a specific "current
    /// password is incorrect" error rather than a deliberately vague one.
    pub fn change_password(&self, user_id: i64, current_password: &str, new_password: &str) -> Result<()> {
        let stored_hash: String = self.conn.query_row("SELECT password_hash FROM users WHERE id = ?1", [user_id], |r| r.get(0)).context("user not found")?;
        if !verify_password(current_password, &stored_hash) {
            anyhow::bail!("current password is incorrect");
        }
        let new_hash = hash_password(new_password)?;
        self.conn.execute("UPDATE users SET password_hash = ?1 WHERE id = ?2", rusqlite::params![new_hash, user_id])?;
        Ok(())
    }

    /// Applies a `ProfileUpdate` to `user_id` — every `Some(_)` field is
    /// written, every `None` field is left as-is (a single `UPDATE ... SET
    /// col = COALESCE(?, col)` per field rather than N separate statements,
    /// so a partial edit from the Account tab doesn't need to first read
    /// the row back to know what to preserve).
    pub fn update_profile(&self, user_id: i64, update: ProfileUpdate) -> Result<User> {
        self.conn.execute(
            "UPDATE users SET
                first_name = COALESCE(?1, first_name),
                last_name  = COALESCE(?2, last_name),
                handle     = COALESCE(?3, handle),
                bio        = COALESCE(?4, bio),
                location   = COALESCE(?5, location)
             WHERE id = ?6",
            rusqlite::params![update.first_name, update.last_name, update.handle, update.bio, update.location, user_id],
        )?;
        self.get_user_by_id(user_id)?.context("user vanished during profile update")
    }

    /// Same "`Some` = write, `None` = leave alone" shape as `update_profile`
    /// — kept as a separate method (not folded into it) since it's driven
    /// by an entirely different UI card (Preferences vs. Personal
    /// Information) and there's no reason a caller editing one should have
    /// to think about the other's fields.
    pub fn update_preferences(&self, user_id: i64, update: PreferencesUpdate) -> Result<User> {
        self.conn.execute(
            "UPDATE users SET
                theme_pref             = COALESCE(?1, theme_pref),
                default_search_scope   = COALESCE(?2, default_search_scope),
                show_gotcha_callouts   = COALESCE(?3, show_gotcha_callouts),
                graph_layout_algorithm = COALESCE(?4, graph_layout_algorithm)
             WHERE id = ?5",
            rusqlite::params![update.theme_pref, update.default_search_scope, update.show_gotcha_callouts.map(|b| b as i64), update.graph_layout_algorithm, user_id],
        )?;
        self.get_user_by_id(user_id)?.context("user vanished during preferences update")
    }
}

/// `CREATE TABLE IF NOT EXISTS` in `SCHEMA` only creates the table if it's
/// missing entirely — it does nothing for a `users` table that already
/// exists from before `first_name`/`last_name` were added, so every
/// `INSERT` against a pre-existing store would fail with a generic "email
/// already registered, or insert failed" (the real cause buried inside a
/// misleading error) until the columns are actually added. `DEFAULT ''`
/// (rather than leaving them nullable) keeps the column NOT NULL, matching
/// every row inserted through `signup` going forward; existing legacy rows
/// just get an empty name until their owner logs in again — there's no
/// profile-edit flow yet to ask them to fill it in properly.
fn migrate_add_name_columns(conn: &Connection) -> Result<()> {
    let mut existing_columns = std::collections::HashSet::new();
    {
        let mut stmt = conn.prepare("PRAGMA table_info(users)")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            existing_columns.insert(row.get::<_, String>(1)?);
        }
    }

    if !existing_columns.contains("first_name") {
        conn.execute("ALTER TABLE users ADD COLUMN first_name TEXT NOT NULL DEFAULT ''", [])?;
    }
    if !existing_columns.contains("last_name") {
        conn.execute("ALTER TABLE users ADD COLUMN last_name TEXT NOT NULL DEFAULT ''", [])?;
    }
    Ok(())
}

/// Same idiom as `migrate_add_name_columns`, for the Account-tab/Preferences
/// columns added alongside Profile + Team Management. `avatar_url`/`handle`
/// stay nullable (no sane empty-string default for "no avatar"/"no handle
/// chosen yet"); everything else gets a concrete default so existing rows
/// behave exactly like a freshly-signed-up user's defaults would.
fn migrate_add_profile_columns(conn: &Connection) -> Result<()> {
    let mut existing_columns = std::collections::HashSet::new();
    {
        let mut stmt = conn.prepare("PRAGMA table_info(users)")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            existing_columns.insert(row.get::<_, String>(1)?);
        }
    }

    let additions: &[(&str, &str)] = &[
        ("avatar_url", "ALTER TABLE users ADD COLUMN avatar_url TEXT"),
        ("bio", "ALTER TABLE users ADD COLUMN bio TEXT NOT NULL DEFAULT ''"),
        ("location", "ALTER TABLE users ADD COLUMN location TEXT NOT NULL DEFAULT ''"),
        ("handle", "ALTER TABLE users ADD COLUMN handle TEXT"),
        ("theme_pref", "ALTER TABLE users ADD COLUMN theme_pref TEXT NOT NULL DEFAULT 'dark'"),
        ("default_search_scope", "ALTER TABLE users ADD COLUMN default_search_scope TEXT NOT NULL DEFAULT 'all'"),
        ("show_gotcha_callouts", "ALTER TABLE users ADD COLUMN show_gotcha_callouts INTEGER NOT NULL DEFAULT 1"),
        ("graph_layout_algorithm", "ALTER TABLE users ADD COLUMN graph_layout_algorithm TEXT NOT NULL DEFAULT 'force'"),
    ];
    for (column, ddl) in additions {
        if !existing_columns.contains(*column) {
            conn.execute(ddl, [])?;
        }
    }
    Ok(())
}

/// Drops the `UNIQUE` constraint on `users.tenant` -- the pivot that turns
/// "tenant" from "this user's own private isolation id" into "the
/// organization id, shared by every member" (Team Management). SQLite has
/// no `ALTER TABLE ... DROP CONSTRAINT`, so a column-level `UNIQUE` can
/// only be removed by rebuilding the table: create a new one without it,
/// copy every row across (ids preserved, so every other table's `user_id`
/// foreign key stays valid -- `sessions`/`user_api_keys`/`two_factor`/
/// `login_challenges` are untouched), drop the old table, rename. Detected
/// via `PRAGMA index_list` rather than a version flag, so this is a no-op
/// (and safe to call every `open()`) once a store has already been
/// rebuilt: the freshly-created table has no such index to find.
fn migrate_relax_tenant_uniqueness(conn: &Connection) -> Result<()> {
    let mut has_tenant_unique_index = false;
    {
        let mut list_stmt = conn.prepare("PRAGMA index_list(users)")?;
        let mut list_rows = list_stmt.query([])?;
        while let Some(row) = list_rows.next()? {
            let index_name: String = row.get(1)?;
            let is_unique: i64 = row.get(2)?;
            if is_unique != 1 {
                continue;
            }
            let mut info_stmt = conn.prepare(&format!("PRAGMA index_info({index_name})"))?;
            let columns: Vec<String> = info_stmt.query_map([], |r| r.get::<_, String>(2))?.collect::<rusqlite::Result<Vec<_>>>()?;
            if columns == ["tenant"] {
                has_tenant_unique_index = true;
                break;
            }
        }
    }
    if !has_tenant_unique_index {
        return Ok(());
    }

    // Per SQLite's documented 12-step "ALTER TABLE" procedure: foreign key
    // enforcement must be off *outside any transaction* for the
    // rebuild, since `sessions`/`user_api_keys`/`two_factor`/
    // `login_challenges` all reference `users(id)` and `DROP TABLE users`
    // would otherwise fail with "FOREIGN KEY constraint failed" -- caught
    // by this migration's own test, not assumed. Restored to its prior
    // value afterward (never force it on for a store that had it off),
    // and `PRAGMA foreign_key_check` verifies the rebuild didn't actually
    // orphan anything before declaring success.
    let fk_was_on: i64 = conn.query_row("PRAGMA foreign_keys", [], |r| r.get(0))?;
    conn.execute("PRAGMA foreign_keys = OFF", [])?;

    let result: Result<()> = (|| {
        conn.execute_batch(&format!(
            "CREATE TABLE users_new (
                id            INTEGER PRIMARY KEY AUTOINCREMENT,
                email         TEXT NOT NULL UNIQUE,
                password_hash TEXT NOT NULL,
                first_name    TEXT NOT NULL,
                last_name     TEXT NOT NULL,
                tenant        TEXT NOT NULL,
                created_at    TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                avatar_url              TEXT,
                bio                     TEXT NOT NULL DEFAULT '',
                location                TEXT NOT NULL DEFAULT '',
                handle                  TEXT,
                theme_pref              TEXT NOT NULL DEFAULT 'dark',
                default_search_scope    TEXT NOT NULL DEFAULT 'all',
                show_gotcha_callouts    INTEGER NOT NULL DEFAULT 1,
                graph_layout_algorithm  TEXT NOT NULL DEFAULT 'force'
            );
            INSERT INTO users_new ({USER_COLUMNS}, password_hash)
                SELECT {USER_COLUMNS}, password_hash FROM users;
            DROP TABLE users;
            ALTER TABLE users_new RENAME TO users;
            CREATE INDEX IF NOT EXISTS idx_users_tenant ON users(tenant);"
        ))?;

        let mut orphans = conn.prepare("PRAGMA foreign_key_check")?;
        let has_orphans = orphans.query([])?.next()?.is_some();
        anyhow::ensure!(!has_orphans, "tenant-uniqueness rebuild left a dangling foreign key -- refusing to continue");
        Ok(())
    })();

    if fk_was_on != 0 {
        conn.execute("PRAGMA foreign_keys = ON", [])?;
    }
    result
}

/// Same idiom again, for the Active-Sessions-card columns on `sessions`.
/// `last_seen_at` can't use `DEFAULT CURRENT_TIMESTAMP` in the `ALTER TABLE
/// ADD COLUMN` itself -- SQLite rejects non-constant defaults there (works
/// fine in `CREATE TABLE`, which is why `SCHEMA` above can use it for a
/// brand-new table). So it's added with a constant `''` default, then
/// backfilled from `created_at` for pre-existing rows -- close enough,
/// there's no way to know a legacy session's real last-use time since it
/// was never tracked.
fn migrate_add_session_metadata_columns(conn: &Connection) -> Result<()> {
    let mut existing_columns = std::collections::HashSet::new();
    {
        let mut stmt = conn.prepare("PRAGMA table_info(sessions)")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            existing_columns.insert(row.get::<_, String>(1)?);
        }
    }

    let additions: &[(&str, &str)] = &[
        ("user_agent", "ALTER TABLE sessions ADD COLUMN user_agent TEXT NOT NULL DEFAULT ''"),
        ("ip_address", "ALTER TABLE sessions ADD COLUMN ip_address TEXT NOT NULL DEFAULT ''"),
        ("last_seen_at", "ALTER TABLE sessions ADD COLUMN last_seen_at TEXT NOT NULL DEFAULT ''"),
    ];
    let last_seen_at_was_missing = !existing_columns.contains("last_seen_at");
    for (column, ddl) in additions {
        if !existing_columns.contains(*column) {
            conn.execute(ddl, [])?;
        }
    }
    if last_seen_at_was_missing {
        conn.execute("UPDATE sessions SET last_seen_at = created_at WHERE last_seen_at = ''", [])?;
    }
    Ok(())
}

fn hash_password(password: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default().hash_password(password.as_bytes(), &salt).map(|h| h.to_string()).map_err(|e| anyhow::anyhow!("hashing password: {e}"))
}

fn verify_password(password: &str, stored_hash: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(stored_hash) else { return false };
    Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok()
}

/// A random opaque id — used as a fresh user's `tenant`. Not a secret (it's
/// just an identifier, like a UUID), so a plain random hex string is fine;
/// no HMAC/derivation needed the way `SecretsProvider`'s outputs are.
fn new_random_id() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("system randomness must be available");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Standard RFC 6238 params (SHA1, 6 digits, 30s step) matching every
/// authenticator app (Google/Microsoft Authenticator, 1Password, etc.) --
/// `skew: 1` tolerates one 30s window of clock drift each direction, the
/// conventional default. Shared by every 2FA method so the exact same TOTP
/// instance shape is used at enroll, confirm, and verify time.
fn build_totp(secret_base32: &str, account_email: &str) -> Result<TOTP> {
    let secret = Secret::Encoded(secret_base32.to_string());
    let secret_bytes = secret.to_bytes().map_err(|e| anyhow::anyhow!("decoding 2FA secret: {e:?}"))?;
    TOTP::new(Algorithm::SHA1, 6, 1, 30, secret_bytes, Some("AgentOps".to_string()), account_email.to_string()).map_err(|e| anyhow::anyhow!("building TOTP: {e}"))
}

/// 10 backup codes, 8 uppercase-alphanumeric chars each (base32-ish
/// alphabet minus visually-ambiguous `0/O/1/I`) -- typed by hand if an
/// authenticator app is lost, so readable/unambiguous matters more than
/// entropy density here; 8 chars from a 32-symbol alphabet is still ~40
/// bits, plenty for a one-time recovery code.
fn generate_backup_codes() -> Result<Vec<String>> {
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    (0..10)
        .map(|_| {
            let mut bytes = [0u8; 8];
            getrandom::fill(&mut bytes).context("generating backup code")?;
            Ok(bytes.iter().map(|b| ALPHABET[*b as usize % ALPHABET.len()] as char).collect())
        })
        .collect()
}

/// Hashes each backup code (Argon2, same as passwords -- never stored
/// plaintext) and joins them `\n`-separated with an `UNUSED:`/`USED:`
/// prefix per line, so `verify_2fa_code` can mark one consumed with a
/// single `UPDATE` of the whole column rather than a second table.
fn hash_backup_codes(codes: &[String]) -> Result<String> {
    let hashed: Result<Vec<String>> = codes.iter().map(|c| Ok(format!("UNUSED:{}", hash_password(c)?))).collect();
    Ok(hashed?.join("\n"))
}

/// `nonce (12 bytes) || ciphertext` in one BLOB -- same scheme and
/// rationale as `agentops-integrations`' `encrypt`/`decrypt` (see that
/// crate's doc comment): a fresh random nonce every call, since AES-GCM
/// nonce reuse under the same key is a real key-recovery vulnerability.
fn encrypt(secrets: &dyn SecretsProvider, tenant: &str, plaintext: &str) -> Result<Vec<u8>> {
    let key_bytes = secrets.integration_key(tenant)?;
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key_bytes));

    let mut nonce_bytes = [0u8; 12];
    getrandom::fill(&mut nonce_bytes).context("generating AES-GCM nonce")?;
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher.encrypt(nonce, plaintext.as_bytes()).map_err(|e| anyhow::anyhow!("encrypting 2FA secret: {e}"))?;
    let mut out = Vec::with_capacity(12 + ciphertext.len());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

fn decrypt(secrets: &dyn SecretsProvider, tenant: &str, blob: &[u8]) -> Result<Zeroizing<String>> {
    anyhow::ensure!(blob.len() > 12, "encrypted 2FA secret blob is too short to contain a nonce");
    let (nonce_bytes, ciphertext) = blob.split_at(12);

    let key_bytes = secrets.integration_key(tenant)?;
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key_bytes));
    let nonce = Nonce::from_slice(nonce_bytes);

    let plaintext = cipher.decrypt(nonce, ciphertext).map_err(|e| anyhow::anyhow!("decrypting 2FA secret (wrong tenant key, or tampered/corrupted data): {e}"))?;
    Ok(Zeroizing::new(String::from_utf8(plaintext).context("decrypted 2FA secret was not valid UTF-8")?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Most tests here only care about email/password/session behavior, not
    /// names -- this keeps every call site from repeating placeholder
    /// first/last names.
    fn signup(store: &AccountStore, email: &str, password: &str) -> Result<(User, String)> {
        store.signup(NewAccount { email, password, first_name: "Test", last_name: "User" })
    }

    fn test_secrets() -> agentops_repo_access::secrets::EnvSecretsProvider {
        agentops_repo_access::secrets::EnvSecretsProvider::from_hex(&"ab".repeat(32)).unwrap()
    }

    #[test]
    fn any_account_exists_flips_true_after_the_first_signup() {
        let store = AccountStore::open_in_memory().unwrap();
        assert!(!store.any_account_exists().unwrap());
        signup(&store, "dev@example.com", "correct horse battery staple").unwrap();
        assert!(store.any_account_exists().unwrap());
    }

    #[test]
    fn signup_then_login_with_the_right_password_succeeds() {
        let store = AccountStore::open_in_memory().unwrap();
        let (signed_up, _) = signup(&store, "dev@example.com", "correct horse battery staple").unwrap();

        let (logged_in, _) = store.login("dev@example.com", "correct horse battery staple").unwrap();
        assert_eq!(signed_up.id, logged_in.id);
        assert_eq!(signed_up.tenant, logged_in.tenant);
    }

    #[test]
    fn login_with_the_wrong_password_fails() {
        let store = AccountStore::open_in_memory().unwrap();
        signup(&store, "dev@example.com", "correct horse battery staple").unwrap();

        let err = store.login("dev@example.com", "wrong password").unwrap_err();
        assert!(err.to_string().contains("invalid email or password"));
    }

    #[test]
    fn login_with_an_unknown_email_fails_with_the_same_generic_error_as_a_wrong_password() {
        let store = AccountStore::open_in_memory().unwrap();
        signup(&store, "dev@example.com", "correct horse battery staple").unwrap();

        let err = store.login("nobody@example.com", "whatever").unwrap_err();
        assert!(err.to_string().contains("invalid email or password"), "must not leak whether the email exists: {err}");
    }

    #[test]
    fn signing_up_the_same_email_twice_fails() {
        let store = AccountStore::open_in_memory().unwrap();
        signup(&store, "dev@example.com", "password one").unwrap();
        assert!(signup(&store, "dev@example.com", "password two").is_err());
    }

    #[test]
    fn two_different_users_get_different_random_tenants() {
        let store = AccountStore::open_in_memory().unwrap();
        let (a, _) = signup(&store, "a@example.com", "password one").unwrap();
        let (b, _) = signup(&store, "b@example.com", "password two").unwrap();
        assert_ne!(a.tenant, b.tenant);
    }

    #[test]
    fn a_freshly_issued_session_token_verifies_to_the_right_user() {
        let store = AccountStore::open_in_memory().unwrap();
        let (user, token) = signup(&store, "dev@example.com", "correct horse battery staple").unwrap();

        let verified = store.verify_session(&token).unwrap();
        assert_eq!(verified.id, user.id);
    }

    #[test]
    fn a_bogus_session_token_is_rejected() {
        let store = AccountStore::open_in_memory().unwrap();
        signup(&store, "dev@example.com", "correct horse battery staple").unwrap();

        let err = store.verify_session("ao_not-a-real-token").unwrap_err();
        assert!(err.to_string().contains("invalid or expired session"));
    }

    #[test]
    fn an_expired_session_is_rejected() {
        let store = AccountStore::open_in_memory().unwrap();
        let (user, _) = signup(&store, "dev@example.com", "correct horse battery staple").unwrap();

        let (raw, hash) = generate_api_key().unwrap();
        store.conn.execute("INSERT INTO sessions (user_id, token_hash, expires_at) VALUES (?1, ?2, datetime('now', '-1 minute'))", rusqlite::params![user.id, hash]).unwrap();

        let err = store.verify_session(&raw).unwrap_err();
        assert!(err.to_string().contains("invalid or expired session"));
    }

    #[test]
    fn revoking_a_session_makes_it_stop_verifying() {
        let store = AccountStore::open_in_memory().unwrap();
        let (_, token) = signup(&store, "dev@example.com", "correct horse battery staple").unwrap();
        assert!(store.verify_session(&token).is_ok());

        store.revoke_session(&token).unwrap();

        let err = store.verify_session(&token).unwrap_err();
        assert!(err.to_string().contains("invalid or expired session"));
    }

    #[test]
    fn opening_a_pre_existing_store_from_before_first_last_name_existed_migrates_in_place() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE users (
                id            INTEGER PRIMARY KEY AUTOINCREMENT,
                email         TEXT NOT NULL UNIQUE,
                password_hash TEXT NOT NULL,
                tenant        TEXT NOT NULL UNIQUE,
                created_at    TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );
            CREATE TABLE sessions (
                id         INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id    INTEGER NOT NULL REFERENCES users(id),
                token_hash TEXT NOT NULL UNIQUE,
                expires_at TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );",
        )
        .unwrap();
        conn.execute("INSERT INTO users (email, password_hash, tenant) VALUES ('legacy@example.com', 'irrelevant', 'legacy-tenant')", []).unwrap();

        // Re-running SCHEMA (a no-op on the already-existing tables) then
        // the migration is exactly what `AccountStore::open` does against a
        // real file -- exercised directly here since `open_in_memory`
        // always starts from a fresh, already-current schema.
        conn.execute_batch(SCHEMA).unwrap();
        migrate_add_name_columns(&conn).unwrap();
        migrate_add_profile_columns(&conn).unwrap();
        migrate_add_session_metadata_columns(&conn).unwrap();
        let store = AccountStore { conn };

        // The pre-existing row survives with empty (not null/missing) names.
        let legacy = store.login("legacy@example.com", "irrelevant");
        assert!(legacy.is_err(), "irrelevant isn't a real Argon2id hash, so login correctly fails on verification -- this just proves the row is still readable at all");

        // And new signups against the migrated table work normally.
        let (user, _) = signup(&store, "new@example.com", "correct horse battery staple").unwrap();
        assert_eq!(user.email, "new@example.com");
    }

    #[test]
    fn opening_a_pre_existing_store_from_before_profile_columns_existed_migrates_in_place() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE users (
                id            INTEGER PRIMARY KEY AUTOINCREMENT,
                email         TEXT NOT NULL UNIQUE,
                password_hash TEXT NOT NULL,
                first_name    TEXT NOT NULL,
                last_name     TEXT NOT NULL,
                tenant        TEXT NOT NULL UNIQUE,
                created_at    TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );
            CREATE TABLE sessions (
                id         INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id    INTEGER NOT NULL REFERENCES users(id),
                token_hash TEXT NOT NULL UNIQUE,
                expires_at TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );",
        )
        .unwrap();
        conn.execute("INSERT INTO users (email, password_hash, first_name, last_name, tenant) VALUES ('legacy@example.com', 'irrelevant', 'Ada', 'Lovelace', 'legacy-tenant')", []).unwrap();
        // A pre-existing *non-empty* sessions table is the whole point of
        // this test: SQLite silently allows `ALTER TABLE ADD COLUMN ...
        // DEFAULT CURRENT_TIMESTAMP` against an *empty* table but rejects
        // it ("Cannot add a column with non-constant default") once the
        // table has rows to backfill -- a real bug this test caught
        // against the live accounts.sqlite (which has real session rows)
        // that an empty-table fixture would never have exercised.
        conn.execute("INSERT INTO users (email, password_hash, first_name, last_name, tenant) VALUES ('legacy2@example.com', 'irrelevant', 'Grace', 'Hopper', 'legacy-tenant-2')", []).unwrap();
        conn.execute("INSERT INTO sessions (user_id, token_hash, expires_at, created_at) VALUES (2, 'legacy-hash', datetime('now', '+30 days'), '2024-01-01 00:00:00')", []).unwrap();

        conn.execute_batch(SCHEMA).unwrap();
        migrate_add_name_columns(&conn).unwrap();
        migrate_add_profile_columns(&conn).unwrap();
        migrate_add_session_metadata_columns(&conn).unwrap();
        let store = AccountStore { conn };

        let legacy_id: i64 = store.conn.query_row("SELECT id FROM users WHERE email = 'legacy@example.com'", [], |r| r.get(0)).unwrap();
        let legacy = store.get_user_by_id(legacy_id).unwrap().unwrap();
        assert_eq!(legacy.bio, "", "profile columns get sane defaults, not missing/null, for pre-existing rows");
        assert_eq!(legacy.theme_pref, "dark");
        assert!(legacy.show_gotcha_callouts);
        assert_eq!(legacy.avatar_url, None, "avatar_url has no sane default, so it stays nullable");

        let legacy_session_last_seen: String = store.conn.query_row("SELECT last_seen_at FROM sessions WHERE token_hash = 'legacy-hash'", [], |r| r.get(0)).unwrap();
        assert_eq!(legacy_session_last_seen, "2024-01-01 00:00:00", "pre-existing sessions backfill last_seen_at from created_at, not an empty string");

        // And new signups against the migrated table work normally.
        let (user, _) = signup(&store, "new@example.com", "correct horse battery staple").unwrap();
        assert_eq!(user.theme_pref, "dark");
    }

    #[test]
    fn migrating_a_pre_existing_store_with_the_legacy_unique_tenant_constraint_lets_users_share_a_tenant() {
        let conn = Connection::open_in_memory().unwrap();
        // The exact legacy shape: every column this crate has ever had,
        // with `tenant` still `UNIQUE` (the constraint this migration
        // exists to remove).
        conn.execute_batch(
            "CREATE TABLE users (
                id            INTEGER PRIMARY KEY AUTOINCREMENT,
                email         TEXT NOT NULL UNIQUE,
                password_hash TEXT NOT NULL,
                first_name    TEXT NOT NULL,
                last_name     TEXT NOT NULL,
                tenant        TEXT NOT NULL UNIQUE,
                created_at    TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                avatar_url              TEXT,
                bio                     TEXT NOT NULL DEFAULT '',
                location                TEXT NOT NULL DEFAULT '',
                handle                  TEXT,
                theme_pref              TEXT NOT NULL DEFAULT 'dark',
                default_search_scope    TEXT NOT NULL DEFAULT 'all',
                show_gotcha_callouts    INTEGER NOT NULL DEFAULT 1,
                graph_layout_algorithm  TEXT NOT NULL DEFAULT 'force'
            );
            CREATE TABLE sessions (
                id           INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id      INTEGER NOT NULL REFERENCES users(id),
                token_hash   TEXT NOT NULL UNIQUE,
                expires_at   TEXT NOT NULL,
                created_at   TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                user_agent   TEXT NOT NULL DEFAULT '',
                ip_address   TEXT NOT NULL DEFAULT '',
                last_seen_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );",
        )
        .unwrap();
        conn.execute("INSERT INTO users (email, password_hash, first_name, last_name, tenant, bio) VALUES ('legacy@example.com', 'irrelevant', 'Ada', 'Lovelace', 'shared-tenant', 'pre-existing bio')", []).unwrap();
        let legacy_id: i64 = conn.last_insert_rowid();
        conn.execute("INSERT INTO sessions (user_id, token_hash, expires_at) VALUES (?1, 'legacy-hash', datetime('now', '+30 days'))", [legacy_id]).unwrap();

        // Confirm the fixture really has the constraint before migrating,
        // so a false-positive pass (constraint never existed) isn't
        // possible.
        assert!(
            conn.execute("INSERT INTO users (email, password_hash, first_name, last_name, tenant) VALUES ('blocked@example.com', 'irrelevant', 'X', 'Y', 'shared-tenant')", []).is_err(),
            "fixture setup check: the legacy schema must actually enforce UNIQUE on tenant"
        );

        conn.execute_batch(SCHEMA).unwrap();
        migrate_add_name_columns(&conn).unwrap();
        migrate_add_profile_columns(&conn).unwrap();
        migrate_relax_tenant_uniqueness(&conn).unwrap();
        migrate_add_session_metadata_columns(&conn).unwrap();
        let store = AccountStore { conn };

        // The pre-existing row and its session survive the table rebuild intact.
        let legacy = store.get_user_by_id(legacy_id).unwrap().unwrap();
        assert_eq!(legacy.email, "legacy@example.com");
        assert_eq!(legacy.bio, "pre-existing bio", "the rebuild must preserve existing column values, not just structure");
        assert_eq!(legacy.tenant, "shared-tenant");
        let session_user_id: i64 = store.conn.query_row("SELECT user_id FROM sessions WHERE token_hash = 'legacy-hash'", [], |r| r.get(0)).unwrap();
        assert_eq!(session_user_id, legacy_id, "the session's user_id foreign key must still point at the right row after the rebuild");

        // The whole point: a second user can now genuinely share that tenant.
        let second = store.signup(NewAccount { email: "second@example.com", password: "correct horse battery staple", first_name: "Grace", last_name: "Hopper" }).unwrap().0;
        store.conn.execute("UPDATE users SET tenant = ?1 WHERE id = ?2", rusqlite::params!["shared-tenant", second.id]).unwrap();
        let refetched = store.get_user_by_id(second.id).unwrap().unwrap();
        assert_eq!(refetched.tenant, "shared-tenant");
        assert_ne!(refetched.id, legacy_id);

        // And running the migration again against an already-rebuilt store is a safe no-op.
        migrate_relax_tenant_uniqueness(&store.conn).unwrap();
        assert!(store.get_user_by_id(legacy_id).unwrap().is_some());
    }

    #[test]
    fn update_profile_only_changes_the_fields_that_were_some() {
        let store = AccountStore::open_in_memory().unwrap();
        let (user, _) = signup(&store, "dev@example.com", "correct horse battery staple").unwrap();

        let updated = store.update_profile(user.id, ProfileUpdate { bio: Some("Staff engineer"), location: Some("San Francisco, CA"), ..Default::default() }).unwrap();
        assert_eq!(updated.bio, "Staff engineer");
        assert_eq!(updated.location, "San Francisco, CA");
        assert_eq!(updated.first_name, user.first_name, "fields not passed as Some must be left untouched");
    }

    #[test]
    fn update_preferences_only_changes_the_fields_that_were_some() {
        let store = AccountStore::open_in_memory().unwrap();
        let (user, _) = signup(&store, "dev@example.com", "correct horse battery staple").unwrap();
        assert!(user.show_gotcha_callouts, "default is on");

        let updated = store.update_preferences(user.id, PreferencesUpdate { show_gotcha_callouts: Some(false), ..Default::default() }).unwrap();
        assert!(!updated.show_gotcha_callouts);
        assert_eq!(updated.theme_pref, "dark", "fields not passed as Some must be left untouched");
    }

    #[test]
    fn change_password_then_login_works_with_the_new_password_only() {
        let store = AccountStore::open_in_memory().unwrap();
        let (user, _) = signup(&store, "dev@example.com", "correct horse battery staple").unwrap();

        store.change_password(user.id, "correct horse battery staple", "a brand new password").unwrap();

        assert!(store.login("dev@example.com", "correct horse battery staple").is_err(), "the old password must stop working");
        assert!(store.login("dev@example.com", "a brand new password").is_ok());
    }

    #[test]
    fn change_password_rejects_the_wrong_current_password() {
        let store = AccountStore::open_in_memory().unwrap();
        let (user, _) = signup(&store, "dev@example.com", "correct horse battery staple").unwrap();

        let err = store.change_password(user.id, "totally wrong", "a brand new password").unwrap_err();
        assert!(err.to_string().contains("current password is incorrect"));
        assert!(store.login("dev@example.com", "correct horse battery staple").is_ok(), "password must be unchanged after a rejected attempt");
    }

    #[test]
    fn create_user_api_key_returns_the_raw_key_only_once_and_lists_it_by_prefix_only() {
        let store = AccountStore::open_in_memory().unwrap();
        let (user, _) = signup(&store, "dev@example.com", "correct horse battery staple").unwrap();

        let (info, raw) = store.create_user_api_key(user.id, "CI / CD Pipeline").unwrap();
        assert!(raw.starts_with("ao_"));
        assert_eq!(info.key_prefix, &raw[..10]);
        assert_eq!(info.name, "CI / CD Pipeline");

        let keys = store.list_user_api_keys(user.id).unwrap();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].key_prefix, info.key_prefix);
        assert!(!format!("{keys:?}").contains(&raw), "the raw key must never be retrievable again after creation");
    }

    #[test]
    fn revoked_api_keys_disappear_from_the_list_and_are_scoped_to_the_owning_user() {
        let store = AccountStore::open_in_memory().unwrap();
        let (user_a, _) = signup(&store, "a@example.com", "password one").unwrap();
        let (user_b, _) = signup(&store, "b@example.com", "password two").unwrap();
        let (info, _) = store.create_user_api_key(user_a.id, "Local dev").unwrap();

        // b can't revoke a's key by guessing its id.
        assert!(!store.revoke_user_api_key(user_b.id, info.id).unwrap());
        assert_eq!(store.list_user_api_keys(user_a.id).unwrap().len(), 1);

        assert!(store.revoke_user_api_key(user_a.id, info.id).unwrap());
        assert_eq!(store.list_user_api_keys(user_a.id).unwrap().len(), 0);
        // Revoking an already-revoked key again is a no-op, not an error.
        assert!(!store.revoke_user_api_key(user_a.id, info.id).unwrap());
    }

    #[test]
    fn begin_then_confirm_2fa_enrollment_enables_it_and_returns_ten_backup_codes() {
        let store = AccountStore::open_in_memory().unwrap();
        let secrets = test_secrets();
        let (user, _) = signup(&store, "dev@example.com", "correct horse battery staple").unwrap();
        assert!(!store.has_2fa_enabled(user.id).unwrap());

        let enrollment = store.begin_2fa_enrollment(&secrets, user.id).unwrap();
        assert!(enrollment.qr_data_uri.starts_with("data:image/png;base64,"));
        assert!(!store.has_2fa_enabled(user.id).unwrap(), "not enabled until confirmed");

        let totp = build_totp(&enrollment.secret_base32, &user.email).unwrap();
        let code = totp.generate_current().unwrap();
        let backup_codes = store.confirm_2fa_enrollment(&secrets, user.id, &code).unwrap();

        assert!(store.has_2fa_enabled(user.id).unwrap());
        assert_eq!(backup_codes.len(), 10);
    }

    #[test]
    fn confirm_2fa_enrollment_rejects_a_wrong_code() {
        let store = AccountStore::open_in_memory().unwrap();
        let secrets = test_secrets();
        let (user, _) = signup(&store, "dev@example.com", "correct horse battery staple").unwrap();
        store.begin_2fa_enrollment(&secrets, user.id).unwrap();

        let err = store.confirm_2fa_enrollment(&secrets, user.id, "000000").unwrap_err();
        assert!(err.to_string().contains("invalid verification code"));
        assert!(!store.has_2fa_enabled(user.id).unwrap());
    }

    #[test]
    fn verify_2fa_code_accepts_a_valid_totp_code_and_rejects_a_bogus_one() {
        let store = AccountStore::open_in_memory().unwrap();
        let secrets = test_secrets();
        let (user, _) = signup(&store, "dev@example.com", "correct horse battery staple").unwrap();
        let enrollment = store.begin_2fa_enrollment(&secrets, user.id).unwrap();
        let totp = build_totp(&enrollment.secret_base32, &user.email).unwrap();
        store.confirm_2fa_enrollment(&secrets, user.id, &totp.generate_current().unwrap()).unwrap();

        assert!(!store.verify_2fa_code(&secrets, user.id, "000000").unwrap());
        assert!(store.verify_2fa_code(&secrets, user.id, &totp.generate_current().unwrap()).unwrap());
    }

    #[test]
    fn verify_2fa_code_accepts_a_backup_code_exactly_once() {
        let store = AccountStore::open_in_memory().unwrap();
        let secrets = test_secrets();
        let (user, _) = signup(&store, "dev@example.com", "correct horse battery staple").unwrap();
        let enrollment = store.begin_2fa_enrollment(&secrets, user.id).unwrap();
        let totp = build_totp(&enrollment.secret_base32, &user.email).unwrap();
        let backup_codes = store.confirm_2fa_enrollment(&secrets, user.id, &totp.generate_current().unwrap()).unwrap();

        let first_code = &backup_codes[0];
        assert!(store.verify_2fa_code(&secrets, user.id, first_code).unwrap());
        assert!(!store.verify_2fa_code(&secrets, user.id, first_code).unwrap(), "a backup code must not be reusable");
    }

    #[test]
    fn disable_2fa_requires_the_password_and_removes_the_gate() {
        let store = AccountStore::open_in_memory().unwrap();
        let secrets = test_secrets();
        let (user, _) = signup(&store, "dev@example.com", "correct horse battery staple").unwrap();
        let enrollment = store.begin_2fa_enrollment(&secrets, user.id).unwrap();
        let totp = build_totp(&enrollment.secret_base32, &user.email).unwrap();
        store.confirm_2fa_enrollment(&secrets, user.id, &totp.generate_current().unwrap()).unwrap();

        assert!(store.disable_2fa(user.id, "wrong password").is_err());
        assert!(store.has_2fa_enabled(user.id).unwrap());

        store.disable_2fa(user.id, "correct horse battery staple").unwrap();
        assert!(!store.has_2fa_enabled(user.id).unwrap());
    }

    #[test]
    fn login_challenge_round_trips_to_a_real_session_with_the_right_code() {
        let store = AccountStore::open_in_memory().unwrap();
        let secrets = test_secrets();
        let (user, _) = signup(&store, "dev@example.com", "correct horse battery staple").unwrap();
        let enrollment = store.begin_2fa_enrollment(&secrets, user.id).unwrap();
        let totp = build_totp(&enrollment.secret_base32, &user.email).unwrap();
        store.confirm_2fa_enrollment(&secrets, user.id, &totp.generate_current().unwrap()).unwrap();

        let challenge = store.create_login_challenge(user.id).unwrap();
        let err = store.complete_login_challenge(&secrets, &challenge, "000000").unwrap_err();
        assert!(err.to_string().contains("invalid verification code"));

        let (completed_user, session_token) = store.complete_login_challenge(&secrets, &challenge, &totp.generate_current().unwrap()).unwrap();
        assert_eq!(completed_user.id, user.id);
        assert!(store.verify_session(&session_token).is_ok());

        // Single-use: the same challenge can't be completed again.
        assert!(store.complete_login_challenge(&secrets, &challenge, &totp.generate_current().unwrap()).is_err());
    }

    #[test]
    fn switch_tenant_changes_which_org_a_user_is_acting_as() {
        let store = AccountStore::open_in_memory().unwrap();
        let (user, _) = signup(&store, "dev@example.com", "correct horse battery staple").unwrap();

        store.switch_tenant(user.id, "some-other-tenant").unwrap();

        let refetched = store.get_user(user.id).unwrap().unwrap();
        assert_eq!(refetched.tenant, "some-other-tenant");
    }

    #[test]
    fn revoking_an_unknown_token_is_not_an_error() {
        let store = AccountStore::open_in_memory().unwrap();
        assert!(store.revoke_session("ao_not-a-real-token").is_ok());
    }

    #[test]
    fn record_session_metadata_and_touch_session_round_trip() {
        let store = AccountStore::open_in_memory().unwrap();
        let (user, token) = signup(&store, "dev@example.com", "correct horse battery staple").unwrap();
        store.record_session_metadata(&token, "Chrome on macOS", "127.0.0.1").unwrap();
        store.touch_session(&token).unwrap();

        let sessions = store.list_sessions(user.id).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].user_agent, "Chrome on macOS");
        assert_eq!(sessions[0].ip_address, "127.0.0.1");
    }

    #[test]
    fn list_sessions_only_returns_that_users_sessions() {
        let store = AccountStore::open_in_memory().unwrap();
        let (user_a, _) = signup(&store, "a@example.com", "password one").unwrap();
        let (_, _) = signup(&store, "b@example.com", "password two").unwrap();

        let sessions = store.list_sessions(user_a.id).unwrap();
        assert_eq!(sessions.len(), 1);
    }

    #[test]
    fn revoke_session_by_id_is_scoped_to_the_owning_user() {
        let store = AccountStore::open_in_memory().unwrap();
        let (user_a, token_a) = signup(&store, "a@example.com", "password one").unwrap();
        let (user_b, _) = signup(&store, "b@example.com", "password two").unwrap();

        let session_id = store.list_sessions(user_a.id).unwrap()[0].id;

        // b can't revoke a's session by guessing its id.
        assert!(!store.revoke_session_by_id(user_b.id, session_id).unwrap());
        assert!(store.verify_session(&token_a).is_ok());

        // a can revoke their own.
        assert!(store.revoke_session_by_id(user_a.id, session_id).unwrap());
        assert!(store.verify_session(&token_a).is_err());
    }

    #[test]
    fn revoke_all_other_sessions_keeps_only_the_current_one() {
        let store = AccountStore::open_in_memory().unwrap();
        let (user, first_token) = signup(&store, "dev@example.com", "correct horse battery staple").unwrap();
        let (_, second_token) = store.login("dev@example.com", "correct horse battery staple").unwrap();
        let (_, third_token) = store.login("dev@example.com", "correct horse battery staple").unwrap();

        let revoked = store.revoke_all_other_sessions(user.id, &hash_api_key(&third_token)).unwrap();
        assert_eq!(revoked, 2);
        assert!(store.verify_session(&first_token).is_err());
        assert!(store.verify_session(&second_token).is_err());
        assert!(store.verify_session(&third_token).is_ok());
    }

    #[test]
    fn first_and_last_name_round_trip_through_signup_login_and_verify_session() {
        let store = AccountStore::open_in_memory().unwrap();
        let (signed_up, token) = store.signup(NewAccount { email: "dev@example.com", password: "correct horse battery staple", first_name: "Ada", last_name: "Lovelace" }).unwrap();
        assert_eq!((signed_up.first_name.as_str(), signed_up.last_name.as_str()), ("Ada", "Lovelace"));

        let (logged_in, _) = store.login("dev@example.com", "correct horse battery staple").unwrap();
        assert_eq!((logged_in.first_name.as_str(), logged_in.last_name.as_str()), ("Ada", "Lovelace"));

        let verified = store.verify_session(&token).unwrap();
        assert_eq!((verified.first_name.as_str(), verified.last_name.as_str()), ("Ada", "Lovelace"));
    }

    #[test]
    fn login_never_stores_or_returns_the_plaintext_password() {
        let store = AccountStore::open_in_memory().unwrap();
        signup(&store, "dev@example.com", "correct horse battery staple").unwrap();

        let stored: String = store.conn.query_row("SELECT password_hash FROM users WHERE email = 'dev@example.com'", [], |r| r.get(0)).unwrap();
        assert!(!stored.contains("correct horse battery staple"));
        assert!(stored.starts_with("$argon2id$"), "must be a real Argon2id PHC string: {stored}");
    }
}

//! Minimal user accounts + sessions (Phase 7, 1.0+ roadmap) — deliberately
//! not full-featured: no email verification, password reset, or 2FA (all
//! flagged as follow-up work, not silently dropped). Exists so "when login
//! happens" — the trigger `agentops-integrations`' credentials vault is
//! meant to key off — is a real thing, not a placeholder concept.
//!
//! Session tokens reuse `agentops_security::api_key`'s existing
//! generate/verify primitive directly — a session token *is* exactly a
//! high-entropy random API key in every way that matters (random, opaque,
//! hashed at rest, looked up by hash), so this doesn't need a second
//! mechanism invented just for sessions.

use std::path::Path;

use agentops_security::api_key::{generate_api_key, hash_api_key};
use anyhow::{Context, Result};
use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use rusqlite::{Connection, OptionalExtension};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub id: i64,
    pub email: String,
    pub tenant: String,
    pub created_at: String,
}

pub struct AccountStore {
    conn: Connection,
}

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS users (
        id            INTEGER PRIMARY KEY AUTOINCREMENT,
        email         TEXT NOT NULL UNIQUE,
        password_hash TEXT NOT NULL,
        tenant        TEXT NOT NULL UNIQUE,
        created_at    TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
    );
    CREATE TABLE IF NOT EXISTS sessions (
        id         INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id    INTEGER NOT NULL REFERENCES users(id),
        token_hash TEXT NOT NULL UNIQUE,
        expires_at TEXT NOT NULL,
        created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
    );
";

/// Sessions are valid for this long from creation — no refresh/renewal
/// mechanism yet (a real follow-up, not built here); a session simply stops
/// working after 30 days and the user logs in again.
const SESSION_LIFETIME_DAYS: i64 = 30;

impl AccountStore {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path).context("opening accounts store")?;
        conn.execute_batch(SCHEMA).context("initializing accounts schema")?;
        Ok(Self { conn })
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().context("opening in-memory accounts store")?;
        conn.execute_batch(SCHEMA).context("initializing accounts schema")?;
        Ok(Self { conn })
    }

    /// Creates a new user + an immediately-valid session — errors if
    /// `email` is already registered.
    pub fn signup(&self, email: &str, password: &str) -> Result<(User, String)> {
        let password_hash = hash_password(password)?;
        let tenant = new_random_id();

        self.conn
            .execute("INSERT INTO users (email, password_hash, tenant) VALUES (?1, ?2, ?3)", rusqlite::params![email, password_hash, tenant])
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
    pub fn login(&self, email: &str, password: &str) -> Result<(User, String)> {
        let row: Option<(i64, String, String, String, String)> = self
            .conn
            .query_row("SELECT id, email, password_hash, tenant, created_at FROM users WHERE email = ?1", [email], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })
            .optional()?;
        let Some((id, email, password_hash, tenant, created_at)) = row else {
            anyhow::bail!("invalid email or password");
        };
        if !verify_password(password, &password_hash) {
            anyhow::bail!("invalid email or password");
        }

        let raw_token = self.create_session(id)?;
        Ok((User { id, email, tenant, created_at }, raw_token))
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
        let row: Option<(i64, String, String, String, String)> = self
            .conn
            .query_row(
                "SELECT u.id, u.email, u.tenant, u.created_at, s.expires_at FROM sessions s JOIN users u ON u.id = s.user_id WHERE s.token_hash = ?1",
                [&hash],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()?;
        let Some((id, email, tenant, created_at, expires_at)) = row else {
            anyhow::bail!("invalid or expired session");
        };

        let now: String = self.conn.query_row("SELECT datetime('now')", [], |r| r.get(0))?;
        if expires_at < now {
            anyhow::bail!("invalid or expired session");
        }

        Ok(User { id, email, tenant, created_at })
    }

    fn get_user_by_id(&self, id: i64) -> Result<Option<User>> {
        self.conn
            .query_row("SELECT id, email, tenant, created_at FROM users WHERE id = ?1", [id], |r| {
                Ok(User { id: r.get(0)?, email: r.get(1)?, tenant: r.get(2)?, created_at: r.get(3)? })
            })
            .optional()
            .map_err(anyhow::Error::from)
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signup_then_login_with_the_right_password_succeeds() {
        let store = AccountStore::open_in_memory().unwrap();
        let (signed_up, _) = store.signup("dev@example.com", "correct horse battery staple").unwrap();

        let (logged_in, _) = store.login("dev@example.com", "correct horse battery staple").unwrap();
        assert_eq!(signed_up.id, logged_in.id);
        assert_eq!(signed_up.tenant, logged_in.tenant);
    }

    #[test]
    fn login_with_the_wrong_password_fails() {
        let store = AccountStore::open_in_memory().unwrap();
        store.signup("dev@example.com", "correct horse battery staple").unwrap();

        let err = store.login("dev@example.com", "wrong password").unwrap_err();
        assert!(err.to_string().contains("invalid email or password"));
    }

    #[test]
    fn login_with_an_unknown_email_fails_with_the_same_generic_error_as_a_wrong_password() {
        let store = AccountStore::open_in_memory().unwrap();
        store.signup("dev@example.com", "correct horse battery staple").unwrap();

        let err = store.login("nobody@example.com", "whatever").unwrap_err();
        assert!(err.to_string().contains("invalid email or password"), "must not leak whether the email exists: {err}");
    }

    #[test]
    fn signing_up_the_same_email_twice_fails() {
        let store = AccountStore::open_in_memory().unwrap();
        store.signup("dev@example.com", "password one").unwrap();
        assert!(store.signup("dev@example.com", "password two").is_err());
    }

    #[test]
    fn two_different_users_get_different_random_tenants() {
        let store = AccountStore::open_in_memory().unwrap();
        let (a, _) = store.signup("a@example.com", "password one").unwrap();
        let (b, _) = store.signup("b@example.com", "password two").unwrap();
        assert_ne!(a.tenant, b.tenant);
    }

    #[test]
    fn a_freshly_issued_session_token_verifies_to_the_right_user() {
        let store = AccountStore::open_in_memory().unwrap();
        let (user, token) = store.signup("dev@example.com", "correct horse battery staple").unwrap();

        let verified = store.verify_session(&token).unwrap();
        assert_eq!(verified.id, user.id);
    }

    #[test]
    fn a_bogus_session_token_is_rejected() {
        let store = AccountStore::open_in_memory().unwrap();
        store.signup("dev@example.com", "correct horse battery staple").unwrap();

        let err = store.verify_session("ao_not-a-real-token").unwrap_err();
        assert!(err.to_string().contains("invalid or expired session"));
    }

    #[test]
    fn an_expired_session_is_rejected() {
        let store = AccountStore::open_in_memory().unwrap();
        let (user, _) = store.signup("dev@example.com", "correct horse battery staple").unwrap();

        let (raw, hash) = generate_api_key().unwrap();
        store.conn.execute("INSERT INTO sessions (user_id, token_hash, expires_at) VALUES (?1, ?2, datetime('now', '-1 minute'))", rusqlite::params![user.id, hash]).unwrap();

        let err = store.verify_session(&raw).unwrap_err();
        assert!(err.to_string().contains("invalid or expired session"));
    }

    #[test]
    fn login_never_stores_or_returns_the_plaintext_password() {
        let store = AccountStore::open_in_memory().unwrap();
        store.signup("dev@example.com", "correct horse battery staple").unwrap();

        let stored: String = store.conn.query_row("SELECT password_hash FROM users WHERE email = 'dev@example.com'", [], |r| r.get(0)).unwrap();
        assert!(!stored.contains("correct horse battery staple"));
        assert!(stored.starts_with("$argon2id$"), "must be a real Argon2id PHC string: {stored}");
    }
}

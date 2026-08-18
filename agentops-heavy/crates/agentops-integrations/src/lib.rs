//! Generic, extensible per-tenant credentials vault (Phase 7, 1.0+
//! roadmap) — one table any integration (Linear, Anthropic/LLM BYOK,
//! future ones) reads from, instead of each integration inventing its own
//! storage. Encrypted, not hashed: unlike an API key AgentOps itself
//! issues (`agentops_security::api_key`, verified by comparing hashes,
//! never needing the raw value back), a stored *third-party* credential
//! must be recoverable in plaintext — AgentOps has to send it outbound to
//! Linear/Anthropic on the tenant's behalf. Encryption uses AES-256-GCM
//! with a key derived per-tenant via
//! `agentops_repo_access::secrets::SecretsProvider::integration_key` — the
//! same master-key-derivation policy boundary `agentops-repo-access`
//! already established for SSH deploy-key passphrases, extended rather
//! than duplicated.
//!
//! **Not yet KMS-backed** — same caveat `EnvSecretsProvider`'s own doc
//! comment already carries: fine for a single self-hosted deployment, not
//! sufficient before any real paying multi-tenant customer's credentials
//! are stored here (see SECURITY.md).

use std::path::Path;

use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, Key, KeyInit, Nonce};
use agentops_repo_access::secrets::SecretsProvider;
use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension};
use zeroize::Zeroizing;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthType {
    ApiKey,
    OAuth,
}

impl AuthType {
    fn as_str(&self) -> &'static str {
        match self {
            AuthType::ApiKey => "api_key",
            AuthType::OAuth => "oauth",
        }
    }

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "api_key" => Ok(AuthType::ApiKey),
            "oauth" => Ok(AuthType::OAuth),
            other => anyhow::bail!("unknown auth_type {other:?}"),
        }
    }
}

/// What a caller gets back after decryption — the only place the raw
/// secret ever exists outside this crate's own encrypt/decrypt boundary.
#[derive(Debug)]
pub struct DecryptedCredential {
    pub provider: String,
    pub auth_type: AuthType,
    pub secret: Zeroizing<String>,
    pub refresh_token: Option<Zeroizing<String>>,
    pub expires_at: Option<String>,
}

/// Everything `store_credential` needs about one credential, bundled to
/// keep the method's own argument count sane.
pub struct NewCredential<'a> {
    pub provider: &'a str,
    pub auth_type: AuthType,
    pub secret: &'a str,
    pub refresh_token: Option<&'a str>,
    pub expires_at: Option<&'a str>,
}

/// The raw SQLite row shape `get_credential` reads before decrypting —
/// named so the type isn't inlined as an unreadable tuple at the call site.
type CredentialRow = (String, Vec<u8>, Option<Vec<u8>>, Option<String>);

/// What a listing endpoint can safely return — **never** the secret or
/// refresh token, same "never leak the sensitive field" discipline
/// `agentops-heavy-api`'s `ConnectionView` already established for repo
/// connections.
#[derive(Debug, Clone)]
pub struct CredentialSummary {
    pub provider: String,
    pub auth_type: AuthType,
    pub created_at: String,
    pub updated_at: String,
}

pub struct CredentialStore {
    conn: Connection,
}

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS credentials (
        id                     INTEGER PRIMARY KEY AUTOINCREMENT,
        tenant                 TEXT NOT NULL,
        provider               TEXT NOT NULL,
        auth_type              TEXT NOT NULL CHECK (auth_type IN ('api_key', 'oauth')),
        encrypted_secret       BLOB NOT NULL,
        encrypted_refresh_token BLOB,
        expires_at             TEXT,
        created_at             TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
        updated_at             TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
        UNIQUE (tenant, provider)
    );
    CREATE TABLE IF NOT EXISTS user_credentials (
        id                      INTEGER PRIMARY KEY AUTOINCREMENT,
        tenant                  TEXT NOT NULL,
        user_id                 INTEGER NOT NULL,
        provider                TEXT NOT NULL,
        auth_type               TEXT NOT NULL CHECK (auth_type IN ('api_key', 'oauth')),
        encrypted_secret        BLOB NOT NULL,
        encrypted_refresh_token BLOB,
        expires_at              TEXT,
        created_at              TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
        updated_at              TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
        UNIQUE (tenant, user_id, provider)
    );
";

impl CredentialStore {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path).context("opening credentials store")?;
        conn.execute_batch(SCHEMA).context("initializing credentials schema")?;
        Ok(Self { conn })
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().context("opening in-memory credentials store")?;
        conn.execute_batch(SCHEMA).context("initializing credentials schema")?;
        Ok(Self { conn })
    }

    /// Encrypts and upserts one tenant's credential for `provider` — a
    /// second call for the same `(tenant, provider)` updates in place
    /// (e.g. a rotated API key, or a refreshed OAuth token), not a
    /// duplicate row, matching the idempotent-upsert reasoning already
    /// used everywhere else this rebuild (`upsert_external_task`, node
    /// upsert, etc).
    pub fn store_credential(&self, secrets: &dyn SecretsProvider, tenant: &str, new: NewCredential<'_>) -> Result<()> {
        let key = secrets.integration_key(tenant)?;
        let encrypted_secret = encrypt(&key, new.secret)?;
        let encrypted_refresh_token = new.refresh_token.map(|t| encrypt(&key, t)).transpose()?;

        self.conn.execute(
            "INSERT INTO credentials (tenant, provider, auth_type, encrypted_secret, encrypted_refresh_token, expires_at, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP) \
             ON CONFLICT (tenant, provider) DO UPDATE SET \
                auth_type = excluded.auth_type, \
                encrypted_secret = excluded.encrypted_secret, \
                encrypted_refresh_token = excluded.encrypted_refresh_token, \
                expires_at = excluded.expires_at, \
                updated_at = CURRENT_TIMESTAMP",
            rusqlite::params![tenant, new.provider, new.auth_type.as_str(), encrypted_secret, encrypted_refresh_token, new.expires_at],
        )?;
        Ok(())
    }

    /// Decrypts and returns `tenant`'s credential for `provider`, if one
    /// exists — `None` (not an error) when nothing is stored, so callers
    /// can cleanly fall back to an env var.
    pub fn get_credential(&self, secrets: &dyn SecretsProvider, tenant: &str, provider: &str) -> Result<Option<DecryptedCredential>> {
        let row: Option<CredentialRow> = self
            .conn
            .query_row(
                "SELECT auth_type, encrypted_secret, encrypted_refresh_token, expires_at FROM credentials WHERE tenant = ?1 AND provider = ?2",
                rusqlite::params![tenant, provider],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        let Some((auth_type, encrypted_secret, encrypted_refresh_token, expires_at)) = row else {
            return Ok(None);
        };

        let key = secrets.integration_key(tenant)?;
        let secret = decrypt(&key, &encrypted_secret)?;
        let refresh_token = encrypted_refresh_token.map(|blob| decrypt(&key, &blob)).transpose()?;
        Ok(Some(DecryptedCredential { provider: provider.to_string(), auth_type: AuthType::from_str(&auth_type)?, secret, refresh_token, expires_at }))
    }

    /// Every provider `tenant` has a credential stored for — summaries
    /// only, never the decrypted secret (no `SecretsProvider` needed to
    /// call this at all, by design: listing what's connected shouldn't
    /// require the ability to decrypt anything).
    pub fn list_credentials(&self, tenant: &str) -> Result<Vec<CredentialSummary>> {
        let mut stmt = self.conn.prepare("SELECT provider, auth_type, created_at, updated_at FROM credentials WHERE tenant = ?1 ORDER BY provider")?;
        let rows = stmt.query_map([tenant], |r| {
            let auth_type: String = r.get(1)?;
            Ok((r.get::<_, String>(0)?, auth_type, r.get::<_, String>(2)?, r.get::<_, String>(3)?))
        })?;
        rows.map(|r| {
            let (provider, auth_type, created_at, updated_at) = r?;
            Ok(CredentialSummary { provider, auth_type: AuthType::from_str(&auth_type)?, created_at, updated_at })
        })
        .collect()
    }

    /// Returns `true` if a credential was actually removed.
    pub fn delete_credential(&self, tenant: &str, provider: &str) -> Result<bool> {
        let changed = self.conn.execute("DELETE FROM credentials WHERE tenant = ?1 AND provider = ?2", rusqlite::params![tenant, provider])?;
        Ok(changed > 0)
    }

    /// Deletes every org-wide credential for `tenant` -- a leaf step of the
    /// org-deletion cascade (`POST /team/delete-organization`). Returns the
    /// row count removed, purely informational.
    pub fn delete_all_for_tenant(&self, tenant: &str) -> Result<usize> {
        self.conn.execute("DELETE FROM credentials WHERE tenant = ?1", [tenant]).map_err(Into::into)
    }

    /// Deletes every member's personal credential for `tenant` (all users,
    /// all providers) -- the personal-layer counterpart to
    /// `delete_all_for_tenant`, same cascade step.
    pub fn delete_all_user_credentials_for_tenant(&self, tenant: &str) -> Result<usize> {
        self.conn.execute("DELETE FROM user_credentials WHERE tenant = ?1", [tenant]).map_err(Into::into)
    }

    /// Personal-layer counterpart to `store_credential` -- same upsert
    /// semantics, scoped by `(tenant, user_id, provider)` instead of just
    /// `(tenant, provider)`, and encrypted under `user_integration_key`
    /// (a real per-user key, not the org-wide `integration_key` reused with
    /// SQL-only scoping -- see that trait method's doc comment for why).
    pub fn store_user_credential(&self, secrets: &dyn SecretsProvider, tenant: &str, user_id: i64, new: NewCredential<'_>) -> Result<()> {
        let key = secrets.user_integration_key(tenant, user_id)?;
        let encrypted_secret = encrypt(&key, new.secret)?;
        let encrypted_refresh_token = new.refresh_token.map(|t| encrypt(&key, t)).transpose()?;

        self.conn.execute(
            "INSERT INTO user_credentials (tenant, user_id, provider, auth_type, encrypted_secret, encrypted_refresh_token, expires_at, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP) \
             ON CONFLICT (tenant, user_id, provider) DO UPDATE SET \
                auth_type = excluded.auth_type, \
                encrypted_secret = excluded.encrypted_secret, \
                encrypted_refresh_token = excluded.encrypted_refresh_token, \
                expires_at = excluded.expires_at, \
                updated_at = CURRENT_TIMESTAMP",
            rusqlite::params![tenant, user_id, new.provider, new.auth_type.as_str(), encrypted_secret, encrypted_refresh_token, new.expires_at],
        )?;
        Ok(())
    }

    pub fn get_user_credential(&self, secrets: &dyn SecretsProvider, tenant: &str, user_id: i64, provider: &str) -> Result<Option<DecryptedCredential>> {
        let row: Option<CredentialRow> = self
            .conn
            .query_row(
                "SELECT auth_type, encrypted_secret, encrypted_refresh_token, expires_at FROM user_credentials WHERE tenant = ?1 AND user_id = ?2 AND provider = ?3",
                rusqlite::params![tenant, user_id, provider],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        let Some((auth_type, encrypted_secret, encrypted_refresh_token, expires_at)) = row else {
            return Ok(None);
        };

        let key = secrets.user_integration_key(tenant, user_id)?;
        let secret = decrypt(&key, &encrypted_secret)?;
        let refresh_token = encrypted_refresh_token.map(|blob| decrypt(&key, &blob)).transpose()?;
        Ok(Some(DecryptedCredential { provider: provider.to_string(), auth_type: AuthType::from_str(&auth_type)?, secret, refresh_token, expires_at }))
    }

    /// Every provider `user_id` (within `tenant`) has a personal credential
    /// stored for -- summaries only, same "no decryption needed to list"
    /// design as `list_credentials`.
    pub fn list_user_credentials(&self, tenant: &str, user_id: i64) -> Result<Vec<CredentialSummary>> {
        let mut stmt = self.conn.prepare("SELECT provider, auth_type, created_at, updated_at FROM user_credentials WHERE tenant = ?1 AND user_id = ?2 ORDER BY provider")?;
        let rows = stmt.query_map(rusqlite::params![tenant, user_id], |r| {
            let auth_type: String = r.get(1)?;
            Ok((r.get::<_, String>(0)?, auth_type, r.get::<_, String>(2)?, r.get::<_, String>(3)?))
        })?;
        rows.map(|r| {
            let (provider, auth_type, created_at, updated_at) = r?;
            Ok(CredentialSummary { provider, auth_type: AuthType::from_str(&auth_type)?, created_at, updated_at })
        })
        .collect()
    }

    /// Returns `true` if a personal credential was actually removed.
    pub fn delete_user_credential(&self, tenant: &str, user_id: i64, provider: &str) -> Result<bool> {
        let changed = self.conn.execute("DELETE FROM user_credentials WHERE tenant = ?1 AND user_id = ?2 AND provider = ?3", rusqlite::params![tenant, user_id, provider])?;
        Ok(changed > 0)
    }
}

/// `nonce (12 bytes) || ciphertext` in one BLOB — deliberately not a
/// separate nonce column, so there's only one place the two could ever get
/// out of sync. A fresh random nonce every call: AES-GCM nonce reuse under
/// the same key is a real, critical key-recovery vulnerability, not a
/// theoretical concern — confirmed via `rustcrypto/aeads` docs, not
/// guessed, that `Aes256Gcm`'s nonce is 96 bits / 12 bytes.
fn encrypt(key_bytes: &[u8], plaintext: &str) -> Result<Vec<u8>> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key_bytes));

    let mut nonce_bytes = [0u8; 12];
    getrandom::fill(&mut nonce_bytes).context("generating AES-GCM nonce")?;
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher.encrypt(nonce, plaintext.as_bytes()).map_err(|e| anyhow::anyhow!("encrypting credential: {e}"))?;
    let mut out = Vec::with_capacity(12 + ciphertext.len());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

fn decrypt(key_bytes: &[u8], blob: &[u8]) -> Result<Zeroizing<String>> {
    anyhow::ensure!(blob.len() > 12, "encrypted credential blob is too short to contain a nonce");
    let (nonce_bytes, ciphertext) = blob.split_at(12);

    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key_bytes));
    let nonce = Nonce::from_slice(nonce_bytes);

    let plaintext = cipher.decrypt(nonce, ciphertext).map_err(|e| anyhow::anyhow!("decrypting credential (wrong key, or tampered/corrupted data): {e}"))?;
    Ok(Zeroizing::new(String::from_utf8(plaintext).context("decrypted credential was not valid UTF-8")?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentops_repo_access::secrets::EnvSecretsProvider;

    fn secrets() -> EnvSecretsProvider {
        EnvSecretsProvider::from_hex(&"ab".repeat(32)).unwrap()
    }

    #[test]
    fn store_then_get_round_trips_the_plaintext_secret() {
        let store = CredentialStore::open_in_memory().unwrap();
        let secrets = secrets();
        store.store_credential(&secrets, "tenant-a", NewCredential { provider: "linear", auth_type: AuthType::ApiKey, secret: "lin_api_supersecret", refresh_token: None, expires_at: None }).unwrap();

        let cred = store.get_credential(&secrets, "tenant-a", "linear").unwrap().unwrap();
        assert_eq!(*cred.secret, "lin_api_supersecret");
        assert_eq!(cred.auth_type, AuthType::ApiKey);
        assert!(cred.refresh_token.is_none());
    }

    #[test]
    fn store_round_trips_the_refresh_token_too() {
        let store = CredentialStore::open_in_memory().unwrap();
        let secrets = secrets();
        store.store_credential(&secrets, "tenant-a", NewCredential { provider: "linear", auth_type: AuthType::OAuth, secret: "access-token", refresh_token: Some("refresh-token"), expires_at: Some("2027-01-01T00:00:00Z") }).unwrap();

        let cred = store.get_credential(&secrets, "tenant-a", "linear").unwrap().unwrap();
        assert_eq!(*cred.secret, "access-token");
        assert_eq!(cred.refresh_token.map(|t| t.to_string()), Some("refresh-token".to_string()));
        assert_eq!(cred.expires_at.as_deref(), Some("2027-01-01T00:00:00Z"));
    }

    #[test]
    fn a_second_store_call_for_the_same_tenant_and_provider_updates_in_place_not_a_duplicate() {
        let store = CredentialStore::open_in_memory().unwrap();
        let secrets = secrets();
        store.store_credential(&secrets, "tenant-a", NewCredential { provider: "linear", auth_type: AuthType::ApiKey, secret: "old-key", refresh_token: None, expires_at: None }).unwrap();
        store.store_credential(&secrets, "tenant-a", NewCredential { provider: "linear", auth_type: AuthType::ApiKey, secret: "new-key", refresh_token: None, expires_at: None }).unwrap();

        let cred = store.get_credential(&secrets, "tenant-a", "linear").unwrap().unwrap();
        assert_eq!(*cred.secret, "new-key");
        assert_eq!(store.list_credentials("tenant-a").unwrap().len(), 1, "must update in place, not duplicate");
    }

    #[test]
    fn get_credential_returns_none_not_an_error_when_nothing_is_stored() {
        let store = CredentialStore::open_in_memory().unwrap();
        let secrets = secrets();
        assert!(store.get_credential(&secrets, "tenant-a", "linear").unwrap().is_none());
    }

    #[test]
    fn one_tenant_never_sees_another_tenants_credentials() {
        let store = CredentialStore::open_in_memory().unwrap();
        let secrets = secrets();
        store.store_credential(&secrets, "tenant-a", NewCredential { provider: "linear", auth_type: AuthType::ApiKey, secret: "tenant-a-secret", refresh_token: None, expires_at: None }).unwrap();

        assert!(store.get_credential(&secrets, "tenant-b", "linear").unwrap().is_none());
        assert!(store.list_credentials("tenant-b").unwrap().is_empty());
    }

    #[test]
    fn list_credentials_never_includes_the_decrypted_secret_field() {
        let store = CredentialStore::open_in_memory().unwrap();
        let secrets = secrets();
        store.store_credential(&secrets, "tenant-a", NewCredential { provider: "linear", auth_type: AuthType::ApiKey, secret: "super-secret-value", refresh_token: None, expires_at: None }).unwrap();

        let listed = store.list_credentials("tenant-a").unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].provider, "linear");
        // `CredentialSummary` structurally has no secret field at all — this
        // test exists mainly to document that guarantee, matching the same
        // "never leak the sensitive field" discipline as `ConnectionView`.
    }

    #[test]
    fn delete_credential_removes_it_and_reports_true() {
        let store = CredentialStore::open_in_memory().unwrap();
        let secrets = secrets();
        store.store_credential(&secrets, "tenant-a", NewCredential { provider: "linear", auth_type: AuthType::ApiKey, secret: "secret", refresh_token: None, expires_at: None }).unwrap();

        assert!(store.delete_credential("tenant-a", "linear").unwrap());
        assert!(store.get_credential(&secrets, "tenant-a", "linear").unwrap().is_none());
        assert!(!store.delete_credential("tenant-a", "linear").unwrap(), "deleting again must report false, not error");
    }

    #[test]
    fn delete_all_for_tenant_removes_only_that_tenants_org_wide_credentials() {
        let store = CredentialStore::open_in_memory().unwrap();
        let secrets = secrets();
        store.store_credential(&secrets, "tenant-a", NewCredential { provider: "linear", auth_type: AuthType::ApiKey, secret: "s1", refresh_token: None, expires_at: None }).unwrap();
        store.store_credential(&secrets, "tenant-a", NewCredential { provider: "anthropic", auth_type: AuthType::ApiKey, secret: "s2", refresh_token: None, expires_at: None }).unwrap();
        store.store_credential(&secrets, "tenant-b", NewCredential { provider: "linear", auth_type: AuthType::ApiKey, secret: "s3", refresh_token: None, expires_at: None }).unwrap();

        let deleted = store.delete_all_for_tenant("tenant-a").unwrap();
        assert_eq!(deleted, 2);
        assert!(store.list_credentials("tenant-a").unwrap().is_empty());
        assert_eq!(store.list_credentials("tenant-b").unwrap().len(), 1, "a different tenant's credentials must survive");
    }

    #[test]
    fn delete_all_user_credentials_for_tenant_removes_every_members_personal_credentials() {
        let store = CredentialStore::open_in_memory().unwrap();
        let secrets = secrets();
        store.store_user_credential(&secrets, "tenant-a", 1, NewCredential { provider: "linear", auth_type: AuthType::ApiKey, secret: "alice", refresh_token: None, expires_at: None }).unwrap();
        store.store_user_credential(&secrets, "tenant-a", 2, NewCredential { provider: "linear", auth_type: AuthType::ApiKey, secret: "bob", refresh_token: None, expires_at: None }).unwrap();
        store.store_user_credential(&secrets, "tenant-b", 1, NewCredential { provider: "linear", auth_type: AuthType::ApiKey, secret: "carol", refresh_token: None, expires_at: None }).unwrap();

        let deleted = store.delete_all_user_credentials_for_tenant("tenant-a").unwrap();
        assert_eq!(deleted, 2);
        assert!(store.list_user_credentials("tenant-a", 1).unwrap().is_empty());
        assert!(store.list_user_credentials("tenant-a", 2).unwrap().is_empty());
        assert_eq!(store.list_user_credentials("tenant-b", 1).unwrap().len(), 1, "a different tenant's personal credentials must survive");
    }

    #[test]
    fn the_stored_blob_never_contains_the_plaintext_secret() {
        let store = CredentialStore::open_in_memory().unwrap();
        let secrets = secrets();
        store.store_credential(&secrets, "tenant-a", NewCredential { provider: "linear", auth_type: AuthType::ApiKey, secret: "lin_api_supersecret", refresh_token: None, expires_at: None }).unwrap();

        let blob: Vec<u8> = store.conn.query_row("SELECT encrypted_secret FROM credentials WHERE tenant = 'tenant-a'", [], |r| r.get(0)).unwrap();
        assert!(!blob.windows(b"lin_api_supersecret".len()).any(|w| w == b"lin_api_supersecret"), "the plaintext secret must never appear in the stored blob");
    }

    #[test]
    fn store_then_get_user_credential_round_trips_the_plaintext_secret() {
        let store = CredentialStore::open_in_memory().unwrap();
        let secrets = secrets();
        store.store_user_credential(&secrets, "tenant-a", 1, NewCredential { provider: "linear", auth_type: AuthType::ApiKey, secret: "personal-lin-key", refresh_token: None, expires_at: None }).unwrap();

        let cred = store.get_user_credential(&secrets, "tenant-a", 1, "linear").unwrap().unwrap();
        assert_eq!(*cred.secret, "personal-lin-key");
    }

    #[test]
    fn a_second_store_call_for_the_same_user_and_provider_updates_in_place_not_a_duplicate() {
        let store = CredentialStore::open_in_memory().unwrap();
        let secrets = secrets();
        store.store_user_credential(&secrets, "tenant-a", 1, NewCredential { provider: "linear", auth_type: AuthType::ApiKey, secret: "old", refresh_token: None, expires_at: None }).unwrap();
        store.store_user_credential(&secrets, "tenant-a", 1, NewCredential { provider: "linear", auth_type: AuthType::ApiKey, secret: "new", refresh_token: None, expires_at: None }).unwrap();

        let cred = store.get_user_credential(&secrets, "tenant-a", 1, "linear").unwrap().unwrap();
        assert_eq!(*cred.secret, "new");
        assert_eq!(store.list_user_credentials("tenant-a", 1).unwrap().len(), 1);
    }

    #[test]
    fn two_members_of_the_same_tenant_have_fully_isolated_personal_credentials() {
        let store = CredentialStore::open_in_memory().unwrap();
        let secrets = secrets();
        store.store_user_credential(&secrets, "tenant-a", 1, NewCredential { provider: "linear", auth_type: AuthType::ApiKey, secret: "alice-secret", refresh_token: None, expires_at: None }).unwrap();
        store.store_user_credential(&secrets, "tenant-a", 2, NewCredential { provider: "linear", auth_type: AuthType::ApiKey, secret: "bob-secret", refresh_token: None, expires_at: None }).unwrap();

        assert_eq!(*store.get_user_credential(&secrets, "tenant-a", 1, "linear").unwrap().unwrap().secret, "alice-secret");
        assert_eq!(*store.get_user_credential(&secrets, "tenant-a", 2, "linear").unwrap().unwrap().secret, "bob-secret");
        assert_eq!(store.list_user_credentials("tenant-a", 1).unwrap().len(), 1);
        assert_eq!(store.list_user_credentials("tenant-a", 2).unwrap().len(), 1);
    }

    #[test]
    fn a_personal_credential_is_isolated_from_the_org_wide_vault_even_for_the_same_provider() {
        let store = CredentialStore::open_in_memory().unwrap();
        let secrets = secrets();
        store.store_credential(&secrets, "tenant-a", NewCredential { provider: "linear", auth_type: AuthType::ApiKey, secret: "org-wide-secret", refresh_token: None, expires_at: None }).unwrap();
        store.store_user_credential(&secrets, "tenant-a", 1, NewCredential { provider: "linear", auth_type: AuthType::ApiKey, secret: "personal-secret", refresh_token: None, expires_at: None }).unwrap();

        assert_eq!(*store.get_credential(&secrets, "tenant-a", "linear").unwrap().unwrap().secret, "org-wide-secret");
        assert_eq!(*store.get_user_credential(&secrets, "tenant-a", 1, "linear").unwrap().unwrap().secret, "personal-secret");
    }

    #[test]
    fn a_personal_credentials_encrypted_blob_cannot_be_decrypted_under_the_org_wide_key() {
        let store = CredentialStore::open_in_memory().unwrap();
        let secrets = secrets();
        store.store_user_credential(&secrets, "tenant-a", 1, NewCredential { provider: "linear", auth_type: AuthType::ApiKey, secret: "personal-secret", refresh_token: None, expires_at: None }).unwrap();

        let blob: Vec<u8> = store.conn.query_row("SELECT encrypted_secret FROM user_credentials WHERE tenant = 'tenant-a' AND user_id = 1", [], |r| r.get(0)).unwrap();
        let org_key = secrets.integration_key("tenant-a").unwrap();
        assert!(decrypt(&org_key, &blob).is_err(), "a personal credential must be cryptographically isolated from the org-wide key, not just SQL-row-scoped");
    }

    #[test]
    fn delete_user_credential_removes_it_and_reports_true() {
        let store = CredentialStore::open_in_memory().unwrap();
        let secrets = secrets();
        store.store_user_credential(&secrets, "tenant-a", 1, NewCredential { provider: "linear", auth_type: AuthType::ApiKey, secret: "secret", refresh_token: None, expires_at: None }).unwrap();

        assert!(store.delete_user_credential("tenant-a", 1, "linear").unwrap());
        assert!(store.get_user_credential(&secrets, "tenant-a", 1, "linear").unwrap().is_none());
        assert!(!store.delete_user_credential("tenant-a", 1, "linear").unwrap(), "deleting again must report false, not error");
    }

    #[test]
    fn get_user_credential_returns_none_not_an_error_when_nothing_is_stored() {
        let store = CredentialStore::open_in_memory().unwrap();
        let secrets = secrets();
        assert!(store.get_user_credential(&secrets, "tenant-a", 1, "linear").unwrap().is_none());
    }

    #[test]
    fn decrypting_with_the_wrong_tenant_key_fails_rather_than_silently_returning_garbage() {
        let store = CredentialStore::open_in_memory().unwrap();
        let secrets = secrets();
        store.store_credential(&secrets, "tenant-a", NewCredential { provider: "linear", auth_type: AuthType::ApiKey, secret: "secret", refresh_token: None, expires_at: None }).unwrap();

        // Same row, but ask for it under the wrong tenant's derived key —
        // AES-GCM's authentication tag must reject this, not decrypt to
        // corrupted-but-plausible bytes.
        let result = store.get_credential(&secrets, "tenant-b", "linear");
        assert!(result.unwrap().is_none(), "no row exists for tenant-b at all — this specifically exercises the lookup-scoping path");

        // Force the cross-tenant-key-mismatch path directly against the
        // encrypt/decrypt primitives (bypassing the store's own
        // tenant-scoped lookup) to prove decryption itself, not just
        // lookup scoping, rejects the wrong key.
        let key_a = secrets.integration_key("tenant-a").unwrap();
        let key_b = secrets.integration_key("tenant-b").unwrap();
        let ciphertext = encrypt(&key_a, "secret").unwrap();
        assert!(decrypt(&key_b, &ciphertext).is_err());
    }
}

//! Where the deploy-key encryption passphrase actually comes from.
//!
//! The rest of this crate (`generate_deploy_keypair`/`UnlockedKey::unlock`)
//! deliberately takes the passphrase as a bare parameter — it's a primitive,
//! not a policy. `SecretsProvider` is the policy boundary: it's the one
//! place that decides how a per-repo passphrase is derived, so swapping "env
//! var master key" for "AWS KMS" or "HashiCorp Vault" later is a new
//! implementation of this trait, not a rewrite of the crypto.
//!
//! [`EnvSecretsProvider`] is the only implementation shipped today. It's a
//! real, working, testable default for a single self-hosted deployment (one
//! operator, one env var) — genuinely fine for that case. It is NOT
//! sufficient for a multi-tenant hosted product: the master key sits in that
//! deployment's process environment, so anyone who can read the heavy-tier
//! process's env (a misconfigured log line, a debug endpoint, host
//! compromise) can derive every tenant's repo passphrases at once. A
//! KMS-backed provider — where the master key never leaves a hardware/cloud
//! boundary and every derivation is an audited API call — is the real
//! requirement before any actual paying customer's deploy key is generated;
//! that implementation isn't written yet (see SECURITY.md).

use anyhow::{bail, Context, Result};
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use zeroize::Zeroizing;

/// Supplies the passphrase used to encrypt/decrypt one repo connection's
/// deploy-key private key. Scoped by `(tenant, repo_id)` so a compromised
/// derivation for one repo doesn't help an attacker with any other repo,
/// even within the same tenant.
pub trait SecretsProvider {
    fn repo_passphrase(&self, tenant: &str, repo_id: &str) -> Result<Zeroizing<Vec<u8>>>;

    /// A per-tenant 32-byte AES-256-GCM key for `agentops-integrations`'
    /// credentials vault — deliberately a *different* derivation from
    /// `repo_passphrase` (a distinct leading domain-separation label fed
    /// into the HMAC, not just a different context string appended after
    /// `tenant`), so even a same-named `tenant` can never make the two
    /// derivations collide: they diverge on the very first byte fed to the
    /// MAC, not just somewhere in the middle of the input.
    fn integration_key(&self, tenant: &str) -> Result<Zeroizing<Vec<u8>>>;

    /// A per-*user* (not just per-tenant) 32-byte AES-256-GCM key for a
    /// member's personal integration credentials (e.g. their own Linear
    /// connection, separate from the org-wide vault `integration_key`
    /// serves). Deliberately its own domain-separation label, not
    /// `integration_key(tenant)` reused with the row scoped by `user_id` in
    /// SQL alone -- reusing that key would mean every member's personal
    /// secret, and the org-wide vault's secrets, all sit under one
    /// identical key, with isolation enforced *only* by a `WHERE user_id =
    /// ?` clause a future bug (or a debug tool reading the table directly)
    /// could bypass. This derivation diverges from every other one on this
    /// trait starting at the first byte fed to the MAC, same reasoning
    /// `integration_key` itself documents.
    fn user_integration_key(&self, tenant: &str, user_id: i64) -> Result<Zeroizing<Vec<u8>>>;
}

/// Derives a per-repo passphrase from one master key via
/// HMAC-SHA256(master_key, tenant || 0x00 || repo_id) — a standard KDF
/// pattern (a single high-entropy secret plus a public context string
/// yields per-context secrets) rather than needing to persist N separate
/// passphrases anywhere. The master key itself is sourced from
/// `AGENTOPS_SECRETS_MASTER_KEY` (64 hex chars = 32 bytes) at construction
/// time and held zeroized in memory for the provider's lifetime.
pub struct EnvSecretsProvider {
    master_key: Zeroizing<Vec<u8>>,
}

impl EnvSecretsProvider {
    pub fn from_env() -> Result<Self> {
        let hex_key = std::env::var("AGENTOPS_SECRETS_MASTER_KEY")
            .context("AGENTOPS_SECRETS_MASTER_KEY must be set (64 hex chars = 32 bytes) — generate with `openssl rand -hex 32`")?;
        Self::from_hex(&hex_key)
    }

    pub fn from_hex(hex_key: &str) -> Result<Self> {
        let bytes = decode_hex(hex_key)?;
        if bytes.len() < 32 {
            bail!("master key must be at least 32 bytes (got {}) — a shorter key weakens every derived repo passphrase at once", bytes.len());
        }
        Ok(Self { master_key: Zeroizing::new(bytes) })
    }
}

impl SecretsProvider for EnvSecretsProvider {
    fn repo_passphrase(&self, tenant: &str, repo_id: &str) -> Result<Zeroizing<Vec<u8>>> {
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.master_key).context("constructing HMAC")?;
        mac.update(tenant.as_bytes());
        mac.update(&[0u8]);
        mac.update(repo_id.as_bytes());
        Ok(Zeroizing::new(mac.finalize().into_bytes().to_vec()))
    }

    fn integration_key(&self, tenant: &str) -> Result<Zeroizing<Vec<u8>>> {
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.master_key).context("constructing HMAC")?;
        mac.update(b"integration-key\0");
        mac.update(tenant.as_bytes());
        Ok(Zeroizing::new(mac.finalize().into_bytes().to_vec()))
    }

    fn user_integration_key(&self, tenant: &str, user_id: i64) -> Result<Zeroizing<Vec<u8>>> {
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.master_key).context("constructing HMAC")?;
        mac.update(b"user-integration-key\0");
        mac.update(tenant.as_bytes());
        mac.update(&[0u8]);
        mac.update(&user_id.to_le_bytes());
        Ok(Zeroizing::new(mac.finalize().into_bytes().to_vec()))
    }
}

fn decode_hex(s: &str) -> Result<Vec<u8>> {
    if s.len() % 2 != 0 {
        bail!("hex string has odd length");
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).context("invalid hex character"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_provider() -> EnvSecretsProvider {
        EnvSecretsProvider::from_hex(&"ab".repeat(32)).unwrap()
    }

    #[test]
    fn same_repo_derives_the_same_passphrase_every_time() {
        let provider = test_provider();
        let a = provider.repo_passphrase("acme", "repo-1").unwrap();
        let b = provider.repo_passphrase("acme", "repo-1").unwrap();
        assert_eq!(*a, *b);
    }

    #[test]
    fn different_repos_derive_different_passphrases() {
        let provider = test_provider();
        let a = provider.repo_passphrase("acme", "repo-1").unwrap();
        let b = provider.repo_passphrase("acme", "repo-2").unwrap();
        assert_ne!(*a, *b);
    }

    #[test]
    fn different_tenants_derive_different_passphrases_for_the_same_repo_id() {
        // Guards against a concatenation bug like tenant="ac" + repo="me-1"
        // colliding with tenant="acme" + repo="1" if the separator byte were
        // ever dropped.
        let provider = test_provider();
        let a = provider.repo_passphrase("ac", "me-1").unwrap();
        let b = provider.repo_passphrase("acme", "1").unwrap();
        assert_ne!(*a, *b);
    }

    #[test]
    fn rejects_a_too_short_master_key() {
        assert!(EnvSecretsProvider::from_hex("ab").is_err());
    }

    #[test]
    fn rejects_odd_length_hex() {
        assert!(EnvSecretsProvider::from_hex(&"a".repeat(63)).is_err());
    }

    #[test]
    fn integration_key_is_deterministic_per_tenant_and_differs_across_tenants() {
        let provider = test_provider();
        let a1 = provider.integration_key("acme").unwrap();
        let a2 = provider.integration_key("acme").unwrap();
        let b = provider.integration_key("other").unwrap();
        assert_eq!(*a1, *a2);
        assert_ne!(*a1, *b);
    }

    #[test]
    fn integration_key_never_collides_with_repo_passphrase_even_for_the_same_tenant() {
        let provider = test_provider();
        let integration = provider.integration_key("acme").unwrap();
        let repo = provider.repo_passphrase("acme", "").unwrap();
        assert_ne!(*integration, *repo, "the two derivations must diverge by construction, not by luck");
    }

    #[test]
    fn user_integration_key_is_deterministic_per_user_and_differs_across_users_and_tenants() {
        let provider = test_provider();
        let u1a = provider.user_integration_key("acme", 1).unwrap();
        let u1b = provider.user_integration_key("acme", 1).unwrap();
        let u2 = provider.user_integration_key("acme", 2).unwrap();
        let other_tenant = provider.user_integration_key("other", 1).unwrap();
        assert_eq!(*u1a, *u1b);
        assert_ne!(*u1a, *u2, "different users in the same tenant must derive different keys");
        assert_ne!(*u1a, *other_tenant, "the same user id in a different tenant must derive a different key");
    }

    #[test]
    fn user_integration_key_never_collides_with_integration_key_or_repo_passphrase() {
        let provider = test_provider();
        let user_key = provider.user_integration_key("acme", 1).unwrap();
        let org_key = provider.integration_key("acme").unwrap();
        let repo_key = provider.repo_passphrase("acme", "1").unwrap();
        assert_ne!(*user_key, *org_key, "a member's personal key must not be the same key that encrypts the org-wide vault");
        assert_ne!(*user_key, *repo_key);
    }
}

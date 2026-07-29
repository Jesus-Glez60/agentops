//! API-key generation and verification for `agentops-api`/`docbrain-api`.
//!
//! Only a SHA-256 hash of a key is ever meant to be stored/configured — the
//! raw key is shown to whoever generates it exactly once. Verification
//! compares hashes in constant time (`subtle::ConstantTimeEq`) so a network
//! attacker measuring response latency can't learn anything about a correct
//! key byte-by-byte.

use anyhow::{bail, Result};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

pub const API_KEY_PREFIX: &str = "ao_";

/// Generates a new random API key. Returns `(raw_key, hash)` — persist only
/// `hash` (e.g. in an env var the server reads at startup); hand `raw_key`
/// to whoever is meant to authenticate with it and don't keep a copy.
pub fn generate_api_key() -> Result<(String, String)> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("system RNG unavailable: {e}"))?;
    let raw = format!("{API_KEY_PREFIX}{}", to_hex(&bytes));
    let hash = hash_api_key(&raw);
    Ok((raw, hash))
}

/// SHA-256 hash of a raw API key, hex-encoded. Deterministic — this is a
/// lookup key, not a password hash, so no per-key salt: a leaked hash lets
/// an attacker recognize a matching raw key but not reverse it into one
/// (SHA-256 preimage resistance), and API keys are high-entropy random
/// tokens rather than user-chosen low-entropy passwords, so the
/// dictionary-attack concern a salted/slow hash defends against doesn't
/// apply here.
pub fn hash_api_key(raw: &str) -> String {
    to_hex(&Sha256::digest(raw.as_bytes()))
}

/// Constant-time comparison of a caller-supplied raw key against a stored
/// hash. Returns `Ok(())` on match, `Err` with a caller-safe message
/// otherwise (never echoes back what was compared).
pub fn verify_api_key(raw: &str, expected_hash: &str) -> Result<()> {
    let actual_hash = hash_api_key(raw);
    if actual_hash.as_bytes().ct_eq(expected_hash.as_bytes()).into() {
        Ok(())
    } else {
        bail!("invalid API key")
    }
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_key_verifies_against_its_own_hash() {
        let (raw, hash) = generate_api_key().unwrap();
        assert!(raw.starts_with(API_KEY_PREFIX));
        assert!(verify_api_key(&raw, &hash).is_ok());
    }

    #[test]
    fn wrong_key_is_rejected() {
        let (_, hash) = generate_api_key().unwrap();
        let (other_raw, _) = generate_api_key().unwrap();
        assert!(verify_api_key(&other_raw, &hash).is_err());
    }

    #[test]
    fn two_generated_keys_are_never_equal() {
        let (raw_a, _) = generate_api_key().unwrap();
        let (raw_b, _) = generate_api_key().unwrap();
        assert_ne!(raw_a, raw_b);
    }

    #[test]
    fn hash_is_deterministic() {
        let (raw, hash) = generate_api_key().unwrap();
        assert_eq!(hash_api_key(&raw), hash);
    }
}

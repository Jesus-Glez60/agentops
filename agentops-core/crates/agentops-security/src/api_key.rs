//! API-key generation and constant-time verification. Servers store only a
//! key's SHA-256 hash (`DOCBRAIN_API_KEY_HASH` etc.), never the raw key —
//! the raw value is shown to whoever generates it exactly once.

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

/// Generates a fresh random API key. Returns `(raw, hash)` — hand `raw` to
/// the caller who'll use it; configure the server with `hash`.
pub fn generate_api_key() -> Result<(String, String)> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).context("generating random API key bytes")?;
    let raw = format!("ao_{}", hex_encode(&bytes));
    let hash = hash_api_key(&raw);
    Ok((raw, hash))
}

/// Generates a fresh `AGENTOPS_SECRETS_MASTER_KEY` — 32 random bytes as 64
/// hex chars, no prefix (unlike [`generate_api_key`]'s `ao_`-prefixed raw
/// key), matching exactly what `openssl rand -hex 32` produces and what
/// `EnvSecretsProvider::from_hex` expects.
pub fn generate_master_key() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).context("generating random master key bytes")?;
    Ok(hex_encode(&bytes))
}

/// Constant-time check that `raw` hashes to `expected_hash` — timing-safe
/// so a byte-by-byte comparison can't leak how much of the key was guessed
/// correctly.
pub fn verify_api_key(raw: &str, expected_hash: &str) -> Result<()> {
    let computed = hash_api_key(raw);
    if bool::from(computed.as_bytes().ct_eq(expected_hash.as_bytes())) {
        Ok(())
    } else {
        anyhow::bail!("invalid API key")
    }
}

/// Public so callers that need to look a token up by its hash (e.g. a
/// session store indexing `sessions.token_hash`) can compute the same hash
/// without duplicating this logic — `verify_api_key` alone only helps once
/// you already know *which* row's hash to compare against.
pub fn hash_api_key(raw: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    hex_encode(&hasher.finalize())
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Shared body of the `require_api_key` Axum middleware every REST service
/// in this workspace used to hand-roll independently (`agentops-api`,
/// `docbrain-api`, `agentops-heavy-api` — byte-for-byte identical). Each
/// service's own middleware still exists as a thin wrapper (since its
/// `AppState` shape differs and `axum::middleware::from_fn_with_state`
/// needs a concrete state type), but the actual header-parsing/verify/
/// error-body logic now lives in exactly one place. `expected_hash: None`
/// means auth is disabled for this deployment (all requests pass) — the
/// same semantics every caller already had.
pub fn check_bearer_api_key(headers: &axum::http::HeaderMap, expected_hash: Option<&str>) -> Result<(), (axum::http::StatusCode, axum::Json<serde_json::Value>)> {
    let Some(expected_hash) = expected_hash else {
        return Ok(());
    };
    let provided = headers.get(axum::http::header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer "));
    match provided {
        Some(raw) if verify_api_key(raw, expected_hash).is_ok() => Ok(()),
        _ => Err((axum::http::StatusCode::UNAUTHORIZED, axum::Json(serde_json::json!({ "error": "missing or invalid API key" })))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_freshly_generated_key_verifies_against_its_own_hash() {
        let (raw, hash) = generate_api_key().unwrap();
        assert!(verify_api_key(&raw, &hash).is_ok());
    }

    #[test]
    fn a_wrong_key_is_rejected() {
        let (_, hash) = generate_api_key().unwrap();
        assert!(verify_api_key("not-the-right-key", &hash).is_err());
    }

    #[test]
    fn two_generated_keys_are_different() {
        let (raw1, _) = generate_api_key().unwrap();
        let (raw2, _) = generate_api_key().unwrap();
        assert_ne!(raw1, raw2);
    }

    #[test]
    fn generated_master_key_is_64_hex_chars_with_no_prefix() {
        let key = generate_master_key().unwrap();
        assert_eq!(key.len(), 64);
        assert!(key.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn two_generated_master_keys_are_different() {
        assert_ne!(generate_master_key().unwrap(), generate_master_key().unwrap());
    }
}

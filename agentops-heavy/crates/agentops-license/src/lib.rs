//! Offline license-key verification for the commercial heavy tier.
//!
//! Deliberately asymmetric (Ed25519), not HMAC: a heavy-tier binary shipped
//! to a customer must be able to verify a license *without* holding any
//! secret that would let its holder forge a new one. Only the verifying
//! (public) key is ever embedded in shipped code; the signing (private) key
//! stays with us and is never distributed. HMAC would require embedding our
//! shared secret in every customer's binary, which defeats the point.
//!
//! Key format: `agentops-license.v1.<base64url(payload_json)>.<base64url(signature)>`
//! where `payload_json` is the canonical JSON encoding of [`LicenseClaims`].

use anyhow::{anyhow, bail, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

const KEY_PREFIX: &str = "agentops-license.v1";

// TEMPORARY LOCAL-DEMO SWAP -- DO NOT COMMIT.
// This is a throwaway Ed25519 keypair generated on this machine
// (agentops-license/examples/gen_keypair.rs) solely to demo semantic
// search locally without touching the real offline production private
// key. The real value is below, commented out -- restore it before any
// commit and delete gen_keypair.rs once the demo key is no longer needed.
//
// pub const PRODUCTION_PUBLIC_KEY: [u8; 32] = [
//     0x64, 0x6b, 0x6a, 0xd2, 0xda, 0x1f, 0x48, 0x3e, 0xfe, 0x99, 0xd7, 0x12, 0x23, 0x19, 0x42, 0x87,
//     0xa8, 0x56, 0xc9, 0x5e, 0xf7, 0x5d, 0x71, 0x64, 0xd3, 0x0b, 0xa9, 0x70, 0x61, 0x7e, 0xa6, 0xc0,
// ];
pub const PRODUCTION_PUBLIC_KEY: [u8; 32] = [
    171, 175, 155, 248, 24, 47, 159, 118, 77, 121, 237, 212, 22, 35, 128, 25, 69, 176, 53, 178, 44,
    13, 97, 162, 99, 91, 197, 13, 38, 211, 253, 43,
];

/// Verifies a license key against the embedded production public key,
/// using the current wall-clock time for the expiry check. The one
/// heavy-tier binaries actually call at startup.
pub fn verify_production_license(key: &str) -> Result<LicenseClaims> {
    let verifying_key = VerifyingKey::from_bytes(&PRODUCTION_PUBLIC_KEY)
        .context("embedded production public key is malformed — this is a build bug, not a license problem")?;
    let now_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context("system clock is before the Unix epoch")?
        .as_secs();
    verify_license(key, &verifying_key, now_unix)
}

/// Reads `AGENTOPS_LICENSE_KEY` and verifies it against the embedded
/// production public key — the standard gate for any paid-tier-only
/// capability (semantic search today, more later). A feature should check
/// this once at startup and simply not enable itself if it fails, the same
/// pattern already used for other optional-but-gated capabilities elsewhere
/// in the heavy tier (e.g. API-key auth): log clearly, degrade the one
/// feature, don't crash the whole server over it.
pub fn require_valid_license_from_env() -> Result<LicenseClaims> {
    let key = std::env::var("AGENTOPS_LICENSE_KEY").context("AGENTOPS_LICENSE_KEY is not set")?;
    verify_production_license(&key)
}

/// The tier a license grants. Kept as an explicit enum (not a free-form
/// string) so a typo in a hand-issued license can't silently grant an
/// unintended tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    Heavy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LicenseClaims {
    pub licensee: String,
    pub tier: Tier,
    /// Unix seconds. Not enforced at sign time — just recorded for audit.
    pub issued_at: u64,
    /// Unix seconds. `None` means perpetual (no expiry check performed).
    pub expires_at: Option<u64>,
    /// `None` means unlimited seats.
    pub seat_limit: Option<u32>,
}

/// Signs `claims` with `signing_key`, producing a self-contained license
/// key string. This is issuer-side tooling — never called from code shipped
/// to a customer, since it requires the private key.
pub fn sign_license(claims: &LicenseClaims, signing_key: &SigningKey) -> Result<String> {
    let payload = serde_json::to_vec(claims).context("serializing license claims")?;
    let signature = signing_key.sign(&payload);
    Ok(format!(
        "{KEY_PREFIX}.{}.{}",
        URL_SAFE_NO_PAD.encode(&payload),
        URL_SAFE_NO_PAD.encode(signature.to_bytes()),
    ))
}

/// Verifies a license key string against `verifying_key`, checking both the
/// signature and (if present) expiry. Returns the validated claims on
/// success. This is the function the heavy-tier binary calls at startup.
pub fn verify_license(key: &str, verifying_key: &VerifyingKey, now_unix: u64) -> Result<LicenseClaims> {
    if !key.starts_with(KEY_PREFIX) {
        bail!("unrecognized license key format (expected prefix `{KEY_PREFIX}`)");
    }
    let rest = key
        .strip_prefix(KEY_PREFIX)
        .and_then(|r| r.strip_prefix('.'))
        .ok_or_else(|| anyhow!("malformed license key: missing payload"))?;
    let (payload_b64, sig_b64) = rest
        .split_once('.')
        .ok_or_else(|| anyhow!("malformed license key: missing signature segment"))?;

    let payload = URL_SAFE_NO_PAD
        .decode(payload_b64)
        .context("license payload is not valid base64")?;
    let sig_bytes = URL_SAFE_NO_PAD
        .decode(sig_b64)
        .context("license signature is not valid base64")?;
    let sig_bytes: [u8; 64] = sig_bytes
        .try_into()
        .map_err(|_| anyhow!("license signature is the wrong length"))?;
    let signature = Signature::from_bytes(&sig_bytes);

    verifying_key
        .verify(&payload, &signature)
        .map_err(|_| anyhow!("license signature verification failed — key was tampered with or not signed by us"))?;

    let claims: LicenseClaims = serde_json::from_slice(&payload).context("license payload is not valid claims JSON")?;

    if let Some(expires_at) = claims.expires_at {
        if now_unix > expires_at {
            bail!("license expired at {expires_at} (now {now_unix})");
        }
    }

    Ok(claims)
}

/// Generates a fresh Ed25519 keypair. Issuer-side tooling for minting the
/// one production keypair (private key kept offline, public key embedded in
/// shipped binaries) — also used by tests to sign/verify without touching
/// any real production key material.
pub fn generate_keypair() -> (SigningKey, VerifyingKey) {
    let signing_key = SigningKey::generate(&mut rand::rng());
    let verifying_key = signing_key.verifying_key();
    (signing_key, verifying_key)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_claims(expires_at: Option<u64>) -> LicenseClaims {
        LicenseClaims {
            licensee: "Acme Corp".into(),
            tier: Tier::Heavy,
            issued_at: 1_700_000_000,
            expires_at,
            seat_limit: Some(25),
        }
    }

    #[test]
    fn round_trips_a_valid_license() {
        let (signing_key, verifying_key) = generate_keypair();
        let claims = sample_claims(Some(2_000_000_000));
        let key = sign_license(&claims, &signing_key).unwrap();

        let verified = verify_license(&key, &verifying_key, 1_800_000_000).unwrap();
        assert_eq!(verified, claims);
    }

    #[test]
    fn rejects_an_expired_license() {
        let (signing_key, verifying_key) = generate_keypair();
        let claims = sample_claims(Some(1_000));
        let key = sign_license(&claims, &signing_key).unwrap();

        let err = verify_license(&key, &verifying_key, 2_000).unwrap_err();
        assert!(err.to_string().contains("expired"));
    }

    #[test]
    fn accepts_a_perpetual_license_with_no_expiry() {
        let (signing_key, verifying_key) = generate_keypair();
        let claims = sample_claims(None);
        let key = sign_license(&claims, &signing_key).unwrap();

        assert!(verify_license(&key, &verifying_key, 99_999_999_999).is_ok());
    }

    #[test]
    fn rejects_a_license_signed_by_a_different_key() {
        let (signing_key, _) = generate_keypair();
        let (_, wrong_verifying_key) = generate_keypair();
        let claims = sample_claims(None);
        let key = sign_license(&claims, &signing_key).unwrap();

        let err = verify_license(&key, &wrong_verifying_key, 0).unwrap_err();
        assert!(err.to_string().contains("verification failed"));
    }

    #[test]
    fn rejects_a_tampered_payload() {
        let (signing_key, verifying_key) = generate_keypair();
        let claims = sample_claims(None);
        let key = sign_license(&claims, &signing_key).unwrap();

        // Flip the tier claim by re-encoding a modified payload but keeping
        // the original signature, simulating an attacker editing the key.
        let mut tampered_claims = claims.clone();
        tampered_claims.seat_limit = Some(999_999);
        let tampered_payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&tampered_claims).unwrap());
        let parts: Vec<&str> = key.rsplitn(2, '.').collect();
        let original_sig = parts[0];
        let tampered_key = format!("{KEY_PREFIX}.{tampered_payload}.{original_sig}");

        let err = verify_license(&tampered_key, &verifying_key, 0).unwrap_err();
        assert!(err.to_string().contains("verification failed"));
    }

    #[test]
    fn embedded_production_public_key_is_well_formed() {
        // Doesn't assert anything about a real license (we don't have the
        // matching private key here) — just that what's embedded in shipped
        // binaries is actually a valid Ed25519 public key, so a bad copy/
        // paste into the constant fails CI instead of every customer's
        // license check at once.
        VerifyingKey::from_bytes(&PRODUCTION_PUBLIC_KEY).unwrap();
    }

    #[test]
    fn rejects_garbage_input_instead_of_panicking() {
        let (_, verifying_key) = generate_keypair();
        assert!(verify_license("not-a-license-key", &verifying_key, 0).is_err());
        assert!(verify_license("agentops-license.v1.", &verifying_key, 0).is_err());
        assert!(verify_license("agentops-license.v1.###.###", &verifying_key, 0).is_err());
    }
}

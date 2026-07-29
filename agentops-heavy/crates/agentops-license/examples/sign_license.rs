//! Issuer-side tooling: mints a real production license key.
//!
//! Never shipped to customers, never runs on customer infrastructure — it
//! requires the production private key, which lives offline, not in this
//! repo. Usage:
//!
//!   AGENTOPS_LICENSE_PRIVATE_KEY_HEX=<64 hex chars> cargo run -p agentops-license \
//!     --example sign_license -- "Acme Corp" [expires_at_unix] [seat_limit]

use agentops_license::{sign_license, LicenseClaims, Tier};
use ed25519_dalek::SigningKey;

fn main() {
    let hex_key = std::env::var("AGENTOPS_LICENSE_PRIVATE_KEY_HEX")
        .expect("set AGENTOPS_LICENSE_PRIVATE_KEY_HEX to the offline production private key");
    let bytes = decode_hex(&hex_key);
    let signing_key = SigningKey::from_bytes(&bytes.try_into().expect("private key must be 32 bytes"));

    let mut args = std::env::args().skip(1);
    let licensee = args.next().expect("usage: sign_license <licensee> [expires_at_unix] [seat_limit]");
    let expires_at = args.next().map(|s| s.parse().expect("expires_at must be a unix timestamp"));
    let seat_limit = args.next().map(|s| s.parse().expect("seat_limit must be a number"));

    let claims = LicenseClaims {
        licensee,
        tier: Tier::Heavy,
        issued_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
        expires_at,
        seat_limit,
    };

    let key = sign_license(&claims, &signing_key).expect("signing should not fail for well-formed claims");
    println!("{key}");
}

fn decode_hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("invalid hex"))
        .collect()
}

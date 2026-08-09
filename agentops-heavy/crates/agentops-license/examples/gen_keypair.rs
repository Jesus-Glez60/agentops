//! Throwaway local-dev tooling: generates a fresh Ed25519 keypair for
//! swapping in as a *local-only* verifying key, so semantic search can be
//! demoed without the real offline production private key ever touching
//! this machine. Never use the printed private key for anything but a local
//! `PRODUCTION_PUBLIC_KEY` swap in a working tree that never gets committed.

use ed25519_dalek::SigningKey;
use std::io::Read;

fn main() {
    // Sidesteps the workspace's `rand` version (0.10, whose OsRng export
    // path doesn't match what ed25519-dalek 2.x's SigningKey::generate
    // expects) by reading raw OS entropy directly -- this is throwaway
    // local-dev tooling, not something that needs to match rand's RNG traits.
    let mut seed = [0u8; 32];
    std::fs::File::open("/dev/urandom").expect("open /dev/urandom").read_exact(&mut seed).expect("read entropy");
    let signing_key = SigningKey::from_bytes(&seed);
    let verifying_key = signing_key.verifying_key();
    println!("private_hex={}", hex::encode(signing_key.to_bytes()));
    println!("public_bytes={:?}", verifying_key.to_bytes());
}

mod hex {
    pub fn encode(bytes: [u8; 32]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }
}

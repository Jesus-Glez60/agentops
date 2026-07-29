//! Verifies a license key against the embedded production public key.
//! What a heavy-tier binary does at startup — usage:
//!   cargo run -p agentops-license --example verify_license -- "<license key>"

use agentops_license::verify_production_license;

fn main() {
    let key = std::env::args().nth(1).expect("usage: verify_license <license key>");
    match verify_production_license(&key) {
        Ok(claims) => println!("VALID: {claims:?}"),
        Err(e) => {
            eprintln!("INVALID: {e:#}");
            std::process::exit(1);
        }
    }
}

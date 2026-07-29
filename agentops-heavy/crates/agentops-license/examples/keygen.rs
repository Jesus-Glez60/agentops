use agentops_license::generate_keypair;

fn main() {
    let (signing_key, verifying_key) = generate_keypair();
    println!("PRIVATE (keep offline, never commit):");
    println!("{}", hex(&signing_key.to_bytes()));
    println!();
    println!("PUBLIC (embed in shipped binary):");
    println!("{}", hex(&verifying_key.to_bytes()));
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

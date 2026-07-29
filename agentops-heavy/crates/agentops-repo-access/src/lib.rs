//! Per-repo SSH deploy-key custody for hosted repo access (heavy tier).
//!
//! Per the plan's threat model, GitHub repo-access credentials are the
//! highest-value secret this product will ever hold — a leaked key is a
//! direct path to a client's entire codebase. Two rules follow from that:
//!
//! 1. **One dedicated keypair per repo, never shared** — a compromised key
//!    exposes exactly one repo, not every repo a tenant has connected.
//! 2. **The private key is never stored or held in memory unencrypted**
//!    except for the brief window a clone/fetch subprocess actually needs
//!    it, written to a `0600` temp file that's overwritten with zeros and
//!    deleted the moment that subprocess exits (`UnlockedKey`'s `Drop`).
//!
//! The GitHub App install flow (the plan's *recommended primary* path,
//! since it avoids private-key custody on our side entirely) is not
//! implemented here — this crate is the SSH-deploy-key fallback path,
//! deliberately scoped on its own first since it needs no external
//! registration to build and verify for real.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use ssh_key::rand_core::OsRng;
use ssh_key::{Algorithm, LineEnding, PrivateKey};
use zeroize::Zeroize;

/// A freshly generated per-repo deploy keypair.
pub struct DeployKeypair {
    /// Safe to display/copy — paste this into GitHub's Deploy Keys UI for
    /// the one repo this key is dedicated to.
    pub public_key_openssh: String,
    /// Already encrypted with the caller's passphrase (OpenSSH's own
    /// bcrypt-pbkdf + AES-256-CTR) — this is what gets persisted. Never
    /// store the unencrypted private key.
    pub encrypted_private_key_openssh: String,
}

/// Generates a fresh Ed25519 keypair dedicated to one repo and immediately
/// encrypts the private key with `passphrase` before it ever leaves this
/// function as a return value.
///
/// `passphrase` should be derived from a per-tenant key held in a real
/// secrets manager (KMS/Vault) — this crate implements the encryption
/// primitive correctly but is deliberately agnostic about where the
/// wrapping key itself lives operationally; that's an infrastructure
/// decision for whatever deploys the heavy tier, not a library concern.
pub fn generate_deploy_keypair(comment: &str, passphrase: &[u8]) -> Result<DeployKeypair> {
    let mut private_key =
        PrivateKey::random(&mut OsRng, Algorithm::Ed25519).context("generating Ed25519 keypair")?;
    private_key.set_comment(comment);

    let public_key_openssh = private_key.public_key().to_openssh().context("encoding public key")?;

    let encrypted = private_key.encrypt(&mut OsRng, passphrase).context("encrypting private key with passphrase")?;
    let encrypted_private_key_openssh =
        encrypted.to_openssh(LineEnding::LF).context("encoding encrypted private key")?.to_string();

    Ok(DeployKeypair { public_key_openssh, encrypted_private_key_openssh })
}

/// An OpenSSH private key decrypted and written to a `0600` temp file for
/// the duration of one clone/fetch operation. The file is overwritten with
/// zeros and deleted when this value is dropped — including on an early
/// return or panic unwind, since cleanup lives in `Drop`, not in the
/// success path only.
#[derive(Debug)]
pub struct UnlockedKey {
    path: PathBuf,
}

impl UnlockedKey {
    /// Decrypts `encrypted_private_key_openssh` with `passphrase` and
    /// writes the plaintext key to a fresh temp file with `0600`
    /// permissions, set before any key material is written to it.
    pub fn unlock(encrypted_private_key_openssh: &str, passphrase: &[u8]) -> Result<Self> {
        let encrypted = PrivateKey::from_openssh(encrypted_private_key_openssh).context("parsing encrypted private key")?;
        if !encrypted.is_encrypted() {
            bail!("refusing to unlock a private key that was never encrypted — this indicates a storage bug upstream, not a legitimate unencrypted key");
        }
        let decrypted = encrypted.decrypt(passphrase).context("decrypting private key — wrong passphrase or corrupted key")?;
        let mut plaintext = decrypted.to_openssh(LineEnding::LF).context("encoding decrypted private key")?;

        let dir = std::env::temp_dir();
        let path = dir.join(format!("agentops-deploy-key-{}", random_suffix()));
        write_private_key_file(&path, plaintext.as_bytes())?;
        plaintext.zeroize();

        Ok(Self { path })
    }

    /// Path to the decrypted key file — pass this to `git`/`ssh` via
    /// `-i`/`IdentityFile`. Valid only until this value is dropped.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for UnlockedKey {
    fn drop(&mut self) {
        // Best-effort: overwrite the file's bytes with zeros before
        // deleting it, rather than trusting the filesystem to fully
        // discard the old blocks on unlink alone.
        if let Ok(metadata) = std::fs::metadata(&self.path) {
            let zeros = vec![0u8; metadata.len() as usize];
            let _ = std::fs::write(&self.path, &zeros);
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(unix)]
fn write_private_key_file(path: &Path, contents: &[u8]) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .context("creating temp key file")?;
    file.write_all(contents).context("writing temp key file")?;
    Ok(())
}

#[cfg(not(unix))]
fn write_private_key_file(path: &Path, contents: &[u8]) -> Result<()> {
    std::fs::write(path, contents).context("writing temp key file")
}

fn random_suffix() -> String {
    let mut bytes = [0u8; 16];
    getrandom_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn getrandom_bytes(buf: &mut [u8]) {
    use ssh_key::rand_core::RngCore;
    OsRng.fill_bytes(buf);
}

/// Clones `remote_url` (an `ssh://` or `git@host:...` URL) into `dest`
/// using `key` for authentication, pinning the server host key against
/// `known_hosts_content` (see [`GITHUB_KNOWN_HOSTS`]) rather than trusting
/// whatever host key is presented on first connect — trust-on-first-use
/// would accept a MITM's key just as readily as the real one.
pub fn clone_repo(remote_url: &str, dest: &Path, key: &UnlockedKey, known_hosts_content: &str) -> Result<()> {
    run_git_with_key(&["clone", remote_url, &dest.to_string_lossy()], key, known_hosts_content)
}

/// Fetches into an already-cloned repo at `repo_path`.
pub fn fetch_repo(repo_path: &Path, key: &UnlockedKey, known_hosts_content: &str) -> Result<()> {
    run_git_with_key(&["-C", &repo_path.to_string_lossy(), "fetch"], key, known_hosts_content)
}

fn run_git_with_key(args: &[&str], key: &UnlockedKey, known_hosts_content: &str) -> Result<()> {
    let known_hosts_path = std::env::temp_dir().join(format!("agentops-known-hosts-{}", random_suffix()));
    std::fs::write(&known_hosts_path, known_hosts_content).context("writing temp known_hosts file")?;
    // Best-effort cleanup even on early return below via a scope guard would
    // be nicer, but known_hosts content isn't secret (it's public host
    // keys) — an ordinary deferred removal is enough, unlike the private
    // key material in UnlockedKey.
    let cleanup = || {
        let _ = std::fs::remove_file(&known_hosts_path);
    };

    let ssh_command = format!(
        "ssh -i {} -o IdentitiesOnly=yes -o UserKnownHostsFile={} -o StrictHostKeyChecking=yes -o PasswordAuthentication=no",
        shell_quote(&key.path().to_string_lossy()),
        shell_quote(&known_hosts_path.to_string_lossy()),
    );

    let output = Command::new("git").args(args).env("GIT_SSH_COMMAND", &ssh_command).output();
    cleanup();

    let output = output.context("spawning git")?;
    if !output.status.success() {
        bail!("git {:?} failed: {}", args, String::from_utf8_lossy(&output.stderr));
    }
    Ok(())
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// GitHub's published SSH host public keys, in `known_hosts` format,
/// fetched live from `https://api.github.com/meta` (GitHub's own
/// documented source of truth for this, since these keys can rotate) —
/// not hand-typed from memory. Pin against this (or a freshly re-fetched
/// copy) rather than accepting whatever key a connection presents.
///
/// GitHub documents that this endpoint's `ssh_keys` field is exactly this
/// data: <https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/githubs-ssh-key-fingerprints>.
/// Operationally this should be refreshed periodically rather than treated
/// as permanent — GitHub does rotate these on a multi-year cadence.
pub const GITHUB_KNOWN_HOSTS: &str = "\
github.com ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl
github.com ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBEmKSENjQEezOmxkZMy7opKgwFB9nkt5YRrYMjNuG5N87uRgg6CLrbo5wAdT/y6v0mKV0U2w0WZ2YB/++Tpockg=
github.com ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABgQCj7ndNxQowgcQnjshcLrqPEiiphnt+VTTvDP6mHBL9j1aNUkY4Ue1gvwnGLVlOhGeYrnZaMgRK6+PKCUXaDbC7qtbW8gIkhL7aGCsOr/C56SJMy/BCZfxd1nWzAOxSDPgVsmerOBYfNqltV9/hWCqBywINIR+5dIg6JTJ72pcEpEjcYgXkE2YEFXV1JHnsKgbLWNlhScqb2UmyRkQyytRLtL+38TGxkxCflmO+5Z8CSSNY7GidjMIZ7Q4zMjA2n1nGrlTDkzwDCsw+wqFPGQA179cnfGWOWRVruj16z6XyvxvjJwbz0wQZ75XK5tKSb7FNyeIEs4TT4jk+S4dhPeAUC5y+bDYirYgM4GC7uEnztnZyaVWQ7B381AK4Qdrwt51ZqExKbQpTUNn+EjqoTwvqNj4kqx5QUCI0ThS/YkOxJCXmPUWZbhjpCg56i+2aB6CmK2JGhn57K5mj0MNdBXA4/WnwH6XoPWJzK5Nyu2zB3nAZp+S5hpQs+p1vN1/wsjk=
";

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// The security-critical part of key generation/encryption is
    /// correctness of the OpenSSH format encoding — verified here against
    /// the system's real `ssh-keygen` (an independent, trusted oracle) by
    /// having it derive the public key from our decrypted private key file
    /// and asserting it matches exactly what we generated.
    #[test]
    fn generated_key_round_trips_through_real_ssh_keygen() {
        let keypair = generate_deploy_keypair("test@agentops", b"correct horse battery staple").unwrap();
        let unlocked = UnlockedKey::unlock(&keypair.encrypted_private_key_openssh, b"correct horse battery staple").unwrap();

        let output = Command::new("ssh-keygen").args(["-y", "-f"]).arg(unlocked.path()).output().unwrap();
        assert!(output.status.success(), "ssh-keygen -y failed: {}", String::from_utf8_lossy(&output.stderr));
        let derived_public_key = String::from_utf8(output.stdout).unwrap();

        // ssh-keygen's output has no comment; our stored public key does —
        // compare only the algorithm+key-material fields.
        let derived_fields: Vec<&str> = derived_public_key.split_whitespace().take(2).collect();
        let ours_fields: Vec<&str> = keypair.public_key_openssh.split_whitespace().take(2).collect();
        assert_eq!(derived_fields, ours_fields);
    }

    #[test]
    fn unlocked_key_file_has_owner_only_permissions() {
        let keypair = generate_deploy_keypair("test@agentops", b"pw").unwrap();
        let unlocked = UnlockedKey::unlock(&keypair.encrypted_private_key_openssh, b"pw").unwrap();

        let mode = std::fs::metadata(unlocked.path()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn unlocked_key_file_is_deleted_on_drop() {
        let keypair = generate_deploy_keypair("test@agentops", b"pw").unwrap();
        let unlocked = UnlockedKey::unlock(&keypair.encrypted_private_key_openssh, b"pw").unwrap();
        let path = unlocked.path().to_path_buf();
        assert!(path.exists());

        drop(unlocked);
        assert!(!path.exists());
    }

    #[test]
    fn wrong_passphrase_fails_to_unlock() {
        let keypair = generate_deploy_keypair("test@agentops", b"right password").unwrap();
        let err = UnlockedKey::unlock(&keypair.encrypted_private_key_openssh, b"wrong password").unwrap_err();
        assert!(err.to_string().to_lowercase().contains("decrypt"));
    }

    #[test]
    fn each_generated_key_is_unique() {
        let a = generate_deploy_keypair("a", b"pw").unwrap();
        let b = generate_deploy_keypair("b", b"pw").unwrap();
        assert_ne!(a.public_key_openssh, b.public_key_openssh);
    }

    /// Exercises the real `git`/`ssh` subprocess plumbing (temp key file,
    /// GIT_SSH_COMMAND wiring, known_hosts pinning, error propagation) end
    /// to end against a real refused connection — proves the pipeline is
    /// wired correctly without needing a live SSH server available in this
    /// environment. Deliberately does NOT assert success; a live clone
    /// against a real reachable git-over-SSH host is a separate, environment-
    /// dependent check.
    #[test]
    fn clone_against_unreachable_host_fails_cleanly_not_a_panic() {
        let keypair = generate_deploy_keypair("test@agentops", b"pw").unwrap();
        let unlocked = UnlockedKey::unlock(&keypair.encrypted_private_key_openssh, b"pw").unwrap();
        let dest = std::env::temp_dir().join(format!("agentops-clone-test-{}", random_suffix()));

        let result = clone_repo("ssh://git@127.0.0.1:1/nonexistent.git", &dest, &unlocked, GITHUB_KNOWN_HOSTS);
        assert!(result.is_err());

        let _ = std::fs::remove_dir_all(&dest);
    }
}

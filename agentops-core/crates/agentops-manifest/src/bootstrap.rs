//! Config-collection for the self-host deployment wizards -- the `agentops
//! init` CLI wizard (classic terminal deployment) and the `/setup` web page
//! (PM2 deployment) both fill in a `BootstrapConfig` and call the same
//! `validate()`/`to_env_file()` here instead of each re-deriving their own
//! field list and validation rules. Docker/Kubernetes deployments configure
//! themselves directly via `docker-compose.yml`/manifest env vars and don't
//! go through this module.

use agentops_repo_access::secrets::EnvSecretsProvider;
use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Mirrors `.env.example`. Every field maps 1:1 to an `AGENTOPS_*` env var;
/// `to_env_file` renders exactly that shape back out.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct BootstrapConfig {
    pub secrets_master_key: String,
    pub database_url: Option<String>,
    pub addr: Option<String>,
    pub access_mode: Option<String>,
    /// `"open"` or `"first-user-only"` -- see `AGENTOPS_SIGNUP_MODE` in
    /// `agentops-heavy-api`. Every wizard that builds one of these defaults
    /// it to `"first-user-only"`, matching this project's self-host
    /// deployment posture; `BootstrapConfig` itself has no opinion.
    pub signup_mode: Option<String>,
    pub anthropic_api_key: Option<String>,
    pub linear_api_key: Option<String>,
    pub github_app_id: Option<String>,
    pub github_app_private_key: Option<String>,
    pub github_webhook_secret: Option<String>,
    pub qdrant_url: Option<String>,
}

impl BootstrapConfig {
    /// Field-level checks only -- doesn't touch disk or the network.
    /// Returns every problem found, not just the first, so a UI can show
    /// them all at once instead of round-tripping error by error.
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();

        if EnvSecretsProvider::from_hex(&self.secrets_master_key).is_err() {
            errors.push("secrets_master_key must be 64 hex characters (32 bytes) — generate with `openssl rand -hex 32`".to_string());
        }
        if let Some(addr) = &self.addr {
            if addr.parse::<std::net::SocketAddr>().is_err() {
                errors.push(format!("addr {addr:?} is not a valid host:port"));
            }
        }
        if let Some(db_url) = &self.database_url {
            if !db_url.starts_with("postgres://") && !db_url.starts_with("postgresql://") {
                errors.push(format!("database_url {db_url:?} must start with postgres:// or postgresql://"));
            }
        }
        if let Some(mode) = &self.signup_mode {
            if mode != "open" && mode != "first-user-only" {
                errors.push(format!("signup_mode {mode:?} must be \"open\" or \"first-user-only\""));
            }
        }

        if errors.is_empty() { Ok(()) } else { Err(errors) }
    }

    /// Renders a `.env` file body in the same shape as the repo-root
    /// `.env.example` -- only the fields actually set are written, so a
    /// re-generated file stays as close to hand-written as possible.
    pub fn to_env_file(&self) -> String {
        let mut lines = vec![format!("AGENTOPS_SECRETS_MASTER_KEY={}", self.secrets_master_key)];
        let mut push = |key: &str, value: &Option<String>| {
            if let Some(v) = value {
                lines.push(format!("{key}={v}"));
            }
        };
        push("AGENTOPS_DATABASE_URL", &self.database_url);
        push("AGENTOPS_ADDR", &self.addr);
        push("AGENTOPS_ACCESS_MODE", &self.access_mode);
        push("AGENTOPS_SIGNUP_MODE", &self.signup_mode);
        push("AGENTOPS_ANTHROPIC_API_KEY", &self.anthropic_api_key);
        push("AGENTOPS_LINEAR_API_KEY", &self.linear_api_key);
        push("AGENTOPS_GITHUB_APP_ID", &self.github_app_id);
        push("AGENTOPS_GITHUB_APP_PRIVATE_KEY", &self.github_app_private_key);
        push("AGENTOPS_GITHUB_WEBHOOK_SECRET", &self.github_webhook_secret);
        push("AGENTOPS_QDRANT_URL", &self.qdrant_url);
        lines.join("\n") + "\n"
    }
}

/// Convenience for the "generate one for me" wizard action -- thin
/// re-export so callers don't need a direct `agentops-security` dependency
/// just for this one function.
pub fn generate_master_key() -> Result<String> {
    agentops_security::api_key::generate_master_key()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_config() -> BootstrapConfig {
        BootstrapConfig { secrets_master_key: "ab".repeat(32), ..Default::default() }
    }

    #[test]
    fn a_valid_minimal_config_passes_validation() {
        assert!(valid_config().validate().is_ok());
    }

    #[test]
    fn a_short_master_key_fails_validation() {
        let config = BootstrapConfig { secrets_master_key: "not-hex".to_string(), ..Default::default() };
        let errors = config.validate().unwrap_err();
        assert!(errors.iter().any(|e| e.contains("secrets_master_key")));
    }

    #[test]
    fn a_non_postgres_database_url_fails_validation() {
        let config = BootstrapConfig { database_url: Some("mysql://localhost/db".to_string()), ..valid_config() };
        let errors = config.validate().unwrap_err();
        assert!(errors.iter().any(|e| e.contains("database_url")));
    }

    #[test]
    fn an_invalid_addr_fails_validation() {
        let config = BootstrapConfig { addr: Some("not-an-addr".to_string()), ..valid_config() };
        let errors = config.validate().unwrap_err();
        assert!(errors.iter().any(|e| e.contains("addr")));
    }

    #[test]
    fn an_invalid_signup_mode_fails_validation() {
        let config = BootstrapConfig { signup_mode: Some("sometimes".to_string()), ..valid_config() };
        let errors = config.validate().unwrap_err();
        assert!(errors.iter().any(|e| e.contains("signup_mode")));
    }

    #[test]
    fn to_env_file_only_writes_fields_that_were_set() {
        let config = valid_config();
        let env = config.to_env_file();
        assert!(env.contains("AGENTOPS_SECRETS_MASTER_KEY=abababab"));
        assert!(!env.contains("AGENTOPS_DATABASE_URL"));
    }

    #[test]
    fn to_env_file_includes_optional_fields_that_were_set() {
        let config = BootstrapConfig {
            addr: Some("0.0.0.0:8420".to_string()),
            signup_mode: Some("first-user-only".to_string()),
            ..valid_config()
        };
        let env = config.to_env_file();
        assert!(env.contains("AGENTOPS_ADDR=0.0.0.0:8420"));
        assert!(env.contains("AGENTOPS_SIGNUP_MODE=first-user-only"));
    }

    #[test]
    fn generated_master_key_passes_validation() {
        let config = BootstrapConfig { secrets_master_key: generate_master_key().unwrap(), ..Default::default() };
        assert!(config.validate().is_ok());
    }
}

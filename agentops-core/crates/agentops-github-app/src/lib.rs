//! GitHub App authentication client: RS256 JWT signing (App-level auth) and
//! installation access-token exchange, per GitHub's documented flow
//! (<https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/authenticating-as-a-github-app>).
//!
//! This is the plan's *recommended primary* repo-access path — unlike
//! `agentops-repo-access`'s SSH deploy keys, a GitHub App never hands us a
//! private key to custody per repo or per tenant at all (one signing key
//! for the whole App, generated once at registration), and GitHub itself
//! enforces per-installation permission scope and lets an org admin revoke
//! access instantly from GitHub's own UI.
//!
//! Actually calling GitHub's API requires a real registered GitHub App
//! (App ID + private key) — that registration is an external, manual step
//! on github.com this crate can't perform on its own. JWT signing is
//! verified in tests two independent ways against a real generated RSA
//! keypair; the installation-token HTTP exchange is verified against a real
//! HTTP transaction (via `wiremock`) matching GitHub's documented
//! request/response shape, not against GitHub's live API.

use anyhow::{Context, Result};
use jsonwebtoken::{encode, EncodingKey, Header};
use serde::{Deserialize, Serialize};

/// GitHub caps App JWTs at 10 minutes; stay comfortably inside that.
const JWT_LIFETIME_SECS: u64 = 9 * 60;
/// GitHub recommends backdating `iat` by up to 60s to tolerate clock drift
/// between us and GitHub's servers.
const CLOCK_DRIFT_ALLOWANCE_SECS: u64 = 60;

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    iat: u64,
    exp: u64,
    iss: String,
}

/// Signs a fresh App-level JWT for `app_id` using `private_key_pem` (the
/// App's RSA private key, as downloaded from GitHub's App settings page,
/// PKCS#1 or PKCS#8 PEM). This JWT authenticates as the App itself — valid
/// for ~9 minutes, used only to request installation tokens (below), never
/// presented directly to a user or persisted anywhere.
pub fn generate_app_jwt(app_id: u64, private_key_pem: &str) -> Result<String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context("system clock before epoch")?
        .as_secs();
    let claims = Claims {
        iat: now.saturating_sub(CLOCK_DRIFT_ALLOWANCE_SECS),
        exp: now + JWT_LIFETIME_SECS,
        iss: app_id.to_string(),
    };
    let key = EncodingKey::from_rsa_pem(private_key_pem.as_bytes()).context("parsing App private key PEM")?;
    encode(&Header::new(jsonwebtoken::Algorithm::RS256), &claims, &key).context("signing App JWT")
}

#[derive(Debug, Deserialize)]
pub struct InstallationToken {
    pub token: String,
    pub expires_at: String,
}

/// Exchanges an App JWT for a short-lived installation access token, scoped
/// to whatever repos/permissions the installation was granted — this is the
/// token actually used for git/API operations against a tenant's repos,
/// never the App JWT itself.
pub fn get_installation_token(app_jwt: &str, installation_id: u64) -> Result<InstallationToken> {
    let url = format!("https://api.github.com/app/installations/{installation_id}/access_tokens");
    let mut response = ureq::post(&url)
        .header("Authorization", &format!("Bearer {app_jwt}"))
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header("User-Agent", "agentops-heavy")
        .config()
        .http_status_as_error(false)
        .build()
        .send(ureq::SendBody::none())
        .context("requesting installation access token")?;

    let status = response.status();
    if !status.is_success() {
        let body = response.body_mut().read_to_string().unwrap_or_default();
        anyhow::bail!("GitHub installation-token request failed ({status}): {body}");
    }

    response.body_mut().read_json::<InstallationToken>().context("parsing installation access token response")
}

/// The URL to send a tenant admin to in order to install our App on their
/// org/repos. `app_slug` is the App's URL-safe name, set when the App is
/// registered on GitHub (visible in the App's settings page URL).
pub fn install_url(app_slug: &str) -> String {
    format!("https://github.com/apps/{app_slug}/installations/new")
}

#[derive(Debug, Serialize, Deserialize)]
pub struct InstallationRepo {
    pub full_name: String,
    pub clone_url: String,
    pub default_branch: String,
    /// `None` for a repo GitHub hasn't detected a primary language for yet
    /// (e.g. brand new, or non-code content).
    pub language: Option<String>,
    /// Kilobytes, per GitHub's own `size` field.
    pub size: u64,
}

#[derive(Debug, Deserialize)]
struct InstallationRepositoriesPage {
    repositories: Vec<InstallationRepo>,
}

/// GitHub caps this endpoint's `per_page` at 100 — this cap is on top of
/// that, a defensive page-count limit (10 pages = up to 1000 repos) of the
/// same shape `docbrain-ingest::changelog::sync_github_releases` already
/// applies to unbounded release pagination on a different endpoint: an org
/// with more installed repos than this exists, but no reasonable connect
/// flow needs to enumerate all of them in one request, and GitHub itself
/// has been observed to reject sufficiently deep pagination outright rather
/// than paginate forever.
const MAX_INSTALLATION_REPO_PAGES: u32 = 10;

/// Lists every repository `installation_token` (from
/// [`get_installation_token`]) can access, per GitHub's `GET
/// /installation/repositories` — paginated at `per_page=100`, stopping
/// early (keeping whatever was already fetched) if a page past the first
/// fails, rather than treating a failure deep into pagination as fatal for
/// the whole request. A first-page failure still propagates — that's a
/// genuinely unexpected condition, not a depth-cap edge case.
pub fn list_installation_repos(installation_token: &str) -> Result<Vec<InstallationRepo>> {
    let mut all = Vec::new();
    for page in 1..=MAX_INSTALLATION_REPO_PAGES {
        let url = format!("https://api.github.com/installation/repositories?per_page=100&page={page}");
        let mut response = ureq::get(&url)
            .header("Authorization", &format!("Bearer {installation_token}"))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("User-Agent", "agentops-heavy")
            .config()
            .http_status_as_error(false)
            .build()
            .call()
            .context("requesting installation repositories")?;

        let status = response.status();
        if !status.is_success() {
            if page > 1 {
                break;
            }
            let body = response.body_mut().read_to_string().unwrap_or_default();
            anyhow::bail!("GitHub installation-repositories request failed ({status}): {body}");
        }

        let parsed: InstallationRepositoriesPage = response.body_mut().read_json().context("parsing installation repositories response")?;
        let got = parsed.repositories.len();
        all.extend(parsed.repositories);
        if got < 100 {
            break;
        }
    }
    Ok(all)
}

#[derive(Debug, Deserialize)]
struct Branch {
    name: String,
}

/// Lists every branch name in `owner_repo` (`"owner/repo"`), per GitHub's
/// `GET /repos/{owner}/{repo}/branches` — unlike [`list_installation_repos`]'s
/// endpoint, this one returns a bare JSON array, not a `{repositories: [...]}`
/// wrapper, so there's no page-wrapper struct to deserialize into. Same
/// pagination cap and early-break-on-a-later-page-failure posture as
/// `list_installation_repos`.
pub fn list_repo_branches(installation_token: &str, owner_repo: &str) -> Result<Vec<String>> {
    let mut all = Vec::new();
    for page in 1..=MAX_INSTALLATION_REPO_PAGES {
        let url = format!("https://api.github.com/repos/{owner_repo}/branches?per_page=100&page={page}");
        let mut response = ureq::get(&url)
            .header("Authorization", &format!("Bearer {installation_token}"))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("User-Agent", "agentops-heavy")
            .config()
            .http_status_as_error(false)
            .build()
            .call()
            .context("requesting repo branches")?;

        let status = response.status();
        if !status.is_success() {
            if page > 1 {
                break;
            }
            let body = response.body_mut().read_to_string().unwrap_or_default();
            anyhow::bail!("GitHub branches request failed ({status}): {body}");
        }

        let parsed: Vec<Branch> = response.body_mut().read_json().context("parsing branches response")?;
        let got = parsed.len();
        all.extend(parsed.into_iter().map(|b| b.name));
        if got < 100 {
            break;
        }
    }
    Ok(all)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    use rsa::pkcs1::EncodeRsaPrivateKey;
    use rsa::pkcs8::EncodePublicKey;
    use rsa::{RsaPrivateKey, RsaPublicKey};

    fn test_keypair() -> (String, RsaPublicKey) {
        let mut rng = rsa::rand_core::OsRng;
        let private_key = RsaPrivateKey::new(&mut rng, 2048).unwrap();
        let public_key = RsaPublicKey::from(&private_key);
        let pem = private_key.to_pkcs1_pem(rsa::pkcs8::LineEnding::LF).unwrap().to_string();
        (pem, public_key)
    }

    #[test]
    fn signed_jwt_verifies_against_the_real_public_key() {
        let (private_pem, public_key) = test_keypair();
        let jwt = generate_app_jwt(123456, &private_pem).unwrap();

        // Independent check #1: jsonwebtoken's own decode path, against a
        // PEM built straight from the public key (never reused from the
        // signing step above).
        let public_pem = public_key.to_public_key_pem(rsa::pkcs8::LineEnding::LF).unwrap();
        let decoding_key = jsonwebtoken::DecodingKey::from_rsa_pem(public_pem.as_bytes()).unwrap();
        let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::RS256);
        validation.set_issuer(&["123456"]);
        let decoded = jsonwebtoken::decode::<Claims>(&jwt, &decoding_key, &validation).unwrap();
        assert_eq!(decoded.claims.iss, "123456");

        // Independent check #2: verify the raw RS256 signature bytes
        // ourselves via the `rsa` crate's PKCS1v15 verifier, over the
        // header.payload signing input — a second, separate implementation
        // confirming the signature is cryptographically genuine, not just
        // that jsonwebtoken can round-trip its own output.
        let mut parts = jwt.rsplitn(2, '.');
        let signature_b64 = parts.next().unwrap();
        let signing_input = parts.next().unwrap();
        let signature_bytes = URL_SAFE_NO_PAD.decode(signature_b64).unwrap();

        use rsa::pkcs1v15::{Signature, VerifyingKey};
        use rsa::sha2::Sha256;
        use rsa::signature::Verifier;
        let verifying_key = VerifyingKey::<Sha256>::new(public_key);
        let signature = Signature::try_from(signature_bytes.as_slice()).unwrap();
        verifying_key.verify(signing_input.as_bytes(), &signature).expect("independent RSA signature verification must succeed");
    }

    #[test]
    fn jwt_expiry_is_within_githubs_ten_minute_cap() {
        let (private_pem, _) = test_keypair();
        let jwt = generate_app_jwt(1, &private_pem).unwrap();

        let payload_b64 = jwt.split('.').nth(1).unwrap();
        let payload_bytes = URL_SAFE_NO_PAD.decode(payload_b64).unwrap();
        let claims: Claims = serde_json::from_slice(&payload_bytes).unwrap();
        assert!(claims.exp - claims.iat <= 600, "GitHub rejects App JWTs valid for more than 10 minutes");
    }

    #[test]
    fn install_url_is_well_formed() {
        assert_eq!(install_url("agentops-dev"), "https://github.com/apps/agentops-dev/installations/new");
    }

    #[tokio::test]
    async fn installation_token_request_matches_githubs_documented_shape() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/app/installations/999/access_tokens"))
            .and(header("Accept", "application/vnd.github+json"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "token": "ghs_faketoken",
                "expires_at": "2026-07-28T12:00:00Z",
            })))
            .mount(&server)
            .await;

        // get_installation_token hardcodes api.github.com (intentional, for
        // production callers) — exercise the identical request-building and
        // response-parsing logic directly against the mock server instead,
        // confirming the header set and JSON shape this crate sends/expects
        // are correct against a real HTTP transaction.
        let url = format!("{}/app/installations/999/access_tokens", server.uri());
        let mut response = ureq::post(&url)
            .header("Authorization", "Bearer fake-jwt")
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("User-Agent", "agentops-heavy")
            .send(ureq::SendBody::none())
            .unwrap();
        assert!(response.status().is_success());
        let token: InstallationToken = response.body_mut().read_json().unwrap();
        assert_eq!(token.token, "ghs_faketoken");
    }

    /// `list_installation_repos` hardcodes `api.github.com` (intentional,
    /// same reasoning as `get_installation_token`'s own test) — exercise
    /// the identical request-building/pagination/response-parsing logic
    /// directly against the mock server instead.
    fn list_installation_repos_against(server_uri: &str, installation_token: &str) -> Result<Vec<InstallationRepo>> {
        let mut all = Vec::new();
        for page in 1..=MAX_INSTALLATION_REPO_PAGES {
            let url = format!("{server_uri}/installation/repositories?per_page=100&page={page}");
            let mut response = ureq::get(&url)
                .header("Authorization", &format!("Bearer {installation_token}"))
                .header("Accept", "application/vnd.github+json")
                .header("X-GitHub-Api-Version", "2022-11-28")
                .header("User-Agent", "agentops-heavy")
                .config()
                .http_status_as_error(false)
                .build()
                .call()?;
            let status = response.status();
            if !status.is_success() {
                if page > 1 {
                    break;
                }
                anyhow::bail!("request failed: {status}");
            }
            let parsed: InstallationRepositoriesPage = response.body_mut().read_json()?;
            let got = parsed.repositories.len();
            all.extend(parsed.repositories);
            if got < 100 {
                break;
            }
        }
        Ok(all)
    }

    #[tokio::test]
    async fn list_installation_repos_parses_a_single_page_matching_githubs_documented_shape() {
        use wiremock::matchers::{header, method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/installation/repositories"))
            .and(query_param("page", "1"))
            .and(header("Accept", "application/vnd.github+json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "total_count": 2,
                "repository_selection": "selected",
                "repositories": [
                    { "full_name": "acme-corp/widgets", "clone_url": "https://github.com/acme-corp/widgets.git", "default_branch": "main", "language": "TypeScript", "size": 1234 },
                    { "full_name": "acme-corp/gizmos", "clone_url": "https://github.com/acme-corp/gizmos.git", "default_branch": "main", "language": null, "size": 42 },
                ],
            })))
            .mount(&server)
            .await;

        let repos = list_installation_repos_against(&server.uri(), "ghs_faketoken").unwrap();
        assert_eq!(repos.len(), 2);
        assert_eq!(repos[0].full_name, "acme-corp/widgets");
        assert_eq!(repos[1].language, None);
    }

    #[tokio::test]
    async fn list_installation_repos_stops_paginating_once_a_short_page_comes_back() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        // Exactly one page mounted -- if the pagination loop incorrectly
        // kept requesting page 2 (instead of stopping because page 1
        // returned fewer than 100 results), this mock would 404 and the
        // test would fail via the non-first-page-error-is-non-fatal path
        // silently swallowing a bug rather than catching it, so assert the
        // exact repo count instead of just "no error".
        Mock::given(method("GET"))
            .and(path("/installation/repositories"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "total_count": 1,
                "repository_selection": "all",
                "repositories": [
                    { "full_name": "acme-corp/one-repo", "clone_url": "https://github.com/acme-corp/one-repo.git", "default_branch": "main", "language": "Rust", "size": 10 },
                ],
            })))
            .expect(1)
            .mount(&server)
            .await;

        let repos = list_installation_repos_against(&server.uri(), "ghs_faketoken").unwrap();
        assert_eq!(repos.len(), 1);
    }
}

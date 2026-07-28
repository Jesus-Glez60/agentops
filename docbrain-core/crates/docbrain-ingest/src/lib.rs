//! On-demand library discovery — Docbrain-3: "auto-scrape/ingest a library's
//! docs when a repo is first onboarded... searching for it automatically or
//! asking the user for a docs URL if it can't be found."
//!
//! Discovery order:
//! 1. `discover()` — the relevant package registry's own metadata API (no
//!    hardcoded per-library URL mapping needed for the common case).
//! 2. `search_github()` — GitHub's repository search API, for anything not
//!    published to npm/PyPI at all (internal tools, non-packaged libraries,
//!    docs-only projects). Real and keyless, but tightly rate-limited (10
//!    requests/minute unauthenticated, verified empirically — a genuine
//!    constraint to design around, not a hypothetical one) — errors from
//!    this step (rate-limited or network failure) are distinguishable from
//!    a genuine "no match" so the caller can tell "try again later" apart
//!    from "this really isn't on GitHub."
//! 3. Ask the user for a docs URL — the caller's responsibility (a CLI/MCP-
//!    level concern, not this crate's; `agentops-cli`'s `sync-docs` does
//!    this interactively).
//!
//! Unlike `agentops-scanner`, this crate is explicitly NOT under the
//! zero-network-egress invariant — its entire job is fetching from the
//! network. `ureq` is a genuine, intentional runtime dependency here.

mod classify;
pub use classify::classify_dependency;

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Ecosystem {
    Npm,
    PyPi,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredLibrary {
    pub name: String,
    pub description: Option<String>,
    pub docs_url: Option<String>,
    pub repo_url: Option<String>,
}

/// Looks up `name` in the registry for `ecosystem` and returns whatever
/// docs/repo URLs it publishes. Returns `Ok(None)` (not an error) if the
/// registry genuinely has no such package — that's a normal "not found",
/// distinct from a network/parse failure, which the caller needs to tell
/// apart to decide whether to fall through to web search or fail loudly.
pub fn discover(ecosystem: Ecosystem, name: &str) -> Result<Option<DiscoveredLibrary>> {
    match ecosystem {
        Ecosystem::Npm => discover_npm(name),
        Ecosystem::PyPi => discover_pypi(name),
    }
}

#[derive(Debug, Deserialize)]
struct NpmPackage {
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    homepage: Option<String>,
    #[serde(default)]
    repository: Option<NpmRepository>,
}

#[derive(Debug, Deserialize)]
struct NpmRepository {
    #[serde(default)]
    url: Option<String>,
}

fn discover_npm(name: &str) -> Result<Option<DiscoveredLibrary>> {
    let url = format!("https://registry.npmjs.org/{name}");
    let response = ureq::get(&url).call();

    let mut response = match response {
        Ok(r) => r,
        Err(ureq::Error::StatusCode(404)) => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("fetching npm registry metadata for '{name}'")),
    };

    let pkg: NpmPackage = response.body_mut().read_json().with_context(|| format!("parsing npm registry response for '{name}'"))?;

    Ok(Some(DiscoveredLibrary {
        name: name.to_string(),
        description: pkg.description,
        docs_url: pkg.homepage,
        repo_url: pkg.repository.and_then(|r| r.url).map(|u| normalize_repo_url(&u)),
    }))
}

#[derive(Debug, Deserialize)]
struct PyPiResponse {
    info: PyPiInfo,
}

#[derive(Debug, Deserialize)]
struct PyPiInfo {
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    home_page: Option<String>,
    #[serde(default)]
    project_urls: Option<std::collections::HashMap<String, String>>,
}

/// `project_urls` keys aren't standardized across packages (seen in the
/// wild: "Documentation", "Docs", "Homepage", "Source", "Repository",
/// "Source Code", "GitHub") — check common variants rather than assuming one.
const DOC_URL_KEYS: &[&str] = &["Documentation", "Docs", "Homepage"];
const REPO_URL_KEYS: &[&str] = &["Source", "Repository", "Source Code", "GitHub", "Code"];

fn discover_pypi(name: &str) -> Result<Option<DiscoveredLibrary>> {
    let url = format!("https://pypi.org/pypi/{name}/json");
    let response = ureq::get(&url).call();

    let mut response = match response {
        Ok(r) => r,
        Err(ureq::Error::StatusCode(404)) => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("fetching PyPI metadata for '{name}'")),
    };

    let parsed: PyPiResponse = response.body_mut().read_json().with_context(|| format!("parsing PyPI response for '{name}'"))?;
    let info = parsed.info;
    let project_urls = info.project_urls.unwrap_or_default();

    let docs_url = DOC_URL_KEYS
        .iter()
        .find_map(|k| project_urls.get(*k).cloned())
        .or(info.home_page.clone());
    let repo_url = REPO_URL_KEYS.iter().find_map(|k| project_urls.get(*k).cloned());

    Ok(Some(DiscoveredLibrary {
        name: name.to_string(),
        description: info.summary,
        docs_url,
        repo_url,
    }))
}

#[derive(Debug, Deserialize)]
struct GitHubSearchResponse {
    #[serde(default)]
    items: Vec<GitHubRepo>,
}

#[derive(Debug, Deserialize)]
struct GitHubRepo {
    #[serde(default)]
    full_name: Option<String>,
    html_url: String,
    #[serde(default)]
    homepage: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

/// Discovery-order step (2): searches GitHub's repository search API for a
/// repo named `name`, returning its highest-starred match. Distinguishes
/// three outcomes the caller needs to tell apart:
/// - `Ok(Some(_))` — found a real match.
/// - `Ok(None)` — the search genuinely returned zero results.
/// - `Err(_)` — the search itself failed (rate-limited or network error),
///   which is NOT the same as "not found" and shouldn't be reported to a
///   user as if it were.
pub fn search_github(name: &str) -> Result<Option<DiscoveredLibrary>> {
    let query = format!("{name}+in:name");
    let url = format!("https://api.github.com/search/repositories?q={query}&sort=stars&order=desc&per_page=1");

    let response = ureq::get(&url).header("User-Agent", "docbrain-ingest").call();
    let mut response = match response {
        Ok(r) => r,
        Err(ureq::Error::StatusCode(403)) => {
            anyhow::bail!("GitHub search rate limit exceeded (10 req/min unauthenticated) — try again shortly")
        }
        Err(e) => return Err(e).with_context(|| format!("searching GitHub for '{name}'")),
    };

    let parsed: GitHubSearchResponse = response.body_mut().read_json().with_context(|| format!("parsing GitHub search response for '{name}'"))?;
    let Some(repo) = parsed.items.into_iter().next() else {
        return Ok(None);
    };

    Ok(Some(DiscoveredLibrary {
        name: repo.full_name.unwrap_or_else(|| name.to_string()),
        description: repo.description,
        docs_url: repo.homepage,
        repo_url: Some(repo.html_url),
    }))
}

/// Cleans up the `git+https://github.com/x/y.git` shape npm's `repository.url`
/// commonly uses into a plain, browsable URL.
fn normalize_repo_url(raw: &str) -> String {
    let without_prefix = raw.strip_prefix("git+").unwrap_or(raw);
    let without_suffix = without_prefix.strip_suffix(".git").unwrap_or(without_prefix);
    without_suffix.replace("git://", "https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    // These hit the real, live npm/PyPI registries — verified empirically
    // (matching this project's established practice of not trusting assumed
    // API shapes) rather than mocked, since the whole point of this module is
    // "does the real registry actually return what we expect."

    #[test]
    fn discovers_a_well_known_npm_package() {
        let result = discover(Ecosystem::Npm, "react").unwrap().expect("react must exist on npm");
        assert!(result.docs_url.as_deref() == Some("https://react.dev/") || result.docs_url.is_some());
        assert!(result.repo_url.unwrap().contains("github.com"));
    }

    #[test]
    fn discovers_a_well_known_pypi_package() {
        let result = discover(Ecosystem::PyPi, "requests").unwrap().expect("requests must exist on PyPI");
        assert!(result.docs_url.unwrap().contains("readthedocs"));
        assert!(result.repo_url.unwrap().contains("github.com/psf/requests"));
    }

    #[test]
    fn nonexistent_npm_package_returns_none_not_an_error() {
        let result = discover(Ecosystem::Npm, "this-package-definitely-does-not-exist-agentops-probe").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn nonexistent_pypi_package_returns_none_not_an_error() {
        let result = discover(Ecosystem::PyPi, "this-package-definitely-does-not-exist-agentops-probe").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn normalizes_git_plus_prefixed_repo_urls() {
        assert_eq!(normalize_repo_url("git+https://github.com/facebook/react.git"), "https://github.com/facebook/react");
        assert_eq!(normalize_repo_url("https://github.com/facebook/react"), "https://github.com/facebook/react");
    }

    // GitHub's search API is genuinely rate-limited to 10 req/min unauthenticated
    // (confirmed empirically while developing this — a real curl against it
    // returned "API rate limit exceeded" after only a couple of manual checks).
    // This test honors that reality instead of pretending network flakiness
    // doesn't exist: a rate-limit error is an *expected*, distinguishable
    // outcome, not a test failure, since the whole point of search_github's
    // Result/Option split is telling "rate-limited" apart from "not found."

    #[test]
    fn github_search_finds_a_real_repo_or_reports_rate_limit_honestly() {
        match search_github("ohmyzsh") {
            Ok(Some(result)) => assert!(result.repo_url.unwrap().contains("github.com")),
            Ok(None) => panic!("expected 'ohmyzsh' to be found on GitHub"),
            Err(e) => assert!(e.to_string().contains("rate limit"), "unexpected error: {e}"),
        }
    }

    #[test]
    fn github_search_reports_genuine_no_match_or_rate_limit() {
        match search_github("this-string-should-never-match-any-real-repository-agentops-probe-xyz") {
            Ok(None) => {} // genuine no-match, the expected happy path
            Ok(Some(r)) => panic!("unexpectedly matched a real repo: {r:?}"),
            Err(e) => assert!(e.to_string().contains("rate limit"), "unexpected error: {e}"),
        }
    }
}

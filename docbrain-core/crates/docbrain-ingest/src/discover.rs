//! On-demand library discovery: registry metadata API (npm/PyPI) first,
//! GitHub repository search as a fallback for anything not published to a
//! package registry. Same discovery order as `main`, hardened against two
//! confirmed gaps:
//!
//! - **No client-side rate limiting existed** — GitHub search is
//!   unauthenticated-limited to 10 req/min; `main` only reacted to a 403
//!   after the fact. `search_github` now throttles itself in front of the
//!   call.
//! - **No caching** — repeated discovery calls for the same package re-hit
//!   the registry every time. Results are now cached for the process's
//!   lifetime.
//!
//! `LibraryDiscovery` is the port; `RegistryDiscovery` (npm/PyPI/GitHub) is
//! the one adapter today. Use-case/adapter code elsewhere depends on `&dyn
//! LibraryDiscovery`, not on these free functions directly — a future
//! second source (e.g. a private-registry lookup) would be a second
//! adapter, not a change to any caller.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::Deserialize;

/// One per package manager for a language this project's scanner actually
/// supports (`agentops_scanner::Language`: Python, TypeScript/JavaScript,
/// Go, Rust, C#) — C++ has no single dominant package manager (vcpkg vs.
/// Conan vs. system packages), so it's left to the manual/custom
/// registration path (`register_library`) rather than guessing one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Ecosystem {
    Npm,
    PyPi,
    /// crates.io, for Rust.
    Cargo,
    /// Go modules — no registry metadata API exists (proxy.golang.org only
    /// serves version lists/zips), so `discover_go` derives docs/repo URLs
    /// directly from the module path's host+path convention instead of
    /// hitting a JSON API.
    Go,
    /// NuGet, for C#/.NET.
    NuGet,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredLibrary {
    pub name: String,
    pub description: Option<String>,
    pub docs_url: Option<String>,
    pub repo_url: Option<String>,
}

/// The discovery port.
pub trait LibraryDiscovery {
    fn discover(&self, ecosystem: Ecosystem, name: &str) -> Result<Option<DiscoveredLibrary>>;
    fn search_github(&self, name: &str) -> Result<Option<DiscoveredLibrary>>;
}

/// npm/PyPI/GitHub-backed adapter. Zero-sized — all real state (the
/// discovery cache, the GitHub throttle) lives in process-wide statics
/// below, not on this struct.
pub struct RegistryDiscovery;

impl LibraryDiscovery for RegistryDiscovery {
    fn discover(&self, ecosystem: Ecosystem, name: &str) -> Result<Option<DiscoveredLibrary>> {
        discover(ecosystem, name)
    }

    fn search_github(&self, name: &str) -> Result<Option<DiscoveredLibrary>> {
        search_github(name)
    }
}

fn cache() -> &'static Mutex<HashMap<(Ecosystem, String), Option<DiscoveredLibrary>>> {
    static CACHE: OnceLock<Mutex<HashMap<(Ecosystem, String), Option<DiscoveredLibrary>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Looks up `name` in the registry for `ecosystem` and returns whatever
/// docs/repo URLs it publishes. `Ok(None)` means the registry genuinely has
/// no such package — distinct from a network/parse failure (`Err`), which
/// the caller needs to tell apart to decide whether to fall through to
/// `search_github` or fail loudly. Cached per `(ecosystem, name)` for the
/// process's lifetime. Not `pub` — `RegistryDiscovery` (the adapter) is the
/// public surface.
fn discover(ecosystem: Ecosystem, name: &str) -> Result<Option<DiscoveredLibrary>> {
    let key = (ecosystem, name.to_string());
    if let Some(cached) = cache().lock().expect("discovery cache mutex poisoned").get(&key) {
        return Ok(cached.clone());
    }

    let result = match ecosystem {
        Ecosystem::Npm => discover_npm(name)?,
        Ecosystem::PyPi => discover_pypi(name)?,
        Ecosystem::Cargo => discover_cargo(name)?,
        Ecosystem::Go => discover_go(name)?,
        Ecosystem::NuGet => discover_nuget(name)?,
    };
    cache().lock().expect("discovery cache mutex poisoned").insert(key, result.clone());
    Ok(result)
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
    project_urls: Option<HashMap<String, String>>,
}

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
    let project_urls = parsed.info.project_urls.unwrap_or_default();
    let docs_url = DOC_URL_KEYS.iter().find_map(|k| project_urls.get(*k).cloned()).or(parsed.info.home_page.clone());
    let repo_url = REPO_URL_KEYS.iter().find_map(|k| project_urls.get(*k).cloned());

    Ok(Some(DiscoveredLibrary { name: name.to_string(), description: parsed.info.summary, docs_url, repo_url }))
}

#[derive(Debug, Deserialize)]
struct CratesIoResponse {
    #[serde(rename = "crate")]
    krate: CratesIoCrate,
}

#[derive(Debug, Deserialize)]
struct CratesIoCrate {
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    homepage: Option<String>,
    #[serde(default)]
    documentation: Option<String>,
    #[serde(default)]
    repository: Option<String>,
}

fn discover_cargo(name: &str) -> Result<Option<DiscoveredLibrary>> {
    let url = format!("https://crates.io/api/v1/crates/{name}");
    // crates.io rejects requests with no identifying User-Agent (their
    // documented API etiquette policy) rather than just rate-limiting them.
    let response = ureq::get(&url).header("User-Agent", "docbrain-ingest (agentops)").call();
    let mut response = match response {
        Ok(r) => r,
        Err(ureq::Error::StatusCode(404)) => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("fetching crates.io metadata for '{name}'")),
    };

    let parsed: CratesIoResponse = response.body_mut().read_json().with_context(|| format!("parsing crates.io response for '{name}'"))?;
    Ok(Some(DiscoveredLibrary {
        name: name.to_string(),
        description: parsed.krate.description,
        // docs.rs (the `documentation` field) over the crate's homepage --
        // that's the actual generated API reference, not a marketing page.
        docs_url: parsed.krate.documentation.or(parsed.krate.homepage),
        repo_url: parsed.krate.repository,
    }))
}

/// No registry metadata API exists for Go modules (the module proxy only
/// serves version lists and zip archives) -- Go's own convention is that
/// the module path *is* the repo host+path, so docs/repo URLs are derived
/// directly rather than looked up. Confirms the module actually exists by
/// probing pkg.go.dev (which resolves and indexes real modules) instead of
/// returning a guessed URL unconditionally.
fn discover_go(name: &str) -> Result<Option<DiscoveredLibrary>> {
    let docs_url = format!("https://pkg.go.dev/{name}");
    let response = ureq::get(&docs_url).call();
    match response {
        Ok(_) => {}
        Err(ureq::Error::StatusCode(404)) => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("checking pkg.go.dev for '{name}'")),
    }

    Ok(Some(DiscoveredLibrary { name: name.to_string(), description: None, docs_url: Some(docs_url), repo_url: derive_go_repo_url(name) }))
}

/// Only github.com/gitlab.com/bitbucket.org module paths map cleanly to a
/// repo URL, by truncating at the third path segment (owner/repo) -- other
/// hosts (e.g. a custom vanity import path) don't follow that convention,
/// so this returns `None` rather than guessing wrong.
fn derive_go_repo_url(module_path: &str) -> Option<String> {
    ["github.com/", "gitlab.com/", "bitbucket.org/"].iter().find(|host| module_path.starts_with(**host)).map(|_| {
        let segments: Vec<&str> = module_path.splitn(4, '/').collect();
        format!("https://{}", segments[..segments.len().min(3)].join("/"))
    })
}

#[derive(Debug, Deserialize)]
struct NuGetSearchResponse {
    #[serde(default)]
    data: Vec<NuGetPackage>,
}

#[derive(Debug, Deserialize)]
struct NuGetPackage {
    #[serde(default)]
    description: Option<String>,
    #[serde(default, rename = "projectUrl")]
    project_url: Option<String>,
}

fn discover_nuget(name: &str) -> Result<Option<DiscoveredLibrary>> {
    // NuGet's flat-container index (version list only) has no description/
    // homepage; the search endpoint is the one that returns package
    // metadata, queried for an exact package id match.
    let url = format!("https://azuresearch-usnc.nuget.org/query?q=packageid:{name}&take=1");
    let response = ureq::get(&url).call();
    let mut response = match response {
        Ok(r) => r,
        Err(ureq::Error::StatusCode(404)) => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("fetching NuGet metadata for '{name}'")),
    };

    let parsed: NuGetSearchResponse = response.body_mut().read_json().with_context(|| format!("parsing NuGet response for '{name}'"))?;
    let Some(pkg) = parsed.data.into_iter().next() else { return Ok(None) };
    Ok(Some(DiscoveredLibrary { name: name.to_string(), description: pkg.description, docs_url: pkg.project_url.clone(), repo_url: pkg.project_url }))
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

/// Blocks (briefly) if called again within `MIN_INTERVAL` of the previous
/// call — real client-side throttling in front of GitHub's unauthenticated
/// 10-req/min search limit, not just a reactive 403 handler.
fn throttle_github() {
    const MIN_INTERVAL: Duration = Duration::from_millis(6_500);
    static LAST_CALL: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();

    let lock = LAST_CALL.get_or_init(|| Mutex::new(None));
    let mut last = lock.lock().expect("github throttle mutex poisoned");
    if let Some(prev) = *last {
        let elapsed = prev.elapsed();
        if elapsed < MIN_INTERVAL {
            std::thread::sleep(MIN_INTERVAL - elapsed);
        }
    }
    *last = Some(Instant::now());
}

/// Fallback discovery step: searches GitHub's repository search API,
/// returning the highest-starred match. Distinguishes a genuine "no match"
/// (`Ok(None)`) from a failed/rate-limited search (`Err`) — the caller
/// needs to tell these apart. Not `pub` — see `discover`'s doc comment.
fn search_github(name: &str) -> Result<Option<DiscoveredLibrary>> {
    throttle_github();

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

fn normalize_repo_url(raw: &str) -> String {
    let without_prefix = raw.strip_prefix("git+").unwrap_or(raw);
    let without_suffix = without_prefix.strip_suffix(".git").unwrap_or(without_prefix);
    without_suffix.replace("git://", "https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_git_plus_prefixed_repo_urls() {
        assert_eq!(normalize_repo_url("git+https://github.com/facebook/react.git"), "https://github.com/facebook/react");
        assert_eq!(normalize_repo_url("https://github.com/facebook/react"), "https://github.com/facebook/react");
    }

    #[test]
    fn discover_returns_a_cached_result_without_hitting_the_network_again() {
        let key = (Ecosystem::Npm, "some-fake-package-for-cache-test".to_string());
        cache().lock().unwrap().insert(
            key,
            Some(DiscoveredLibrary { name: "cached-result".to_string(), description: None, docs_url: None, repo_url: None }),
        );
        let result = discover(Ecosystem::Npm, "some-fake-package-for-cache-test").unwrap();
        assert_eq!(result.unwrap().name, "cached-result");
    }

    #[test]
    fn discover_caches_a_genuine_not_found_result_too() {
        let key = (Ecosystem::PyPi, "some-other-fake-package-for-cache-test".to_string());
        cache().lock().unwrap().insert(key, None);
        let result = discover(Ecosystem::PyPi, "some-other-fake-package-for-cache-test").unwrap();
        assert!(result.is_none());
    }

    // Network-dependent, matching this crate's established practice of
    // verifying against the real registries rather than mocks.
    #[test]
    fn discovers_a_well_known_npm_package() {
        match discover(Ecosystem::Npm, "left-pad") {
            Ok(Some(result)) => assert!(result.repo_url.is_some() || result.docs_url.is_some()),
            Ok(None) => panic!("expected 'left-pad' to exist on npm"),
            Err(e) => eprintln!("skipping network-dependent assertion: {e}"),
        }
    }

    #[test]
    fn discovers_a_well_known_crate() {
        match discover(Ecosystem::Cargo, "serde") {
            Ok(Some(result)) => assert!(result.repo_url.is_some() || result.docs_url.is_some()),
            Ok(None) => panic!("expected 'serde' to exist on crates.io"),
            Err(e) => eprintln!("skipping network-dependent assertion: {e}"),
        }
    }

    #[test]
    fn discovers_a_well_known_go_module() {
        match discover(Ecosystem::Go, "github.com/gin-gonic/gin") {
            Ok(Some(result)) => {
                assert_eq!(result.docs_url.as_deref(), Some("https://pkg.go.dev/github.com/gin-gonic/gin"));
                assert_eq!(result.repo_url.as_deref(), Some("https://github.com/gin-gonic/gin"));
            }
            Ok(None) => panic!("expected 'github.com/gin-gonic/gin' to resolve on pkg.go.dev"),
            Err(e) => eprintln!("skipping network-dependent assertion: {e}"),
        }
    }

    #[test]
    fn discovers_a_well_known_nuget_package() {
        match discover(Ecosystem::NuGet, "Newtonsoft.Json") {
            Ok(Some(result)) => assert!(result.repo_url.is_some() || result.docs_url.is_some()),
            Ok(None) => panic!("expected 'Newtonsoft.Json' to exist on NuGet"),
            Err(e) => eprintln!("skipping network-dependent assertion: {e}"),
        }
    }

    #[test]
    fn go_module_repo_url_is_derived_for_known_forge_hosts() {
        assert_eq!(derive_go_repo_url("github.com/gin-gonic/gin"), Some("https://github.com/gin-gonic/gin".to_string()));
        assert_eq!(derive_go_repo_url("github.com/owner/repo/v2"), Some("https://github.com/owner/repo".to_string()), "a major-version suffix segment must not become part of the repo URL");
    }

    #[test]
    fn go_module_repo_url_is_not_guessed_for_a_vanity_import_path() {
        assert_eq!(derive_go_repo_url("example.com/foo"), None);
    }
}

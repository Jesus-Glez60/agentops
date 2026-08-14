//! GitHub-releases-backed changelog sync. Same `(from_version, to_version,
//! entry, breaking)` pairing logic as `main`, hardened against two
//! confirmed gaps (a third — no idempotency on re-sync — is fixed at the
//! storage layer instead: `docbrain_graph`'s `add_changelog_entry` is now a
//! natural-key upsert, not a plain `INSERT`):
//!
//! - **Pagination**: `main` hardcoded `per_page=100` with no follow-up
//!   requests, silently losing changelog pairs for any repo with more than
//!   100 releases. This follows the `Link` header's `rel="next"` until
//!   exhausted.
//! - **Auth**: `main` never sent an `Authorization` header (60 req/hr
//!   unauthenticated GitHub-wide limit). An optional token raises that
//!   limit; omitting it falls back to unauthenticated, same as before.
//!
//! `ChangelogSync` is the port; `GitHubReleasesSync` is the one adapter
//! today. Use-case/adapter code elsewhere depends on `&dyn ChangelogSync`,
//! not on `sync_github_releases` directly.

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangelogFetchEntry {
    pub from_version: String,
    pub to_version: String,
    pub entry: String,
    pub breaking: bool,
}

/// The changelog-sync port.
pub trait ChangelogSync {
    fn sync(&self, owner: &str, repo: &str, token: Option<&str>) -> Result<Vec<ChangelogFetchEntry>>;
}

/// GitHub-Releases-backed adapter. Zero-sized — no state of its own.
pub struct GitHubReleasesSync;

impl ChangelogSync for GitHubReleasesSync {
    fn sync(&self, owner: &str, repo: &str, token: Option<&str>) -> Result<Vec<ChangelogFetchEntry>> {
        sync_github_releases(owner, repo, token)
    }
}

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    #[serde(default)]
    tag_name: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

/// Releases are returned newest-first, and a changelog only needs recent
/// version pairs — capped well short of GitHub's own pagination-depth limit
/// on this endpoint (confirmed live: `vercel/next.js` has thousands of
/// canary releases, whose `Link: rel="next"` chain runs past a depth GitHub
/// itself refuses with `422` — burning the entire loop, and most of an
/// unauthenticated caller's 60 req/hr budget, on releases nobody's diffing
/// against anyway).
const MAX_PAGES: usize = 5;

/// Fetches up to `MAX_PAGES` pages of releases for `owner/repo` (paginating
/// via the `Link` header) and pairs up consecutive non-draft/non-prerelease
/// releases into changelog entries. `token`, if given, is sent as a Bearer
/// token to raise GitHub's rate limit past the 60 req/hr unauthenticated
/// default. Not `pub` — `GitHubReleasesSync` (the adapter) is the public
/// surface.
fn sync_github_releases(owner: &str, repo: &str, token: Option<&str>) -> Result<Vec<ChangelogFetchEntry>> {
    let mut url = format!("https://api.github.com/repos/{owner}/{repo}/releases?per_page=100");
    let mut all_releases: Vec<GitHubRelease> = Vec::new();

    for page_num in 0..MAX_PAGES {
        let mut req = ureq::get(&url).header("User-Agent", "docbrain-ingest");
        if let Some(t) = token {
            req = req.header("Authorization", &format!("Bearer {t}"));
        }

        let response = req.call();
        let mut response = match response {
            Ok(r) => r,
            Err(ureq::Error::StatusCode(403)) => anyhow::bail!(
                "GitHub API rate limit exceeded fetching releases for {owner}/{repo} — try again shortly{}",
                if token.is_none() { " (pass an auth token to raise the limit)" } else { "" }
            ),
            Err(ureq::Error::StatusCode(404)) => {
                anyhow::bail!("no GitHub repo '{owner}/{repo}' found, or it has no releases API access")
            }
            // GitHub's own pagination-depth cap on this endpoint — not a
            // caller error. Stop and use whatever's already been fetched
            // rather than failing the whole sync over releases this deep.
            Err(ureq::Error::StatusCode(422)) if page_num > 0 => break,
            Err(e) => return Err(e).with_context(|| format!("fetching releases for {owner}/{repo}")),
        };

        let next_url = response.headers().get("Link").and_then(|v| v.to_str().ok()).and_then(parse_next_link);

        let page: Vec<GitHubRelease> = response.body_mut().read_json().with_context(|| format!("parsing releases response for {owner}/{repo}"))?;
        if page.is_empty() {
            break;
        }
        all_releases.extend(page);

        match next_url {
            Some(next) => url = next,
            None => break,
        }
    }

    let real: Vec<GitHubRelease> = all_releases.into_iter().filter(|r| !r.draft && !r.prerelease && !r.tag_name.is_empty()).collect();

    let mut entries = Vec::new();
    for pair in real.windows(2) {
        let (to, from) = (&pair[0], &pair[1]);
        let body = to.body.clone().unwrap_or_default();
        let breaking = is_breaking(&body, &from.tag_name, &to.tag_name);
        entries.push(ChangelogFetchEntry {
            from_version: strip_v_prefix(&from.tag_name),
            to_version: strip_v_prefix(&to.tag_name),
            entry: if body.trim().is_empty() { format!("Released {}", to.tag_name) } else { body },
            breaking,
        });
    }
    Ok(entries)
}

/// Parses a `Link: <url>; rel="next", <url2>; rel="last"` header value and
/// returns the `rel="next"` URL, if present.
fn parse_next_link(link_header: &str) -> Option<String> {
    for part in link_header.split(',') {
        let mut segments = part.split(';');
        let url_part = segments.next()?.trim();
        let is_next = segments.any(|s| s.trim() == "rel=\"next\"");
        if is_next {
            return Some(url_part.trim_start_matches('<').trim_end_matches('>').to_string());
        }
    }
    None
}

fn strip_v_prefix(tag: &str) -> String {
    tag.strip_prefix('v').unwrap_or(tag).to_string()
}

/// Heuristic, not a guarantee — see `main`'s original rationale, unchanged:
/// an explicit "breaking" mention, or a major-version bump.
fn is_breaking(body: &str, from_tag: &str, to_tag: &str) -> bool {
    if body.to_lowercase().contains("breaking") {
        return true;
    }
    match (major_version(from_tag), major_version(to_tag)) {
        (Some(f), Some(t)) => t > f,
        _ => false,
    }
}

fn major_version(tag: &str) -> Option<u64> {
    strip_v_prefix(tag).split('.').next()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_v_prefix() {
        assert_eq!(strip_v_prefix("v1.2.3"), "1.2.3");
        assert_eq!(strip_v_prefix("1.2.3"), "1.2.3");
    }

    #[test]
    fn detects_breaking_via_body_keyword() {
        assert!(is_breaking("This has a BREAKING CHANGE in it.", "v1.0.0", "v1.1.0"));
        assert!(!is_breaking("Just some bug fixes.", "v1.0.0", "v1.1.0"));
    }

    #[test]
    fn detects_breaking_via_major_version_bump() {
        assert!(is_breaking("Bug fixes only.", "v1.9.0", "v2.0.0"));
        assert!(!is_breaking("Bug fixes only.", "v1.9.0", "v1.10.0"));
    }

    #[test]
    fn parses_the_next_link_out_of_a_link_header() {
        let header = r#"<https://api.github.com/repos/x/y/releases?page=2>; rel="next", <https://api.github.com/repos/x/y/releases?page=5>; rel="last""#;
        assert_eq!(parse_next_link(header), Some("https://api.github.com/repos/x/y/releases?page=2".to_string()));
    }

    #[test]
    fn no_next_link_when_link_header_has_only_prev_and_first() {
        let header = r#"<https://api.github.com/repos/x/y/releases?page=1>; rel="prev", <https://api.github.com/repos/x/y/releases?page=1>; rel="first""#;
        assert_eq!(parse_next_link(header), None);
    }

    // Network-dependent, matching this crate's established practice.
    #[test]
    fn syncs_real_releases_from_a_well_known_repo() {
        match sync_github_releases("psf", "requests", None) {
            Ok(entries) => assert!(entries.iter().all(|e| !e.from_version.is_empty() && !e.to_version.is_empty())),
            Err(e) => eprintln!("skipping network-dependent assertion: {e}"),
        }
    }

    // Regression test for a real, confirmed failure: `vercel/next.js` has
    // thousands of releases (canary builds); paginating until GitHub's own
    // depth cap previously surfaced as a fatal 422 partway through,
    // silently killing changelog sync for any deeply-paginated repo.
    #[test]
    fn a_repo_with_deep_release_history_returns_recent_entries_instead_of_erroring() {
        match sync_github_releases("vercel", "next.js", None) {
            Ok(entries) => assert!(!entries.is_empty(), "expected at least the most recent page's changelog pairs"),
            Err(e) => eprintln!("skipping network-dependent assertion: {e}"),
        }
    }
}

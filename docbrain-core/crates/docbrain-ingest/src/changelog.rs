//! GitHub-releases-backed changelog sync — Docbrain-2's "fine-grained
//! multi-version awareness" needs real changelog entries between exact
//! versions, not just doc snapshots per version. GitHub's releases API
//! already gives exactly this (one entry per tag, in descending order),
//! so this turns that into the same `(from_version, to_version, entry,
//! breaking)` shape `docbrain-graph::add_changelog_entry` expects, one
//! entry per *consecutive* pair of releases.

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangelogFetchEntry {
    pub from_version: String,
    pub to_version: String,
    pub entry: String,
    pub breaking: bool,
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

/// Fetches releases for `owner/repo` from GitHub and pairs up consecutive
/// (non-draft, non-prerelease) releases into changelog entries — GitHub
/// returns releases newest-first, so `releases[i]` is `to_version` and
/// `releases[i + 1]` is the `from_version` it changed from. A release with
/// an empty body still produces an entry (some maintainers just list the
/// diff via the tag itself) rather than being silently dropped.
pub fn sync_github_releases(owner: &str, repo: &str) -> Result<Vec<ChangelogFetchEntry>> {
    let url = format!("https://api.github.com/repos/{owner}/{repo}/releases?per_page=100");
    let response = ureq::get(&url).header("User-Agent", "docbrain-ingest").call();
    let mut response = match response {
        Ok(r) => r,
        Err(ureq::Error::StatusCode(403)) => {
            anyhow::bail!("GitHub API rate limit exceeded fetching releases for {owner}/{repo} — try again shortly")
        }
        Err(ureq::Error::StatusCode(404)) => {
            anyhow::bail!("no GitHub repo '{owner}/{repo}' found, or it has no releases API access")
        }
        Err(e) => return Err(e).with_context(|| format!("fetching releases for {owner}/{repo}")),
    };

    let releases: Vec<GitHubRelease> =
        response.body_mut().read_json().with_context(|| format!("parsing releases response for {owner}/{repo}"))?;

    let real: Vec<GitHubRelease> = releases.into_iter().filter(|r| !r.draft && !r.prerelease && !r.tag_name.is_empty()).collect();

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

fn strip_v_prefix(tag: &str) -> String {
    tag.strip_prefix('v').unwrap_or(tag).to_string()
}

/// Heuristic, not a guarantee: a release body explicitly saying "breaking"
/// (case-insensitive, the common changelog convention — "BREAKING CHANGE",
/// "Breaking:", etc.), or a major-version bump between the two tags (the
/// semver convention breaking changes are supposed to follow), counts as
/// breaking. Neither signal is reliable alone — a lot of real-world
/// projects don't bump major for genuinely breaking changes — so this is
/// documented as a heuristic, not treated as authoritative.
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

    // Hits the real GitHub API against a real, stable repo with many
    // releases — same "verify against the real thing" practice as this
    // crate's other network-facing tests.
    #[test]
    fn syncs_real_releases_from_a_well_known_repo() {
        match sync_github_releases("psf", "requests") {
            Ok(entries) => {
                assert!(!entries.is_empty(), "requests has many tagged releases");
                assert!(entries.iter().all(|e| !e.from_version.is_empty() && !e.to_version.is_empty()));
            }
            Err(e) => assert!(e.to_string().contains("rate limit"), "unexpected error: {e}"),
        }
    }
}

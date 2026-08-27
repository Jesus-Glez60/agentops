//! `GET /scans`, `POST /repos/{name}/rescan`, `GET /activity` — the
//! dashboard's repo-intelligence data, composed entirely from
//! already-existing primitives (`agentops-manifest`'s local scan registry +
//! `agentops-graph`'s per-repo store). Scoped to **local repos only** — no
//! `agentops-heavy-api` connection registry involved, since the pipeline
//! that would clone+scan a hosted-connected repo doesn't exist yet (see the
//! frontend kickoff plan for the full reasoning).
//!
//! No health/staleness logic lives here — that's a client-computed
//! threshold in the frontend (`apps/web/src/lib/repo-health.ts`, ported from
//! `main`'s tested version), matching `main`'s own explicit precedent that
//! no backend field should own that classification.

use std::path::{Path, PathBuf};

use agentops_graph::{GraphStore, Node, NodeKind, NodeProminence};
use agentops_manifest::ManifestEntry;
use axum::extract::{Path as AxumPath, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::search::snippet;
use crate::AppState;

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct RepoCounts {
    pub symbols: i64,
    pub files: i64,
    pub gotchas: i64,
    /// The subset of `gotchas` a human hasn't looked at yet (`!curated`).
    /// `gotchas` stays the honest total -- gotchas are permanent knowledge,
    /// never "resolved away" -- this is what drives the dashboard's
    /// "needs curation" stat and health warning, since only curating a
    /// gotcha (never deleting/hiding one) brings this count down.
    pub gotchas_needing_curation: i64,
    pub decisions: i64,
}

#[derive(Serialize, Clone, Debug)]
pub struct RepoSummary {
    pub name: String,
    pub path: String,
    pub branch: Option<String>,
    pub last_scanned_at: u64,
    /// `None` when the manifest entry has never actually been scanned into
    /// the graph, or its store couldn't be opened — never fabricated zeros.
    pub counts: Option<RepoCounts>,
    /// `true` when the manifest's recorded path no longer exists on disk
    /// (deleted/moved repo) — the row still surfaces (a real, once-scanned
    /// repo) but honestly flagged rather than silently omitted or 500ing
    /// the whole list for every other repo.
    pub path_missing: bool,
}

#[derive(Serialize, Clone, Debug)]
pub struct ActivityEvent {
    pub repo: String,
    pub started_at: String,
    pub files_added: i64,
    pub files_changed: i64,
    pub files_removed: i64,
    pub symbols_added: i64,
    pub symbols_changed: i64,
    pub symbols_removed: i64,
}

fn count_by_kind(nodes: &[Node]) -> RepoCounts {
    let mut counts = RepoCounts { symbols: 0, files: 0, gotchas: 0, gotchas_needing_curation: 0, decisions: 0 };
    for node in nodes {
        match node.kind {
            NodeKind::Symbol => counts.symbols += 1,
            NodeKind::File => counts.files += 1,
            NodeKind::Gotcha => {
                counts.gotchas += 1;
                if !node.curated {
                    counts.gotchas_needing_curation += 1;
                }
            }
            NodeKind::Decision => counts.decisions += 1,
            // Definition/Note/DocSection aren't part of this dashboard's
            // four-segment breakdown (matches the design tokens already
            // established for Symbols/Files/Gotchas/Decisions specifically).
            NodeKind::Definition | NodeKind::Note | NodeKind::DocSection => {}
        }
    }
    counts
}

/// Reads the checked-out branch straight from `<repo_path>/.git`, no
/// `git2`/`gix` dependency needed. Handles the normal case (`ref:
/// refs/heads/<name>` in `.git/HEAD`), **detached HEAD** (`.git/HEAD` holds
/// a raw 40-char commit SHA instead — shown as a short `"detached @
/// <sha7>"` label rather than crashing or silently misrendering the SHA as
/// if it were a branch name), and **worktrees** (`.git` is a *file*
/// containing `gitdir: <path>`, not a directory — followed before reading
/// `HEAD`).
pub fn read_branch(repo_path: &Path) -> Option<String> {
    let dot_git = repo_path.join(".git");
    let git_dir = if dot_git.is_dir() {
        dot_git
    } else {
        let contents = std::fs::read_to_string(&dot_git).ok()?;
        let pointer = contents.trim().strip_prefix("gitdir:")?.trim();
        PathBuf::from(pointer)
    };

    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    if let Some(branch) = head.strip_prefix("ref: refs/heads/") {
        Some(branch.to_string())
    } else if head.len() >= 7 && head.bytes().all(|b| b.is_ascii_hexdigit()) {
        Some(format!("detached @ {}", &head[..7]))
    } else {
        None
    }
}

/// Every manifest entry whose store actually opened, paired with its
/// resolved repo name -- the shared core of `GET /activity` and `GET
/// /local-search`, both of which only care about repos with real, queryable
/// data and silently skip everything else (unlike `GET /scans`, which must
/// show a `path_missing` row rather than hide the gap, so it keeps its own
/// per-entry logic in `summarize_repo`). `pub` -- `agentops-heavy-api`'s
/// tenant-scoped equivalents call this same pattern against
/// `ConnectionStore`-resolved stores instead of manifest entries.
pub fn open_scanned_repos(entries: &[ManifestEntry]) -> Vec<(String, Box<dyn GraphStore>)> {
    entries
        .iter()
        .filter_map(|entry| {
            let path = PathBuf::from(&entry.path);
            if !path.exists() {
                return None;
            }
            let name = agentops_mcp::repo_name(&path);
            agentops_mcp::open_store(&path).ok().map(|store| (name, store))
        })
        .collect()
}

/// Resolves a display name (`agentops_mcp::repo_name`'s output) back to its
/// manifest entry -- shared by `rescan_repo_json` and `search.rs`'s node
/// detail route, both of which take a repo *name* in the URL path rather
/// than a raw filesystem path. `pub` for the same cross-crate reuse reason
/// as `open_scanned_repos`.
pub fn find_by_name<'a>(entries: &'a [ManifestEntry], name: &str) -> Option<&'a ManifestEntry> {
    entries.iter().find(|e| agentops_mcp::repo_name(&PathBuf::from(&e.path)) == name)
}

/// `pub` -- `agentops-heavy-api`'s `GET /repos` extension calls this against
/// a synthetic `ManifestEntry` built from each tenant connection's resolved
/// checkout path (`ManifestEntry` is a plain two-field data carrier, not
/// coupled to the real manifest file, so constructing one on the fly for
/// this purpose is legitimate reuse, not a hack).
pub fn summarize_repo(entry: &ManifestEntry) -> RepoSummary {
    let path = PathBuf::from(&entry.path);
    let name = agentops_mcp::repo_name(&path);

    if !path.exists() {
        return RepoSummary { name, path: entry.path.clone(), branch: None, last_scanned_at: entry.last_scanned_at, counts: None, path_missing: true };
    }

    let branch = read_branch(&path);

    let store = match agentops_mcp::open_store(&path) {
        Ok(store) => store,
        Err(e) => {
            eprintln!("GET /scans: could not open graph store for {:?}: {e:#}", entry.path);
            return RepoSummary { name, path: entry.path.clone(), branch, last_scanned_at: entry.last_scanned_at, counts: None, path_missing: true };
        }
    };

    let counts = match store.latest_scan(&name) {
        Ok(Some(_)) => store.all_nodes(&name).ok().map(|nodes| count_by_kind(&nodes)),
        _ => None,
    };

    RepoSummary { name, path: entry.path.clone(), branch, last_scanned_at: entry.last_scanned_at, counts, path_missing: false }
}

/// Same `spawn_blocking` reasoning as `/nodes` in `lib.rs`: `open_store` can
/// block on real I/O (a slow Postgres round-trip under
/// `AGENTOPS_DATABASE_URL`), so this must not run on the async executor's
/// own thread even though it's cheap for the SQLite default.
pub(crate) async fn list_repos_json(State(state): State<AppState>) -> (StatusCode, Json<Value>) {
    let manifest_path = state.manifest_path.clone();
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<RepoSummary>> {
        let entries = agentops_manifest::list_scanned_repos_at(&manifest_path)?;
        Ok(entries.iter().map(summarize_repo).collect())
    })
    .await;

    match result {
        Ok(Ok(repos)) => (StatusCode::OK, Json(json!({ "repos": repos }))),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("internal task error: {e}") }))),
    }
}

pub(crate) async fn rescan_repo_json(State(state): State<AppState>, AxumPath(name): AxumPath<String>) -> (StatusCode, Json<Value>) {
    let manifest_path = state.manifest_path.clone();
    let target_name = name.clone();
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<RepoSummary>> {
        let entries = agentops_manifest::list_scanned_repos_at(&manifest_path)?;
        let Some(entry) = find_by_name(&entries, &target_name) else {
            return Ok(None);
        };
        let path = PathBuf::from(&entry.path);

        // `with_embeddings: true` -- the dashboard's rescan button is now
        // also what populates real semantic-search data (see the search
        // page plan). The *first* rescan of a repo after this change
        // downloads BGE-small-en-v1.5 (~130MB, one-time, cached after) and
        // embeds every symbol -- noticeably slower than a normal rescan.
        // Every rescan after that only re-embeds new/changed symbols, per
        // persist()'s existing change-detection.
        agentops_mcp::scan_and_persist(&path, true)?;
        // scan_and_persist itself never touches the manifest (only
        // agentops-cli's `install` command does, after persisting) -- a
        // rescan triggered from here must bump last_scanned_at itself, or
        // the dashboard's own "Last scan" column would go stale the moment
        // you actually use its rescan button.
        agentops_manifest::record_scan_at(&manifest_path, &path)?;

        let refreshed_entries = agentops_manifest::list_scanned_repos_at(&manifest_path)?;
        let refreshed = refreshed_entries.iter().find(|e| e.path == entry.path).cloned().unwrap_or_else(|| entry.clone());
        Ok(Some(summarize_repo(&refreshed)))
    })
    .await;

    match result {
        Ok(Ok(Some(summary))) => (StatusCode::OK, Json(serde_json::to_value(summary).unwrap())),
        Ok(Ok(None)) => (StatusCode::NOT_FOUND, Json(json!({ "error": format!("no scanned repo named {name:?}") }))),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("internal task error: {e}") }))),
    }
}

/// Recent scan events across every manifest repo, newest-first, capped —
/// backs the dashboard's scrolling activity ticker. `started_at` is a
/// SQLite `CURRENT_TIMESTAMP`-formatted string (lexicographically
/// sortable), so a plain string comparison is a correct sort here, not an
/// approximation.
pub(crate) async fn activity_json(State(state): State<AppState>) -> (StatusCode, Json<Value>) {
    let manifest_path = state.manifest_path.clone();
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<ActivityEvent>> {
        let entries = agentops_manifest::list_scanned_repos_at(&manifest_path)?;
        let mut events = Vec::new();
        for (name, store) in open_scanned_repos(&entries) {
            if let Ok(Some(scan)) = store.latest_scan(&name) {
                events.push(ActivityEvent {
                    repo: name,
                    started_at: scan.started_at,
                    files_added: scan.files_added,
                    files_changed: scan.files_changed,
                    files_removed: scan.files_removed,
                    symbols_added: scan.symbols_added,
                    symbols_changed: scan.symbols_changed,
                    symbols_removed: scan.symbols_removed,
                });
            }
        }
        events.sort_by(|a, b| b.started_at.cmp(&a.started_at));
        events.truncate(20);
        Ok(events)
    })
    .await;

    match result {
        Ok(Ok(events)) => (StatusCode::OK, Json(json!({ "activity": events }))),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("internal task error: {e}") }))),
    }
}

#[derive(Serialize, Debug)]
pub struct GotchaSummary {
    pub repo: String,
    pub id: i64,
    pub name: Option<String>,
    pub path: Option<String>,
    pub container: Option<String>,
    pub start_line: Option<i64>,
    pub end_line: Option<i64>,
    pub snippet: Option<String>,
    pub curated: bool,
    pub prominence: NodeProminence,
    pub curation_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct GotchasQuery {
    /// `needs_curation` | `kept` | `reduced`; omitted means every bucket.
    bucket: Option<String>,
    /// Comma-separated repo names; omitted or empty means "all scanned repos".
    repos: Option<String>,
}

pub const GOTCHA_BUCKETS: [&str; 3] = ["needs_curation", "kept", "reduced"];

/// The three real curation states a gotcha can be in -- never a fourth
/// "closed" one. Caller must validate `bucket` against `GOTCHA_BUCKETS`
/// first; an unrecognized bucket here just matches nothing. `pub` -- reused
/// by `agentops-heavy-api`'s tenant-scoped `GET /gotchas`.
pub fn matches_bucket(node: &Node, bucket: &str) -> bool {
    match bucket {
        "needs_curation" => !node.curated,
        "kept" => node.curated && node.prominence == NodeProminence::Full,
        "reduced" => node.prominence == NodeProminence::Reduced,
        _ => false,
    }
}

/// `GET /gotchas` — every `Gotcha` node across scanned repos (optionally
/// filtered by curation bucket/repo), for the dashboard's dedicated
/// curation page. Composes `open_scanned_repos` + `GraphStore::nodes_by_kind`
/// exactly the way `activity_json` composes `open_scanned_repos` +
/// `latest_scan` — no new cross-repo iteration logic.
pub(crate) async fn gotchas_json(State(state): State<AppState>, Query(q): Query<GotchasQuery>) -> (StatusCode, Json<Value>) {
    let manifest_path = state.manifest_path.clone();
    let bucket = q.bucket.clone();
    if let Some(b) = &bucket {
        if !GOTCHA_BUCKETS.contains(&b.as_str()) {
            return (StatusCode::BAD_REQUEST, Json(json!({ "error": format!("invalid 'bucket': {b:?}") })));
        }
    }
    let repo_filter: Option<Vec<String>> = q.repos.as_deref().map(|s| s.split(',').map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect());

    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<GotchaSummary>> {
        let entries = agentops_manifest::list_scanned_repos_at(&manifest_path)?;
        let mut repos = open_scanned_repos(&entries);
        if let Some(names) = &repo_filter {
            repos.retain(|(name, _)| names.contains(name));
        }

        let mut gotchas = Vec::new();
        for (name, store) in &repos {
            for node in store.nodes_by_kind(name, NodeKind::Gotcha)? {
                if let Some(b) = &bucket {
                    if !matches_bucket(&node, b) {
                        continue;
                    }
                }
                gotchas.push(GotchaSummary {
                    repo: name.clone(),
                    id: node.id,
                    name: node.name,
                    path: node.path,
                    container: node.container,
                    start_line: node.start_line,
                    end_line: node.end_line,
                    snippet: snippet(&node.content),
                    curated: node.curated,
                    prominence: node.prominence,
                    curation_reason: node.curation_reason,
                });
            }
        }
        Ok(gotchas)
    })
    .await;

    match result {
        Ok(Ok(gotchas)) => (StatusCode::OK, Json(json!({ "gotchas": gotchas }))),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("internal task error: {e}") }))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn body_json(response: axum::response::Response) -> Value {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn manifest_path() -> PathBuf {
        tempfile::tempdir().unwrap().keep().join("manifest.json")
    }

    fn test_app(manifest_path: PathBuf) -> axum::Router {
        crate::build_router(agentops_mcp::AccessMode::Full, None, manifest_path)
    }

    #[tokio::test]
    async fn a_repo_thats_never_been_scanned_has_null_counts_not_zeros() {
        let dir = tempfile::tempdir().unwrap();
        let manifest_path = manifest_path();
        agentops_manifest::record_scan_at(&manifest_path, dir.path()).unwrap();

        let app = test_app(manifest_path);
        let resp = app.oneshot(Request::builder().uri("/scans").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp).await;
        let repos = body["repos"].as_array().unwrap();
        assert_eq!(repos.len(), 1);
        assert!(repos[0]["counts"].is_null(), "a manifest entry with no scan history must report null counts, not fabricated zeros: {repos:?}");
        assert_eq!(repos[0]["path_missing"], false);
    }

    #[tokio::test]
    async fn get_repos_reports_real_node_counts_after_a_scan() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.py"), "def greet():\n    return 'hi'\n").unwrap();
        agentops_mcp::scan_and_persist(dir.path(), false).unwrap();
        let manifest_path = manifest_path();
        agentops_manifest::record_scan_at(&manifest_path, dir.path()).unwrap();

        let app = test_app(manifest_path);
        let resp = app.oneshot(Request::builder().uri("/scans").body(Body::empty()).unwrap()).await.unwrap();
        let body = body_json(resp).await;
        let repos = body["repos"].as_array().unwrap();
        assert_eq!(repos[0]["counts"]["files"], 1);
        assert_eq!(repos[0]["counts"]["symbols"], 1);
    }

    #[tokio::test]
    async fn one_deleted_repo_path_does_not_500_the_whole_list() {
        let real_dir = tempfile::tempdir().unwrap();
        let deleted_dir = tempfile::tempdir().unwrap();
        let deleted_path = deleted_dir.path().to_path_buf();
        drop(deleted_dir);
        std::fs::remove_dir_all(&deleted_path).ok();

        let manifest_path = manifest_path();
        agentops_manifest::record_scan_at(&manifest_path, real_dir.path()).unwrap();
        agentops_manifest::record_scan_at(&manifest_path, &deleted_path).unwrap();

        let app = test_app(manifest_path);
        let resp = app.oneshot(Request::builder().uri("/scans").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "one bad path must not fail the entire request");
        let body = body_json(resp).await;
        let repos = body["repos"].as_array().unwrap();
        assert_eq!(repos.len(), 2);
        assert!(repos.iter().any(|r| r["path_missing"] == true));
        assert!(repos.iter().any(|r| r["path_missing"] == false));
    }

    // Both tests below go through the real `POST /repos/{name}/rescan`
    // route, which now runs with `with_embeddings: true` -- the first time
    // this test binary runs after that change, these two calls trigger a
    // real BGE-small-en-v1.5 download/load (cached by fastembed/hf-hub
    // after that, so only the very first run pays the cost). Deliberate:
    // this route is the only thing that populates real search data, so a
    // test actually exercising it must exercise the real path, not a faked
    // one -- same reasoning `agentops-embeddings`' own live model test uses.

    #[tokio::test]
    async fn rescan_updates_counts_and_last_scanned_at() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.py"), "def greet():\n    return 'hi'\n").unwrap();
        agentops_mcp::scan_and_persist(dir.path(), false).unwrap();
        let manifest_path = manifest_path();
        agentops_manifest::record_scan_at(&manifest_path, dir.path()).unwrap();
        let name = agentops_mcp::repo_name(dir.path());

        std::fs::write(dir.path().join("second.py"), "def wave():\n    return 'hi'\n").unwrap();

        let app = test_app(manifest_path);
        let resp = app.oneshot(Request::builder().method("POST").uri(format!("/repos/{name}/rescan")).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp).await;
        assert_eq!(body["counts"]["files"], 2, "rescan must reflect the new file: {body:?}");
    }

    #[tokio::test]
    async fn rescan_actually_populates_real_embeddings_not_just_graph_nodes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.py"), "def greet():\n    return 'hi'\n").unwrap();
        let manifest_path = manifest_path();
        agentops_manifest::record_scan_at(&manifest_path, dir.path()).unwrap();
        let name = agentops_mcp::repo_name(dir.path());

        let app = test_app(manifest_path);
        let resp = app.oneshot(Request::builder().method("POST").uri(format!("/repos/{name}/rescan")).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let store = agentops_mcp::open_store(dir.path()).unwrap();
        let query_embedding = agentops_embeddings::Embedder::embed(&agentops_embeddings::LocalEmbedder, "greet").unwrap();
        let hits = store.search_similar(&name, &query_embedding, 5, None).unwrap();
        assert!(!hits.is_empty(), "the dashboard's rescan route must leave real embeddings behind, not just graph nodes, or semantic search stays permanently empty");
    }

    #[tokio::test]
    async fn rescanning_an_unknown_repo_name_404s() {
        let manifest_path = manifest_path();
        let app = test_app(manifest_path);
        let resp = app.oneshot(Request::builder().method("POST").uri("/repos/nope/rescan").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn activity_lists_recent_scans_newest_first_and_capped() {
        let dir_a = tempfile::tempdir().unwrap();
        std::fs::write(dir_a.path().join("a.py"), "def a():\n    return 1\n").unwrap();
        agentops_mcp::scan_and_persist(dir_a.path(), false).unwrap();

        let dir_b = tempfile::tempdir().unwrap();
        std::fs::write(dir_b.path().join("b.py"), "def b():\n    return 2\n").unwrap();
        agentops_mcp::scan_and_persist(dir_b.path(), false).unwrap();

        let manifest_path = manifest_path();
        agentops_manifest::record_scan_at(&manifest_path, dir_a.path()).unwrap();
        agentops_manifest::record_scan_at(&manifest_path, dir_b.path()).unwrap();

        let app = test_app(manifest_path);
        let resp = app.oneshot(Request::builder().uri("/activity").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp).await;
        let activity = body["activity"].as_array().unwrap();
        assert_eq!(activity.len(), 2);
    }

    fn gotcha_note_dir() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let notes_dir = dir.path().join(".agentops").join("notes");
        std::fs::create_dir_all(&notes_dir).unwrap();
        std::fs::write(notes_dir.join("gotcha.md"), "---\ntitle: \"A real gotcha\"\ntype: gotcha\n---\n\nSomething tricky.\n").unwrap();
        let path = dir.path().to_path_buf();
        (dir, path)
    }

    #[tokio::test]
    async fn gotchas_json_lists_a_real_gotcha_defaulting_to_uncurated_full_prominence() {
        let (_dir, path) = gotcha_note_dir();
        agentops_mcp::scan_and_persist(&path, false).unwrap();
        let manifest_path = manifest_path();
        agentops_manifest::record_scan_at(&manifest_path, &path).unwrap();

        let app = test_app(manifest_path);
        let resp = app.oneshot(Request::builder().uri("/gotchas").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp).await;
        let gotchas = body["gotchas"].as_array().unwrap();
        assert_eq!(gotchas.len(), 1, "{body:?}");
        assert_eq!(gotchas[0]["curated"], false);
        assert_eq!(gotchas[0]["prominence"], "Full");
        assert!(gotchas[0]["curation_reason"].is_null());
    }

    #[tokio::test]
    async fn gotchas_json_bucket_filter_moves_a_curated_gotcha_out_of_needs_curation() {
        let (_dir, path) = gotcha_note_dir();
        agentops_mcp::scan_and_persist(&path, false).unwrap();
        let manifest_path = manifest_path();
        agentops_manifest::record_scan_at(&manifest_path, &path).unwrap();
        let name = agentops_mcp::repo_name(&path);

        let store = agentops_mcp::open_store(&path).unwrap();
        let gotcha = store.nodes_by_kind(&name, NodeKind::Gotcha).unwrap().into_iter().next().unwrap();
        store.set_curation(&name, gotcha.id, NodeProminence::Reduced, Some("niche")).unwrap();

        let app = test_app(manifest_path);
        let resp = app.clone().oneshot(Request::builder().uri("/gotchas?bucket=needs_curation").body(Body::empty()).unwrap()).await.unwrap();
        let body = body_json(resp).await;
        assert_eq!(body["gotchas"].as_array().unwrap().len(), 0, "a curated gotcha must not appear in needs_curation: {body:?}");

        let resp = app.oneshot(Request::builder().uri("/gotchas?bucket=reduced").body(Body::empty()).unwrap()).await.unwrap();
        let body = body_json(resp).await;
        assert_eq!(body["gotchas"].as_array().unwrap().len(), 1, "the reduced bucket must show it: {body:?}");
        assert_eq!(body["gotchas"][0]["curation_reason"], "niche");
    }

    #[tokio::test]
    async fn gotchas_json_rejects_an_unrecognized_bucket() {
        let manifest_path = manifest_path();
        let app = test_app(manifest_path);
        let resp = app.oneshot(Request::builder().uri("/gotchas?bucket=bogus").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn get_repos_reports_gotchas_needing_curation_as_the_uncurated_subset() {
        let (_dir, path) = gotcha_note_dir();
        agentops_mcp::scan_and_persist(&path, false).unwrap();
        let manifest_path = manifest_path();
        agentops_manifest::record_scan_at(&manifest_path, &path).unwrap();
        let name = agentops_mcp::repo_name(&path);

        let store = agentops_mcp::open_store(&path).unwrap();
        let gotcha = store.nodes_by_kind(&name, NodeKind::Gotcha).unwrap().into_iter().next().unwrap();
        store.set_curation(&name, gotcha.id, NodeProminence::Reduced, Some("niche")).unwrap();

        let app = test_app(manifest_path);
        let resp = app.oneshot(Request::builder().uri("/scans").body(Body::empty()).unwrap()).await.unwrap();
        let body = body_json(resp).await;
        let counts = &body["repos"][0]["counts"];
        assert_eq!(counts["gotchas"], 1, "the honest total must still count a curated gotcha: {counts:?}");
        assert_eq!(counts["gotchas_needing_curation"], 0, "a curated gotcha (even Reduced) must not count as needing curation: {counts:?}");
    }

    #[test]
    fn read_branch_resolves_a_normal_checked_out_branch() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".git/HEAD"), "ref: refs/heads/feat/full-rework\n").unwrap();

        assert_eq!(read_branch(dir.path()), Some("feat/full-rework".to_string()));
    }

    #[test]
    fn read_branch_reports_a_short_sha_for_detached_head() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".git/HEAD"), "a1b2c3d4e5f60718293a4b5c6d7e8f9012345678\n").unwrap();

        assert_eq!(read_branch(dir.path()), Some("detached @ a1b2c3d".to_string()));
    }

    #[test]
    fn read_branch_follows_a_worktrees_gitdir_pointer() {
        let real_repo = tempfile::tempdir().unwrap();
        let worktree_git_dir = real_repo.path().join("worktrees").join("wt1");
        std::fs::create_dir_all(&worktree_git_dir).unwrap();
        std::fs::write(worktree_git_dir.join("HEAD"), "ref: refs/heads/some-worktree-branch\n").unwrap();

        let worktree = tempfile::tempdir().unwrap();
        std::fs::write(worktree.path().join(".git"), format!("gitdir: {}\n", worktree_git_dir.display())).unwrap();

        assert_eq!(read_branch(worktree.path()), Some("some-worktree-branch".to_string()));
    }

    #[test]
    fn read_branch_is_none_for_a_path_with_no_git_dir_at_all() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_branch(dir.path()), None);
    }
}

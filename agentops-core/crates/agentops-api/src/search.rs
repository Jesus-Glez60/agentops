//! `GET /local-search`, `GET /repos/{name}/nodes/{id}` — real dense semantic
//! search + the search page's detail-panel endpoint. Composes
//! `agentops-embeddings`/`agentops-graph` primitives that already exist;
//! no new search logic, no `GraphStore` trait changes.
//!
//! **Distance → similarity**: `search_similar` queries a `vec0` table with
//! no `distance_metric` set, which per `sqlite-vec`'s own docs defaults to
//! **L2**, not cosine (despite `agentops-graph`'s doc comment calling it
//! "cosine distance" — imprecise about mechanism, not about ranking:
//! `fastembed`'s `TextEmbedding::embed` L2-normalizes every vector before
//! returning it, and for unit vectors `L2_distance² = 2 − 2·cosine_similarity`,
//! so L2 order already equals cosine order). The similarity this route
//! displays is therefore `1 − (distance² / 2)`, not the naive `1 − distance`
//! a true-cosine-distance assumption would suggest.

use std::path::PathBuf;

use agentops_embeddings::{Embedder, LocalEmbedder};
use agentops_graph::{Edge, EdgeRelation, Node, NodeKind, NodeProminence};
use axum::extract::{Path as AxumPath, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::repos::{find_by_name, open_scanned_repos};
use crate::AppState;

const DEFAULT_TOP_K: usize = 20;
/// How much of `Node.content` a list-view result shows before the detail
/// panel is opened -- long enough to be useful, short enough to keep
/// `/search`'s response small when it's fanning out across every repo.
const SNIPPET_CHARS: usize = 240;

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    q: String,
    /// Comma-separated repo names; omitted or empty means "all scanned repos".
    repos: Option<String>,
    /// Comma-separated kinds (symbol/file/gotcha/decision/definition/note);
    /// omitted or empty means "all kinds".
    kind: Option<String>,
    top_k: Option<usize>,
}

#[derive(Serialize, Debug)]
pub struct SearchResult {
    pub repo: String,
    pub id: i64,
    pub kind: NodeKind,
    pub name: Option<String>,
    pub path: Option<String>,
    pub container: Option<String>,
    pub start_line: Option<i64>,
    pub end_line: Option<i64>,
    pub snippet: Option<String>,
    pub similarity: f32,
    pub curated: bool,
    pub prominence: NodeProminence,
    pub curation_reason: Option<String>,
}

/// `pub` -- `repos.rs`'s `gotchas_json` reuses this instead of a second copy
/// of the same truncation logic, and so does `agentops-heavy-api`'s
/// tenant-scoped `GET /gotchas`/`GET /local-search`.
pub fn snippet(content: &Option<String>) -> Option<String> {
    content.as_ref().map(|c| if c.chars().count() > SNIPPET_CHARS { format!("{}…", c.chars().take(SNIPPET_CHARS).collect::<String>()) } else { c.clone() })
}

/// `pub` -- reused by `agentops-heavy-api`'s tenant-scoped `GET
/// /local-search` so the distance→similarity derivation (see this module's
/// doc comment) lives in exactly one place.
pub fn distance_to_similarity(distance: f32) -> f32 {
    1.0 - (distance.powi(2) / 2.0)
}

/// `pub` -- same reuse reason as `distance_to_similarity`.
pub fn node_to_result(repo: &str, node: Node, distance: f32) -> SearchResult {
    SearchResult {
        repo: repo.to_string(),
        id: node.id,
        kind: node.kind,
        name: node.name,
        path: node.path,
        container: node.container,
        start_line: node.start_line,
        end_line: node.end_line,
        snippet: snippet(&node.content),
        similarity: distance_to_similarity(distance),
        curated: node.curated,
        prominence: node.prominence,
        curation_reason: node.curation_reason,
    }
}

pub(crate) async fn search_json(State(state): State<AppState>, Query(q): Query<SearchQuery>) -> (StatusCode, Json<Value>) {
    let manifest_path = state.manifest_path.clone();
    let top_k = q.top_k.unwrap_or(DEFAULT_TOP_K);
    let repo_filter: Option<Vec<String>> = q.repos.as_deref().map(|s| s.split(',').map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect());
    let kind_filter: Result<Vec<NodeKind>, String> = q
        .kind
        .as_deref()
        .map(|s| s.split(',').map(str::trim).filter(|p| !p.is_empty()).map(|k| crate::parse_kind(k).ok_or_else(|| k.to_string())).collect())
        .unwrap_or(Ok(Vec::new()));
    let kinds = match kind_filter {
        Ok(k) => k,
        Err(bad) => return (StatusCode::BAD_REQUEST, Json(json!({ "error": format!("invalid 'kind': {bad:?}") }))),
    };
    // An empty kind list means "no filter" -- one `None`-kind pass per repo
    // rather than one pass per every possible kind (cheaper, and matches
    // search_similar's own "None = no kind restriction" contract).
    let kind_passes: Vec<Option<NodeKind>> = if kinds.is_empty() { vec![None] } else { kinds.into_iter().map(Some).collect() };

    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<SearchResult>> {
        let embedding = LocalEmbedder.embed(&q.q)?;

        let entries = agentops_manifest::list_scanned_repos_at(&manifest_path)?;
        let mut repos = open_scanned_repos(&entries);
        if let Some(names) = &repo_filter {
            repos.retain(|(name, _)| names.contains(name));
        }

        let mut hits: Vec<SearchResult> = Vec::new();
        for (name, store) in &repos {
            for kind in &kind_passes {
                for (node, distance) in store.search_similar(name, &embedding, top_k, *kind)? {
                    hits.push(node_to_result(name, node, distance));
                }
            }
        }
        // Highest similarity (== lowest distance) first, across every
        // repo/kind pass merged together -- sorted by a damped copy of
        // similarity, not the field itself, so a Reduced-prominence hit
        // ranks lower but `SearchResult.similarity` (the UI's relevance
        // badge source) stays the real, undamped cosine-derived value.
        hits.sort_by(|a, b| {
            let rank_a = a.similarity * agentops_graph::prominence_rank_multiplier(a.prominence) as f32;
            let rank_b = b.similarity * agentops_graph::prominence_rank_multiplier(b.prominence) as f32;
            rank_b.partial_cmp(&rank_a).unwrap_or(std::cmp::Ordering::Equal)
        });
        hits.truncate(top_k);
        Ok(hits)
    })
    .await;

    match result {
        Ok(Ok(results)) => (StatusCode::OK, Json(json!({ "results": results }))),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("internal task error: {e}") }))),
    }
}

#[derive(Serialize, Debug)]
pub struct ConnectedNode {
    pub id: i64,
    pub kind: NodeKind,
    pub name: Option<String>,
    pub path: Option<String>,
    /// e.g. `"affects"` for an outgoing edge, `"← affects"` for an incoming
    /// one -- matches `main`'s own directional-arrow convention for the
    /// same concept.
    pub relation: String,
}

#[derive(Serialize, Debug)]
pub struct NodeDetail {
    pub id: i64,
    pub kind: NodeKind,
    pub repo: String,
    pub path: Option<String>,
    pub name: Option<String>,
    pub container: Option<String>,
    pub start_line: Option<i64>,
    pub end_line: Option<i64>,
    pub content: Option<String>,
    pub connected: Vec<ConnectedNode>,
    pub curated: bool,
    pub prominence: NodeProminence,
    pub curation_reason: Option<String>,
}

/// `pub(crate)` so `subgraph.rs` reuses this instead of re-deriving the
/// same match statement.
pub(crate) fn relation_label(relation: EdgeRelation, incoming: bool) -> String {
    let label = match relation {
        EdgeRelation::DependsOn => "depends on",
        EdgeRelation::Documents => "documents",
        EdgeRelation::Affects => "affects",
        EdgeRelation::References => "references",
        EdgeRelation::Covers => "covers",
    };
    if incoming { format!("← {label}") } else { label.to_string() }
}

/// `pub` -- reused by `agentops-heavy-api`'s tenant-scoped
/// `GET /repos/{id}/nodes/{node_id}`.
pub fn connected_nodes(store: &dyn agentops_graph::GraphStore, repo: &str, node_id: i64) -> anyhow::Result<Vec<ConnectedNode>> {
    let mut connected = Vec::new();
    let resolve = |store: &dyn agentops_graph::GraphStore, other_id: i64| -> anyhow::Result<Option<Node>> { store.get_node(repo, other_id) };

    let outgoing: Vec<Edge> = store.edges_from(repo, node_id)?;
    for edge in outgoing {
        if let Some(other) = resolve(store, edge.dst_id)? {
            connected.push(ConnectedNode { id: other.id, kind: other.kind, name: other.name, path: other.path, relation: relation_label(edge.relation, false) });
        }
    }
    let incoming: Vec<Edge> = store.edges_to(repo, node_id)?;
    for edge in incoming {
        if let Some(other) = resolve(store, edge.src_id)? {
            connected.push(ConnectedNode { id: other.id, kind: other.kind, name: other.name, path: other.path, relation: relation_label(edge.relation, true) });
        }
    }
    Ok(connected)
}

pub(crate) async fn node_detail_json(State(state): State<AppState>, AxumPath((repo_name, id)): AxumPath<(String, i64)>) -> (StatusCode, Json<Value>) {
    let manifest_path = state.manifest_path.clone();
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<NodeDetail>> {
        let entries = agentops_manifest::list_scanned_repos_at(&manifest_path)?;
        let Some(entry) = find_by_name(&entries, &repo_name) else { return Ok(None) };
        let path = PathBuf::from(&entry.path);
        if !path.exists() {
            return Ok(None);
        }
        let store = agentops_mcp::open_store(&path)?;

        let Some(node) = store.get_node(&repo_name, id)? else { return Ok(None) };
        let connected = connected_nodes(store.as_ref(), &repo_name, id)?;

        Ok(Some(NodeDetail {
            id: node.id,
            kind: node.kind,
            repo: repo_name.clone(),
            path: node.path,
            name: node.name,
            container: node.container,
            start_line: node.start_line,
            end_line: node.end_line,
            content: node.content,
            curated: node.curated,
            prominence: node.prominence,
            curation_reason: node.curation_reason,
            connected,
        }))
    })
    .await;

    match result {
        Ok(Ok(Some(detail))) => (StatusCode::OK, Json(serde_json::to_value(detail).unwrap())),
        Ok(Ok(None)) => (StatusCode::NOT_FOUND, Json(json!({ "error": "no such node" }))),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("internal task error: {e}") }))),
    }
}

#[derive(Debug, Deserialize)]
pub struct SetCurationBody {
    prominence: String,
    /// Required (non-empty) when `prominence` is `"reduced"`, ignored when
    /// `"full"` -- a demotion without a reason would just be an unexplained
    /// downranking, which defeats the point of curation being visible.
    reason: Option<String>,
}

fn parse_prominence(s: &str) -> Option<NodeProminence> {
    match s {
        "full" => Some(NodeProminence::Full),
        "reduced" => Some(NodeProminence::Reduced),
        _ => None,
    }
}

/// `POST /repos/{name}/nodes/{id}/curation` — the gotchas page's Keep/
/// Reduce/Restore actions. Path resolution mirrors `node_detail_json`
/// exactly (unknown repo/path/node all 404, not a generic error).
pub(crate) async fn set_curation_json(State(state): State<AppState>, AxumPath((repo_name, id)): AxumPath<(String, i64)>, Json(body): Json<SetCurationBody>) -> (StatusCode, Json<Value>) {
    let Some(prominence) = parse_prominence(&body.prominence) else {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": format!("invalid 'prominence': {:?}", body.prominence) })));
    };
    let reason = body.reason.as_deref().map(str::trim).filter(|r| !r.is_empty());
    if prominence == NodeProminence::Reduced && reason.is_none() {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "'reason' is required (non-empty) when prominence is 'reduced'" })));
    }
    let reason = reason.map(str::to_string);

    let manifest_path = state.manifest_path.clone();
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<()>> {
        let entries = agentops_manifest::list_scanned_repos_at(&manifest_path)?;
        let Some(entry) = find_by_name(&entries, &repo_name) else { return Ok(None) };
        let path = PathBuf::from(&entry.path);
        if !path.exists() {
            return Ok(None);
        }
        let store = agentops_mcp::open_store(&path)?;

        if store.get_node(&repo_name, id)?.is_none() {
            return Ok(None);
        }
        store.set_curation(&repo_name, id, prominence, reason.as_deref())?;
        Ok(Some(()))
    })
    .await;

    match result {
        Ok(Ok(Some(()))) => (StatusCode::OK, Json(json!({ "id": id, "prominence": prominence }))),
        Ok(Ok(None)) => (StatusCode::NOT_FOUND, Json(json!({ "error": "no such node" }))),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("internal task error: {e}") }))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentops_graph::NodeKind;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use std::path::Path;
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

    /// Scans `dir` with real embeddings and records it in the manifest --
    /// every test below that exercises `/search` needs this (dense search
    /// against a repo with no embeddings correctly returns nothing, which
    /// isn't what these tests are checking). First run in this binary pays
    /// the one-time BGE model download, same as `repos.rs`'s own tests.
    fn scan_with_embeddings(manifest_path: &Path, dir: &Path) -> String {
        agentops_mcp::scan_and_persist(dir, true).unwrap();
        agentops_manifest::record_scan_at(manifest_path, dir).unwrap();
        agentops_mcp::repo_name(dir)
    }

    #[test]
    fn distance_to_similarity_matches_the_derived_formula() {
        // Identical unit vectors: distance 0 -> similarity 1.0.
        assert!((distance_to_similarity(0.0) - 1.0).abs() < 1e-6);
        // Exactly antipodal unit vectors (maximally dissimilar): distance
        // 2 -> similarity -1.0, matching cosine similarity's own range --
        // if this were silently clamped to 0 instead, that would hide a
        // real inversion bug rather than surface it.
        assert!((distance_to_similarity(2.0) - (-1.0)).abs() < 1e-6);
    }

    #[tokio::test]
    async fn search_finds_real_matches_across_multiple_repos_sorted_by_similarity() {
        let dir_a = tempfile::tempdir().unwrap();
        std::fs::write(dir_a.path().join("auth.py"), "def refresh_session_token():\n    \"\"\"Rotates the access and refresh tokens.\"\"\"\n    pass\n").unwrap();

        let dir_b = tempfile::tempdir().unwrap();
        std::fs::write(dir_b.path().join("billing.py"), "def charge_credit_card():\n    \"\"\"Processes a payment for an invoice.\"\"\"\n    pass\n").unwrap();

        let manifest_path = manifest_path();
        scan_with_embeddings(&manifest_path, dir_a.path());
        scan_with_embeddings(&manifest_path, dir_b.path());

        let app = test_app(manifest_path);
        let resp = app.oneshot(Request::builder().uri("/local-search?q=refreshing+a+user%27s+session+token").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp).await;
        let results = body["results"].as_array().unwrap();
        assert!(!results.is_empty(), "expected real matches: {body:?}");
        assert_eq!(results[0]["name"], "refresh_session_token", "the closer semantic match must rank first: {results:?}");

        // Sorted by similarity, descending.
        let scores: Vec<f64> = results.iter().map(|r| r["similarity"].as_f64().unwrap()).collect();
        let mut sorted = scores.clone();
        sorted.sort_by(|a, b| b.partial_cmp(a).unwrap());
        assert_eq!(scores, sorted, "results must be sorted best-match-first: {scores:?}");
    }

    #[tokio::test]
    async fn search_ranks_a_reduced_prominence_hit_lower_but_reports_its_real_similarity() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("auth.py"), "def refresh_session_token():\n    \"\"\"Rotates the access and refresh tokens.\"\"\"\n    pass\n").unwrap();
        let manifest_path = manifest_path();
        let name = scan_with_embeddings(&manifest_path, dir.path());

        let store = agentops_mcp::open_store(dir.path()).unwrap();
        let symbol = store.nodes_by_kind(&name, NodeKind::Symbol).unwrap().into_iter().next().unwrap();
        drop(store);
        let app = test_app(manifest_path);

        // Confirm the real similarity before curating.
        let resp = app.clone().oneshot(Request::builder().uri("/local-search?q=refreshing+a+user%27s+session+token").body(Body::empty()).unwrap()).await.unwrap();
        let body = body_json(resp).await;
        let before = body["results"].as_array().unwrap().iter().find(|r| r["id"] == symbol.id).unwrap()["similarity"].as_f64().unwrap();

        let curation_resp = app.clone().oneshot(curation_request(format!("/repos/{name}/nodes/{}/curation", symbol.id), "reduced", Some("niche"))).await.unwrap();
        assert_eq!(curation_resp.status(), StatusCode::OK);

        let resp = app.oneshot(Request::builder().uri("/local-search?q=refreshing+a+user%27s+session+token").body(Body::empty()).unwrap()).await.unwrap();
        let body = body_json(resp).await;
        let results = body["results"].as_array().unwrap();
        let hit = results.iter().find(|r| r["id"] == symbol.id).expect("curation must never hide a hit");
        assert_eq!(hit["similarity"].as_f64().unwrap(), before, "the displayed similarity must stay real/undamped after curation");
    }

    #[tokio::test]
    async fn search_kind_filter_restricts_to_the_requested_kinds() {
        let dir = tempfile::tempdir().unwrap();
        let notes_dir = dir.path().join(".agentops").join("notes");
        std::fs::create_dir_all(&notes_dir).unwrap();
        std::fs::write(dir.path().join("auth.py"), "def refresh_session_token():\n    pass\n").unwrap();
        std::fs::write(notes_dir.join("token-note.md"), "---\ntitle: \"Token rotation\"\ntype: gotcha\n---\n\nrefresh_session_token rotates both tokens.\n").unwrap();

        let manifest_path = manifest_path();
        let name = scan_with_embeddings(&manifest_path, dir.path());

        let app = test_app(manifest_path);

        let symbol_only = app
            .clone()
            .oneshot(Request::builder().uri(format!("/local-search?q=token+rotation&repos={name}&kind=symbol")).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let symbol_body = body_json(symbol_only).await;
        let symbol_results = symbol_body["results"].as_array().unwrap();
        assert!(!symbol_results.is_empty());
        assert!(symbol_results.iter().all(|r| r["kind"] == "Symbol"), "kind=symbol must exclude every other kind: {symbol_results:?}");

        let gotcha_only = app
            .oneshot(Request::builder().uri(format!("/local-search?q=token+rotation&repos={name}&kind=gotcha")).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let gotcha_body = body_json(gotcha_only).await;
        let gotcha_results = gotcha_body["results"].as_array().unwrap();
        assert!(!gotcha_results.is_empty());
        assert!(gotcha_results.iter().all(|r| r["kind"] == "Gotcha"), "kind=gotcha must exclude every other kind: {gotcha_results:?}");
    }

    #[tokio::test]
    async fn search_rejects_an_unrecognized_kind() {
        let app = test_app(manifest_path());
        let resp = app.oneshot(Request::builder().uri("/local-search?q=x&kind=not-a-real-kind").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn one_unscanned_repo_does_not_fail_the_whole_search() {
        let scanned_dir = tempfile::tempdir().unwrap();
        std::fs::write(scanned_dir.path().join("auth.py"), "def refresh_session_token():\n    pass\n").unwrap();
        let unscanned_dir = tempfile::tempdir().unwrap();

        let manifest_path = manifest_path();
        scan_with_embeddings(&manifest_path, scanned_dir.path());
        // Recorded in the manifest but never actually scanned -- open_store
        // will succeed (a fresh, empty graph.db) but has no embeddings.
        agentops_manifest::record_scan_at(&manifest_path, unscanned_dir.path()).unwrap();

        let app = test_app(manifest_path);
        let resp = app.oneshot(Request::builder().uri("/local-search?q=refreshing+a+session+token").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "an unscanned repo alongside a real one must not fail the whole search");
        let body = body_json(resp).await;
        assert!(!body["results"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn node_detail_returns_the_node_and_correctly_labeled_connected_edges() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("auth.py"), "def refresh_session_token():\n    pass\ndef verify_token():\n    pass\n").unwrap();
        agentops_mcp::scan_and_persist(dir.path(), false).unwrap();

        let store = agentops_mcp::open_store(dir.path()).unwrap();
        let name = agentops_mcp::repo_name(dir.path());
        let symbols = store.nodes_by_kind(&name, NodeKind::Symbol).unwrap();
        let refresh = symbols.iter().find(|n| n.name.as_deref() == Some("refresh_session_token")).unwrap();
        let verify = symbols.iter().find(|n| n.name.as_deref() == Some("verify_token")).unwrap();
        store.add_edge(&name, refresh.id, verify.id, EdgeRelation::DependsOn).unwrap();
        drop(store);

        let manifest_path = manifest_path();
        agentops_manifest::record_scan_at(&manifest_path, dir.path()).unwrap();

        let app = test_app(manifest_path);

        // From refresh_session_token's side: an outgoing edge.
        let resp = app.clone().oneshot(Request::builder().uri(format!("/repos/{name}/nodes/{}", refresh.id)).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp).await;
        assert_eq!(body["name"], "refresh_session_token");
        let connected = body["connected"].as_array().unwrap();
        assert!(connected.iter().any(|c| c["name"] == "verify_token" && c["relation"] == "depends on"), "outgoing edge must be labeled without an arrow: {connected:?}");

        // From verify_token's side: the same edge, incoming.
        let resp = app.oneshot(Request::builder().uri(format!("/repos/{name}/nodes/{}", verify.id)).body(Body::empty()).unwrap()).await.unwrap();
        let body = body_json(resp).await;
        let connected = body["connected"].as_array().unwrap();
        assert!(connected.iter().any(|c| c["name"] == "refresh_session_token" && c["relation"] == "← depends on"), "incoming edge must be arrow-prefixed: {connected:?}");
    }

    #[tokio::test]
    async fn node_detail_404s_for_an_unknown_node_id() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("auth.py"), "def f():\n    pass\n").unwrap();
        agentops_mcp::scan_and_persist(dir.path(), false).unwrap();
        let manifest_path = manifest_path();
        let name = scan_with_embeddings(&manifest_path, dir.path());

        let app = test_app(manifest_path);
        let resp = app.oneshot(Request::builder().uri(format!("/repos/{name}/nodes/999999")).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn node_detail_404s_for_an_unknown_repo_name() {
        let app = test_app(manifest_path());
        let resp = app.oneshot(Request::builder().uri("/repos/nope/nodes/1").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    fn curation_request(uri: String, prominence: &str, reason: Option<&str>) -> Request<Body> {
        Request::builder().method("POST").uri(uri).header("content-type", "application/json").body(Body::from(json!({ "prominence": prominence, "reason": reason }).to_string())).unwrap()
    }

    #[tokio::test]
    async fn set_curation_updates_the_node_and_round_trips_through_node_detail() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("auth.py"), "def f():\n    pass\n").unwrap();
        agentops_mcp::scan_and_persist(dir.path(), false).unwrap();
        let store = agentops_mcp::open_store(dir.path()).unwrap();
        let name = agentops_mcp::repo_name(dir.path());
        let symbol = store.nodes_by_kind(&name, NodeKind::Symbol).unwrap().into_iter().next().unwrap();
        drop(store);

        let manifest_path = manifest_path();
        agentops_manifest::record_scan_at(&manifest_path, dir.path()).unwrap();
        let app = test_app(manifest_path);

        let resp = app.clone().oneshot(curation_request(format!("/repos/{name}/nodes/{}/curation", symbol.id), "reduced", Some("niche"))).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp).await;
        assert_eq!(body["prominence"], "Reduced");

        let resp = app.oneshot(Request::builder().uri(format!("/repos/{name}/nodes/{}", symbol.id)).body(Body::empty()).unwrap()).await.unwrap();
        let body = body_json(resp).await;
        assert_eq!(body["prominence"], "Reduced", "the curation change must be durable, not just echoed back: {body:?}");
        assert_eq!(body["curation_reason"], "niche");
        assert_eq!(body["curated"], true);
    }

    #[tokio::test]
    async fn set_curation_rejects_reduced_prominence_with_no_reason() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("auth.py"), "def f():\n    pass\n").unwrap();
        agentops_mcp::scan_and_persist(dir.path(), false).unwrap();
        let name = agentops_mcp::repo_name(dir.path());
        let manifest_path = manifest_path();
        agentops_manifest::record_scan_at(&manifest_path, dir.path()).unwrap();

        let app = test_app(manifest_path);
        let resp = app.oneshot(curation_request(format!("/repos/{name}/nodes/1/curation"), "reduced", None)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn set_curation_rejects_an_unrecognized_prominence() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("auth.py"), "def f():\n    pass\n").unwrap();
        agentops_mcp::scan_and_persist(dir.path(), false).unwrap();
        let name = agentops_mcp::repo_name(dir.path());
        let manifest_path = manifest_path();
        agentops_manifest::record_scan_at(&manifest_path, dir.path()).unwrap();

        let app = test_app(manifest_path);
        let resp = app.oneshot(curation_request(format!("/repos/{name}/nodes/1/curation"), "bogus", None)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn set_curation_404s_for_an_unknown_node_id() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("auth.py"), "def f():\n    pass\n").unwrap();
        agentops_mcp::scan_and_persist(dir.path(), false).unwrap();
        let name = agentops_mcp::repo_name(dir.path());
        let manifest_path = manifest_path();
        agentops_manifest::record_scan_at(&manifest_path, dir.path()).unwrap();

        let app = test_app(manifest_path);
        let resp = app.oneshot(curation_request(format!("/repos/{name}/nodes/999999/curation"), "full", None)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn set_curation_404s_for_an_unknown_repo_name() {
        let app = test_app(manifest_path());
        let resp = app.oneshot(curation_request("/repos/nope/nodes/1/curation".to_string(), "full", None)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}

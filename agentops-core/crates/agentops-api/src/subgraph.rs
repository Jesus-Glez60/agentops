//! `GET /repos/{name}/nodes/{id}/graph` — the Knowledge Graph screen's
//! multi-hop subgraph endpoint. Orchestration on top of `GraphStore`'s
//! existing single-hop `edges_from`/`edges_to`; no new trait methods, no
//! schema changes. `SubgraphNode` deliberately omits `content`/`container`/
//! `start_line`/`end_line`/`connected` -- those stay exclusively on
//! `GET /repos/{name}/nodes/{id}` (`node_detail_json`), which the frontend
//! calls separately on node selection, keeping this payload small at
//! `NODE_CAP` nodes and avoiding field duplication/drift between the two
//! endpoints.

use std::collections::HashSet;
use std::path::PathBuf;

use agentops_graph::{Edge, EdgeRelation, GraphStore, Node, NodeKind, NodeProminence};
use axum::extract::{Path as AxumPath, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::repos::find_by_name;
use crate::AppState;

/// Hard cap on nodes returned per subgraph -- the real governor on both
/// cost and render legibility, not the depth slider (a hub `File` node with
/// hundreds of incoming `DependsOn` edges is realistic, and React Flow's
/// circular kind-colored nodes become unreadable well before a few
/// hundred). Also doubles as a latency guard against `PostgresGraphStore`,
/// whose `edges_from`/`edges_to` each pay a fresh pool-checkout + blocking
/// round trip -- an uncapped BFS against a hub node would mean an unbounded
/// number of those per request.
const NODE_CAP: usize = 150;

#[derive(Debug, Deserialize)]
pub struct SubgraphQuery {
    mode: String,
    depth: Option<u32>,
    /// Comma-separated kinds, same convention as `SearchQuery.kind` --
    /// empty/omitted means "no filter".
    kind: Option<String>,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct SubgraphNode {
    pub id: i64,
    pub kind: NodeKind,
    pub name: Option<String>,
    pub path: Option<String>,
    pub curated: bool,
    pub prominence: NodeProminence,
    /// BFS distance from the seed node; `0` for the seed itself.
    pub depth: u32,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct SubgraphEdge {
    pub id: i64,
    pub src_id: i64,
    pub dst_id: i64,
    pub relation: EdgeRelation,
    /// Bare word ("depends on"/"documents"/"affects"), no "← " prefix --
    /// direction is conveyed structurally via `src_id`/`dst_id` in a
    /// multi-node graph, unlike `search.rs::relation_label`'s single-node
    /// perspective.
    pub label: String,
}

#[derive(Serialize, Debug)]
pub struct SubgraphResponse {
    pub seed_id: i64,
    pub mode: String,
    pub depth: u32,
    pub nodes: Vec<SubgraphNode>,
    pub edges: Vec<SubgraphEdge>,
    pub truncated: bool,
}

enum Direction {
    Outgoing,
    Incoming,
    Both,
}

/// Maps a mode name to which edge relations it follows and in which
/// direction(s) -- `None` for an unrecognized mode (caller 400s).
///
/// `References` (same-file symbol-to-symbol) joins `DependsOn` in the
/// direction-relative modes, not just `local` -- it's a symbol-granularity
/// flavor of "depends on," so "what does this depend on"/"what depends on
/// this" should reflect it the same way file-level `DependsOn` already does.
fn mode_filter(mode: &str) -> Option<(Vec<EdgeRelation>, Direction)> {
    match mode {
        "local" => Some((vec![EdgeRelation::DependsOn, EdgeRelation::Documents, EdgeRelation::Affects, EdgeRelation::References], Direction::Both)),
        "dep_chain" => Some((vec![EdgeRelation::DependsOn, EdgeRelation::References], Direction::Outgoing)),
        "impact" => Some((vec![EdgeRelation::DependsOn, EdgeRelation::References], Direction::Incoming)),
        "knowledge" => Some((vec![EdgeRelation::Affects], Direction::Both)),
        _ => None,
    }
}

fn node_to_subgraph_node(node: Node, depth: u32) -> SubgraphNode {
    SubgraphNode { id: node.id, kind: node.kind, name: node.name, path: node.path, curated: node.curated, prominence: node.prominence, depth }
}

/// BFS from `seed_id` out to `depth` hops, following `mode`'s relation/
/// direction filter. The seed is always included at `depth: 0` regardless
/// of `kind_filter` -- it's the user's chosen center, not a filterable
/// result. A `kind_filter` mismatch excludes both the node *and* any edge
/// touching it, never a dangling edge reference. Returns `Ok(None)` if
/// `mode` is unrecognized or the seed node doesn't exist.
pub(crate) fn build_subgraph(store: &dyn GraphStore, repo: &str, seed_id: i64, mode: &str, depth: u32, kind_filter: &[NodeKind]) -> anyhow::Result<Option<SubgraphResponse>> {
    let Some((relations, direction)) = mode_filter(mode) else { return Ok(None) };
    let Some(seed) = store.get_node(repo, seed_id)? else { return Ok(None) };

    let mut visited_node_ids: HashSet<i64> = HashSet::from([seed_id]);
    let mut visited_edge_ids: HashSet<i64> = HashSet::new();
    let mut nodes_out: Vec<SubgraphNode> = vec![node_to_subgraph_node(seed, 0)];
    let mut edges_out: Vec<SubgraphEdge> = Vec::new();
    let mut frontier: Vec<i64> = vec![seed_id];
    let mut truncated = false;

    for hop in 1..=depth {
        if frontier.is_empty() || nodes_out.len() >= NODE_CAP {
            break;
        }
        let mut next_frontier: Vec<i64> = Vec::new();

        for node_id in &frontier {
            let mut candidates: Vec<(Edge, i64)> = Vec::new();
            match direction {
                Direction::Outgoing => candidates.extend(store.edges_from(repo, *node_id)?.into_iter().map(|e| { let dst = e.dst_id; (e, dst) })),
                Direction::Incoming => candidates.extend(store.edges_to(repo, *node_id)?.into_iter().map(|e| { let src = e.src_id; (e, src) })),
                Direction::Both => {
                    candidates.extend(store.edges_from(repo, *node_id)?.into_iter().map(|e| { let dst = e.dst_id; (e, dst) }));
                    candidates.extend(store.edges_to(repo, *node_id)?.into_iter().map(|e| { let src = e.src_id; (e, src) }));
                }
            }

            for (edge, other_id) in candidates {
                if !relations.contains(&edge.relation) {
                    continue;
                }
                let Some(other) = store.get_node(repo, other_id)? else { continue };
                if !kind_filter.is_empty() && !kind_filter.contains(&other.kind) {
                    continue;
                }

                let is_new_node = !visited_node_ids.contains(&other_id);
                if is_new_node {
                    if nodes_out.len() >= NODE_CAP {
                        truncated = true;
                        continue;
                    }
                    visited_node_ids.insert(other_id);
                    nodes_out.push(node_to_subgraph_node(other, hop));
                    next_frontier.push(other_id);
                }
                if visited_edge_ids.insert(edge.id) {
                    let label = crate::search::relation_label(edge.relation, false);
                    edges_out.push(SubgraphEdge { id: edge.id, src_id: edge.src_id, dst_id: edge.dst_id, relation: edge.relation, label });
                }
            }
        }

        frontier = next_frontier;
    }

    Ok(Some(SubgraphResponse { seed_id, mode: mode.to_string(), depth, nodes: nodes_out, edges: edges_out, truncated }))
}

/// `GET /repos/{name}/nodes/{id}/graph` -- path resolution mirrors
/// `node_detail_json` exactly (unknown repo/path/node all 404).
pub async fn subgraph_json(State(state): State<AppState>, AxumPath((repo_name, id)): AxumPath<(String, i64)>, Query(q): Query<SubgraphQuery>) -> (StatusCode, Json<Value>) {
    let depth = q.depth.unwrap_or(2).clamp(1, 4);
    let kind_filter: Result<Vec<NodeKind>, String> = q
        .kind
        .as_deref()
        .map(|s| s.split(',').map(str::trim).filter(|p| !p.is_empty()).map(|k| crate::parse_kind(k).ok_or_else(|| k.to_string())).collect())
        .unwrap_or(Ok(Vec::new()));
    let kinds = match kind_filter {
        Ok(k) => k,
        Err(bad) => return (StatusCode::BAD_REQUEST, Json(json!({ "error": format!("invalid 'kind': {bad:?}") }))),
    };

    if mode_filter(&q.mode).is_none() {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": format!("invalid 'mode': {:?}", q.mode) })));
    }

    let manifest_path = state.manifest_path.clone();
    let mode = q.mode.clone();
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<SubgraphResponse>> {
        let entries = agentops_manifest::list_scanned_repos_at(&manifest_path)?;
        let Some(entry) = find_by_name(&entries, &repo_name) else { return Ok(None) };
        let path = PathBuf::from(&entry.path);
        if !path.exists() {
            return Ok(None);
        }
        let store = agentops_mcp::open_store(&path)?;
        build_subgraph(store.as_ref(), &repo_name, id, &mode, depth, &kinds)
    })
    .await;

    match result {
        Ok(Ok(Some(subgraph))) => (StatusCode::OK, Json(serde_json::to_value(subgraph).unwrap())),
        Ok(Ok(None)) => (StatusCode::NOT_FOUND, Json(json!({ "error": "no such node" }))),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("internal task error: {e}") }))),
    }
}

#[derive(Debug, Deserialize)]
pub struct RepoGraphQuery {
    kind: Option<String>,
}

#[derive(Serialize, Debug)]
pub struct RepoGraphResponse {
    pub repo: String,
    pub nodes: Vec<SubgraphNode>,
    pub edges: Vec<SubgraphEdge>,
    pub truncated: bool,
}

/// Every node and edge in `repo` (optionally kind-filtered), not centered
/// on any seed -- the Knowledge Graph screen's "pick a repo, see
/// everything" entry point, as distinct from `build_subgraph`'s BFS around
/// a chosen node. Deliberately **not** subject to `NODE_CAP`: the cap
/// exists for `build_subgraph` because an uncapped BFS around a hub node is
/// unbounded (and, on `PostgresGraphStore`, pays one blocking round trip
/// per node visited) -- neither risk applies here, since this always does
/// exactly two queries (`all_nodes` + `all_edges`) regardless of repo size,
/// and a real repo's edges are what a user asking to see "the whole repo"
/// actually wants back, including the sparse-looking ones that a random
/// 150-node sample would mostly cut out (this endpoint's earlier capped
/// version dropped the vast majority of edges purely because one endpoint
/// fell outside an arbitrary node subset, not because the relationship
/// didn't exist). The `kind` filter is still there for narrowing the
/// *view* down when 1000+ Symbol nodes make it hard to read, not for
/// bounding cost.
pub(crate) fn build_repo_graph(store: &dyn GraphStore, repo: &str, kind_filter: &[NodeKind]) -> anyhow::Result<RepoGraphResponse> {
    let all_nodes = store.all_nodes(repo)?;
    let mut kept_ids: HashSet<i64> = HashSet::new();
    let mut nodes_out: Vec<SubgraphNode> = Vec::new();

    for node in all_nodes {
        if !kind_filter.is_empty() && !kind_filter.contains(&node.kind) {
            continue;
        }
        kept_ids.insert(node.id);
        nodes_out.push(node_to_subgraph_node(node, 0));
    }

    // An edge only survives if both endpoints did -- same "never a dangling
    // edge reference" rule `build_subgraph`'s kind filter follows.
    let edges_out: Vec<SubgraphEdge> = store
        .all_edges(repo)?
        .into_iter()
        .filter(|e| kept_ids.contains(&e.src_id) && kept_ids.contains(&e.dst_id))
        .map(|e| SubgraphEdge { id: e.id, src_id: e.src_id, dst_id: e.dst_id, relation: e.relation, label: crate::search::relation_label(e.relation, false) })
        .collect();

    Ok(RepoGraphResponse { repo: repo.to_string(), nodes: nodes_out, edges: edges_out, truncated: false })
}

/// `GET /repos/{name}/graph` -- path resolution mirrors `node_detail_json`
/// (unknown repo/path all 404).
pub async fn repo_graph_json(State(state): State<AppState>, AxumPath(repo_name): AxumPath<String>, Query(q): Query<RepoGraphQuery>) -> (StatusCode, Json<Value>) {
    let kind_filter: Result<Vec<NodeKind>, String> = q
        .kind
        .as_deref()
        .map(|s| s.split(',').map(str::trim).filter(|p| !p.is_empty()).map(|k| crate::parse_kind(k).ok_or_else(|| k.to_string())).collect())
        .unwrap_or(Ok(Vec::new()));
    let kinds = match kind_filter {
        Ok(k) => k,
        Err(bad) => return (StatusCode::BAD_REQUEST, Json(json!({ "error": format!("invalid 'kind': {bad:?}") }))),
    };

    let manifest_path = state.manifest_path.clone();
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<RepoGraphResponse>> {
        let entries = agentops_manifest::list_scanned_repos_at(&manifest_path)?;
        let Some(entry) = find_by_name(&entries, &repo_name) else { return Ok(None) };
        let path = PathBuf::from(&entry.path);
        if !path.exists() {
            return Ok(None);
        }
        let store = agentops_mcp::open_store(&path)?;
        Ok(Some(build_repo_graph(store.as_ref(), &repo_name, &kinds)?))
    })
    .await;

    match result {
        Ok(Ok(Some(graph))) => (StatusCode::OK, Json(serde_json::to_value(graph).unwrap())),
        Ok(Ok(None)) => (StatusCode::NOT_FOUND, Json(json!({ "error": "no such repo" }))),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("internal task error: {e}") }))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentops_graph::NewNode;

    fn manifest_path() -> PathBuf {
        tempfile::tempdir().unwrap().keep().join("manifest.json")
    }

    fn test_app(manifest_path: PathBuf) -> axum::Router {
        crate::build_router(agentops_mcp::AccessMode::Full, None, manifest_path)
    }

    /// Scans an empty repo (no embeddings -- these tests never search, just
    /// traverse edges) and returns `(repo_name, store)`. Callers build their
    /// own node/edge fixtures on top via `store.add_node`/`store.add_edge`.
    fn scanned_repo(manifest_path: &std::path::Path, dir: &std::path::Path) -> (String, Box<dyn GraphStore>) {
        agentops_mcp::scan_and_persist(dir, false).unwrap();
        agentops_manifest::record_scan_at(manifest_path, dir).unwrap();
        let repo = agentops_mcp::repo_name(dir);
        let store = agentops_mcp::open_store(dir).unwrap();
        (repo, store)
    }

    fn symbol(store: &dyn GraphStore, repo: &str, name: &str) -> i64 {
        store.add_node(NewNode { kind: NodeKind::Symbol, repo: repo.into(), path: Some(format!("{name}.py")), name: Some(name.into()), container: None, start_line: Some(1), end_line: Some(2), content: Some(name.into()) }).unwrap()
    }

    fn file(store: &dyn GraphStore, repo: &str, name: &str) -> i64 {
        store.add_node(NewNode { kind: NodeKind::File, repo: repo.into(), path: Some(name.into()), name: None, container: None, start_line: None, end_line: None, content: None }).unwrap()
    }

    fn gotcha(store: &dyn GraphStore, repo: &str, name: &str) -> i64 {
        store.add_node(NewNode { kind: NodeKind::Gotcha, repo: repo.into(), path: None, name: Some(name.into()), container: None, start_line: None, end_line: None, content: Some("a gotcha".into()) }).unwrap()
    }

    #[test]
    fn local_mode_includes_all_relations_both_directions() {
        let dir = tempfile::tempdir().unwrap();
        let (repo, store) = scanned_repo(&manifest_path(), dir.path());
        let a = symbol(store.as_ref(), &repo, "a");
        let b = symbol(store.as_ref(), &repo, "b");
        let c = file(store.as_ref(), &repo, "c.py");
        let d = symbol(store.as_ref(), &repo, "d");
        let e = symbol(store.as_ref(), &repo, "e");
        store.add_edge(&repo, a, b, EdgeRelation::DependsOn).unwrap();
        store.add_edge(&repo, c, b, EdgeRelation::Documents).unwrap();
        store.add_edge(&repo, b, d, EdgeRelation::Affects).unwrap();
        store.add_edge(&repo, b, e, EdgeRelation::References).unwrap();

        let sub = build_subgraph(store.as_ref(), &repo, b, "local", 2, &[]).unwrap().unwrap();
        let ids: HashSet<i64> = sub.nodes.iter().map(|n| n.id).collect();
        assert_eq!(ids, HashSet::from([a, b, c, d, e]));
        assert_eq!(sub.edges.len(), 4);
    }

    #[test]
    fn dep_chain_mode_is_outgoing_depends_on_only() {
        let dir = tempfile::tempdir().unwrap();
        let (repo, store) = scanned_repo(&manifest_path(), dir.path());
        let a = symbol(store.as_ref(), &repo, "a");
        let b = symbol(store.as_ref(), &repo, "b");
        let c = symbol(store.as_ref(), &repo, "c");
        store.add_edge(&repo, a, b, EdgeRelation::DependsOn).unwrap();
        store.add_edge(&repo, c, a, EdgeRelation::Documents).unwrap();

        let sub = build_subgraph(store.as_ref(), &repo, a, "dep_chain", 2, &[]).unwrap().unwrap();
        let ids: HashSet<i64> = sub.nodes.iter().map(|n| n.id).collect();
        assert_eq!(ids, HashSet::from([a, b]));
    }

    #[test]
    fn dep_chain_mode_also_follows_outgoing_references_edges() {
        let dir = tempfile::tempdir().unwrap();
        let (repo, store) = scanned_repo(&manifest_path(), dir.path());
        let a = symbol(store.as_ref(), &repo, "a");
        let b = symbol(store.as_ref(), &repo, "b");
        store.add_edge(&repo, a, b, EdgeRelation::References).unwrap();

        let sub = build_subgraph(store.as_ref(), &repo, a, "dep_chain", 2, &[]).unwrap().unwrap();
        let ids: HashSet<i64> = sub.nodes.iter().map(|n| n.id).collect();
        assert_eq!(ids, HashSet::from([a, b]), "same-file symbol References is a symbol-granularity flavor of depends-on");
    }

    #[test]
    fn impact_mode_is_incoming_depends_on_only() {
        let dir = tempfile::tempdir().unwrap();
        let (repo, store) = scanned_repo(&manifest_path(), dir.path());
        let a = symbol(store.as_ref(), &repo, "a");
        let b = symbol(store.as_ref(), &repo, "b");
        let c = symbol(store.as_ref(), &repo, "c");
        store.add_edge(&repo, a, b, EdgeRelation::DependsOn).unwrap();
        store.add_edge(&repo, b, c, EdgeRelation::DependsOn).unwrap();

        let sub = build_subgraph(store.as_ref(), &repo, b, "impact", 2, &[]).unwrap().unwrap();
        let ids: HashSet<i64> = sub.nodes.iter().map(|n| n.id).collect();
        assert_eq!(ids, HashSet::from([a, b]), "impact from b must include what depends on b (a), not what b depends on (c)");
    }

    #[test]
    fn impact_mode_also_follows_incoming_references_edges() {
        let dir = tempfile::tempdir().unwrap();
        let (repo, store) = scanned_repo(&manifest_path(), dir.path());
        let a = symbol(store.as_ref(), &repo, "a");
        let b = symbol(store.as_ref(), &repo, "b");
        store.add_edge(&repo, a, b, EdgeRelation::References).unwrap();

        let sub = build_subgraph(store.as_ref(), &repo, b, "impact", 2, &[]).unwrap().unwrap();
        let ids: HashSet<i64> = sub.nodes.iter().map(|n| n.id).collect();
        assert_eq!(ids, HashSet::from([a, b]), "impact from b must include a, which references b");
    }

    #[test]
    fn knowledge_mode_never_follows_references_edges() {
        let dir = tempfile::tempdir().unwrap();
        let (repo, store) = scanned_repo(&manifest_path(), dir.path());
        let a = symbol(store.as_ref(), &repo, "a");
        let b = symbol(store.as_ref(), &repo, "b");
        store.add_edge(&repo, a, b, EdgeRelation::References).unwrap();

        let sub = build_subgraph(store.as_ref(), &repo, a, "knowledge", 2, &[]).unwrap().unwrap();
        assert_eq!(sub.nodes.len(), 1, "knowledge mode is Affects-only; a References edge must not be followed");
    }

    #[test]
    fn knowledge_mode_is_affects_both_directions_and_depth_is_a_no_op_with_no_chained_affects_edges() {
        let dir = tempfile::tempdir().unwrap();
        let (repo, store) = scanned_repo(&manifest_path(), dir.path());
        let g = gotcha(store.as_ref(), &repo, "g");
        let s = symbol(store.as_ref(), &repo, "s");
        store.add_edge(&repo, g, s, EdgeRelation::Affects).unwrap();

        let depth1 = build_subgraph(store.as_ref(), &repo, s, "knowledge", 1, &[]).unwrap().unwrap();
        let depth4 = build_subgraph(store.as_ref(), &repo, s, "knowledge", 4, &[]).unwrap().unwrap();
        let ids1: HashSet<i64> = depth1.nodes.iter().map(|n| n.id).collect();
        let ids4: HashSet<i64> = depth4.nodes.iter().map(|n| n.id).collect();
        assert_eq!(ids1, HashSet::from([g, s]));
        assert_eq!(ids1, ids4, "no special-cased depth logic -- BFS naturally stops with no chained Affects edges");
    }

    #[test]
    fn depth_limits_the_bfs() {
        let dir = tempfile::tempdir().unwrap();
        let (repo, store) = scanned_repo(&manifest_path(), dir.path());
        let a = symbol(store.as_ref(), &repo, "a");
        let b = symbol(store.as_ref(), &repo, "b");
        let c = symbol(store.as_ref(), &repo, "c");
        let d = symbol(store.as_ref(), &repo, "d");
        let e = symbol(store.as_ref(), &repo, "e");
        store.add_edge(&repo, a, b, EdgeRelation::DependsOn).unwrap();
        store.add_edge(&repo, b, c, EdgeRelation::DependsOn).unwrap();
        store.add_edge(&repo, c, d, EdgeRelation::DependsOn).unwrap();
        store.add_edge(&repo, d, e, EdgeRelation::DependsOn).unwrap();

        let shallow = build_subgraph(store.as_ref(), &repo, a, "dep_chain", 2, &[]).unwrap().unwrap();
        let shallow_ids: HashSet<i64> = shallow.nodes.iter().map(|n| n.id).collect();
        assert_eq!(shallow_ids, HashSet::from([a, b, c]));

        let deep = build_subgraph(store.as_ref(), &repo, a, "dep_chain", 4, &[]).unwrap().unwrap();
        let deep_ids: HashSet<i64> = deep.nodes.iter().map(|n| n.id).collect();
        assert_eq!(deep_ids, HashSet::from([a, b, c, d, e]));
    }

    #[test]
    fn bfs_dedupes_a_diamond_shaped_cycle() {
        let dir = tempfile::tempdir().unwrap();
        let (repo, store) = scanned_repo(&manifest_path(), dir.path());
        let a = symbol(store.as_ref(), &repo, "a");
        let b = symbol(store.as_ref(), &repo, "b");
        let c = symbol(store.as_ref(), &repo, "c");
        let d = symbol(store.as_ref(), &repo, "d");
        store.add_edge(&repo, a, b, EdgeRelation::DependsOn).unwrap();
        store.add_edge(&repo, a, c, EdgeRelation::DependsOn).unwrap();
        store.add_edge(&repo, b, d, EdgeRelation::DependsOn).unwrap();
        store.add_edge(&repo, c, d, EdgeRelation::DependsOn).unwrap();
        store.add_edge(&repo, d, a, EdgeRelation::DependsOn).unwrap();

        let sub = build_subgraph(store.as_ref(), &repo, a, "local", 4, &[]).unwrap().unwrap();
        assert_eq!(sub.nodes.len(), 4, "each of a/b/c/d exactly once despite the cycle back to a");
        assert_eq!(sub.edges.len(), 5, "each physical edge exactly once despite Both direction reaching some from both ends");
        let node_ids: Vec<i64> = sub.nodes.iter().map(|n| n.id).collect();
        let unique: HashSet<i64> = node_ids.iter().copied().collect();
        assert_eq!(node_ids.len(), unique.len());
    }

    #[test]
    fn kind_filter_excludes_both_the_node_and_its_edge() {
        let dir = tempfile::tempdir().unwrap();
        let (repo, store) = scanned_repo(&manifest_path(), dir.path());
        let s = symbol(store.as_ref(), &repo, "s");
        let f = file(store.as_ref(), &repo, "f.py");
        store.add_edge(&repo, s, f, EdgeRelation::DependsOn).unwrap();

        let sub = build_subgraph(store.as_ref(), &repo, s, "local", 2, &[NodeKind::Symbol]).unwrap().unwrap();
        assert_eq!(sub.nodes.len(), 1, "the File node must be excluded");
        assert!(sub.edges.is_empty(), "the seed->file edge must be excluded along with the node");
    }

    #[test]
    fn seed_node_is_always_included_even_if_its_own_kind_is_filtered_out() {
        let dir = tempfile::tempdir().unwrap();
        let (repo, store) = scanned_repo(&manifest_path(), dir.path());
        let s = symbol(store.as_ref(), &repo, "s");

        let sub = build_subgraph(store.as_ref(), &repo, s, "local", 2, &[NodeKind::File]).unwrap().unwrap();
        assert_eq!(sub.nodes.len(), 1);
        assert_eq!(sub.nodes[0].id, s);
        assert_eq!(sub.nodes[0].depth, 0);
    }

    #[test]
    fn zero_edge_node_returns_just_the_seed() {
        let dir = tempfile::tempdir().unwrap();
        let (repo, store) = scanned_repo(&manifest_path(), dir.path());
        let s = symbol(store.as_ref(), &repo, "s");

        let sub = build_subgraph(store.as_ref(), &repo, s, "local", 4, &[]).unwrap().unwrap();
        assert_eq!(sub.nodes.len(), 1);
        assert!(sub.edges.is_empty());
        assert!(!sub.truncated);
    }

    #[test]
    fn unknown_mode_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let (repo, store) = scanned_repo(&manifest_path(), dir.path());
        let s = symbol(store.as_ref(), &repo, "s");
        assert!(build_subgraph(store.as_ref(), &repo, s, "bogus", 2, &[]).unwrap().is_none());
    }

    #[test]
    fn unknown_seed_node_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let (repo, store) = scanned_repo(&manifest_path(), dir.path());
        assert!(build_subgraph(store.as_ref(), &repo, 999_999, "local", 2, &[]).unwrap().is_none());
    }

    #[test]
    fn truncated_flag_is_set_when_the_node_cap_is_hit() {
        let dir = tempfile::tempdir().unwrap();
        let (repo, store) = scanned_repo(&manifest_path(), dir.path());
        let hub = symbol(store.as_ref(), &repo, "hub");
        for i in 0..(NODE_CAP + 5) {
            let leaf = symbol(store.as_ref(), &repo, &format!("leaf{i}"));
            store.add_edge(&repo, hub, leaf, EdgeRelation::DependsOn).unwrap();
        }

        let sub = build_subgraph(store.as_ref(), &repo, hub, "local", 2, &[]).unwrap().unwrap();
        assert_eq!(sub.nodes.len(), NODE_CAP);
        assert!(sub.truncated);
    }

    #[tokio::test]
    async fn unknown_repo_404s_over_http() {
        use axum::body::Body;
        use axum::http::Request;
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let app = test_app(manifest_path());
        let req = Request::builder().uri("/repos/nope/nodes/1/graph?mode=local").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let _ = resp.into_body().collect().await.unwrap().to_bytes();
    }

    #[tokio::test]
    async fn invalid_mode_400s_over_http() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        let manifest = manifest_path();
        let dir = tempfile::tempdir().unwrap();
        let (repo, store) = scanned_repo(&manifest, dir.path());
        let s = symbol(store.as_ref(), &repo, "s");
        drop(store);

        let app = test_app(manifest);
        let req = Request::builder().uri(format!("/repos/{repo}/nodes/{s}/graph?mode=bogus")).body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn unknown_node_404s_over_http() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        let manifest = manifest_path();
        let dir = tempfile::tempdir().unwrap();
        let (repo, store) = scanned_repo(&manifest, dir.path());
        drop(store);

        let app = test_app(manifest);
        let req = Request::builder().uri(format!("/repos/{repo}/nodes/999999/graph?mode=local")).body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn repo_graph_includes_every_node_and_edge_unfiltered() {
        let dir = tempfile::tempdir().unwrap();
        let (repo, store) = scanned_repo(&manifest_path(), dir.path());
        let a = symbol(store.as_ref(), &repo, "a");
        let b = symbol(store.as_ref(), &repo, "b");
        let f = file(store.as_ref(), &repo, "f.py");
        let g = gotcha(store.as_ref(), &repo, "g");
        store.add_edge(&repo, a, b, EdgeRelation::DependsOn).unwrap();
        store.add_edge(&repo, g, a, EdgeRelation::Affects).unwrap();

        let graph = build_repo_graph(store.as_ref(), &repo, &[]).unwrap();
        let ids: HashSet<i64> = graph.nodes.iter().map(|n| n.id).collect();
        assert_eq!(ids, HashSet::from([a, b, f, g]));
        assert_eq!(graph.edges.len(), 2);
        assert!(!graph.truncated);
    }

    #[test]
    fn repo_graph_kind_filter_excludes_nodes_and_their_edges() {
        let dir = tempfile::tempdir().unwrap();
        let (repo, store) = scanned_repo(&manifest_path(), dir.path());
        let a = symbol(store.as_ref(), &repo, "a");
        let g = gotcha(store.as_ref(), &repo, "g");
        store.add_edge(&repo, g, a, EdgeRelation::Affects).unwrap();

        let graph = build_repo_graph(store.as_ref(), &repo, &[NodeKind::Symbol]).unwrap();
        assert_eq!(graph.nodes.len(), 1);
        assert_eq!(graph.nodes[0].id, a);
        assert!(graph.edges.is_empty(), "the gotcha->symbol edge must be excluded since the Gotcha endpoint is filtered out");
    }

    #[test]
    fn repo_graph_is_never_truncated_even_past_the_bfs_node_cap() {
        let dir = tempfile::tempdir().unwrap();
        let (repo, store) = scanned_repo(&manifest_path(), dir.path());
        for i in 0..(NODE_CAP + 5) {
            symbol(store.as_ref(), &repo, &format!("s{i}"));
        }

        let graph = build_repo_graph(store.as_ref(), &repo, &[]).unwrap();
        assert_eq!(graph.nodes.len(), NODE_CAP + 5, "unlike build_subgraph's BFS, the whole-repo view has no cap");
        assert!(!graph.truncated);
    }

    #[test]
    fn repo_graph_on_an_empty_repo_returns_nothing_and_is_not_truncated() {
        let dir = tempfile::tempdir().unwrap();
        let (repo, store) = scanned_repo(&manifest_path(), dir.path());
        let graph = build_repo_graph(store.as_ref(), &repo, &[]).unwrap();
        assert!(graph.nodes.is_empty());
        assert!(graph.edges.is_empty());
        assert!(!graph.truncated);
    }

    #[tokio::test]
    async fn repo_graph_unknown_repo_404s_over_http() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        let app = test_app(manifest_path());
        let req = Request::builder().uri("/repos/nope/graph").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn repo_graph_invalid_kind_400s_over_http() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        let manifest = manifest_path();
        let dir = tempfile::tempdir().unwrap();
        let (repo, store) = scanned_repo(&manifest, dir.path());
        drop(store);

        let app = test_app(manifest);
        let req = Request::builder().uri(format!("/repos/{repo}/graph?kind=bogus")).body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn repo_graph_returns_real_data_over_http() {
        use axum::body::Body;
        use axum::http::Request;
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let manifest = manifest_path();
        let dir = tempfile::tempdir().unwrap();
        let (repo, store) = scanned_repo(&manifest, dir.path());
        let a = symbol(store.as_ref(), &repo, "a");
        let b = symbol(store.as_ref(), &repo, "b");
        store.add_edge(&repo, a, b, EdgeRelation::DependsOn).unwrap();
        drop(store);

        let app = test_app(manifest);
        let req = Request::builder().uri(format!("/repos/{repo}/graph")).body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["edges"].as_array().unwrap().len(), 1);
    }
}

//! Graph hotspot / "god node" detection (Graphify-inspired) -- flags
//! over-connected nodes in a repo's own graph, grouped by a hand-rolled
//! single-pass modularity community detection. No new dependency:
//! `petgraph` is already a workspace dependency (used by
//! `agentops-scanner::ranker` for PageRank-based file ranking); its `algo`
//! module ships shortest-path/minimum-spanning-tree/PageRank routines but
//! **no community-detection algorithm of any kind** (confirmed directly
//! against `petgraph`'s own docs, not assumed), and no mature/maintained
//! Leiden or Louvain crate could be confirmed to exist on crates.io either
//! -- this repo has a recorded gotcha about trusting an unverified crate
//! name for exactly this kind of gap
//! (`candle-lora-crate-does-not-exist-hand-rolled-lora-adapter-instead`), so
//! this is a genuine reuse-before-writing gap, not a violation.
//!
//! This implements only Louvain's local-moving phase (single pass, no
//! recursive graph aggregation into a second level) -- sufficient for a
//! first version; if it proves too coarse on a real repo's graph, a second
//! aggregation level is the natural next step, not a rewrite.

use std::collections::HashMap;

use anyhow::Result;
use petgraph::graph::{NodeIndex, UnGraph};
use petgraph::visit::EdgeRef;
use serde::Serialize;

use crate::GraphStore;

/// One over-connected node, with which community (as assigned by the
/// single-pass modularity grouping below) it landed in.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Hotspot {
    pub node_id: i64,
    pub degree: usize,
    pub community: usize,
}

/// Simple (unweighted) degree count -- both edge directions -- per node
/// that has at least one edge in `repo`. A node with zero edges is simply
/// absent from the map, not present with a `0` entry.
pub fn node_degrees(store: &dyn GraphStore, repo: &str) -> Result<HashMap<i64, usize>> {
    let edges = store.all_edges(repo)?;
    let mut degrees: HashMap<i64, usize> = HashMap::new();
    for edge in &edges {
        if edge.src_id == edge.dst_id {
            continue;
        }
        *degrees.entry(edge.src_id).or_insert(0) += 1;
        *degrees.entry(edge.dst_id).or_insert(0) += 1;
    }
    Ok(degrees)
}

/// Repo-relative "over-connected" threshold -- the 90th percentile of this
/// repo's own degree distribution, never a hardcoded absolute node count,
/// consistent with how PPR/decay scoring elsewhere in this codebase scales
/// with the repo's own signals rather than a fixed constant. Floored at 2
/// so a sparse repo where every connected node has degree 1 doesn't flag
/// its entire connected set as "hotspots."
fn hotspot_degree_threshold(degrees: &HashMap<i64, usize>) -> usize {
    let mut values: Vec<usize> = degrees.values().copied().collect();
    values.sort_unstable();
    let idx = ((values.len() as f64) * 0.9).floor() as usize;
    let idx = idx.min(values.len().saturating_sub(1));
    values[idx].max(2)
}

/// Every node in `repo` whose degree is at or above this repo's own 90th
/// percentile, each annotated with its assigned community -- highest degree
/// first. Empty for a repo with no edges (nothing to be "over-connected"
/// relative to).
pub fn detect_hotspots(store: &dyn GraphStore, repo: &str) -> Result<Vec<Hotspot>> {
    let nodes = store.all_nodes(repo)?;
    let edges = store.all_edges(repo)?;
    if nodes.is_empty() || edges.is_empty() {
        return Ok(Vec::new());
    }

    let mut graph: UnGraph<i64, f64> = UnGraph::new_undirected();
    let mut index_of: HashMap<i64, NodeIndex> = HashMap::new();
    for node in &nodes {
        let idx = graph.add_node(node.id);
        index_of.insert(node.id, idx);
    }
    for edge in &edges {
        if edge.src_id == edge.dst_id {
            continue;
        }
        let (Some(&a), Some(&b)) = (index_of.get(&edge.src_id), index_of.get(&edge.dst_id)) else {
            continue;
        };
        // Weight floored above zero -- a zero/negative weight would make
        // this edge invisible to (or destabilize) the modularity gain
        // calculation below, which assumes strictly positive edge mass.
        graph.add_edge(a, b, edge.weight.max(0.0001));
    }

    let communities = louvain_single_pass(&graph);

    let degrees = node_degrees(store, repo)?;
    if degrees.is_empty() {
        return Ok(Vec::new());
    }
    let threshold = hotspot_degree_threshold(&degrees);

    let mut hotspots: Vec<Hotspot> = nodes
        .iter()
        .filter_map(|n| {
            let degree = *degrees.get(&n.id)?;
            if degree < threshold {
                return None;
            }
            let idx = *index_of.get(&n.id)?;
            let community = communities.get(&idx).copied().unwrap_or(0);
            Some(Hotspot { node_id: n.id, degree, community })
        })
        .collect();
    hotspots.sort_by(|a, b| b.degree.cmp(&a.degree).then(a.node_id.cmp(&b.node_id)));
    Ok(hotspots)
}

/// Standard Louvain local-moving phase: each node starts in its own
/// community, then repeatedly moves to whichever neighboring community
/// (including staying put) maximizes modularity gain, until a full pass
/// makes no move or `MAX_ITERATIONS` is hit (a real repo graph is small
/// enough that this always converges well before the cap in practice --
/// the cap exists only to bound worst-case pathological input, not because
/// non-convergence is expected).
fn louvain_single_pass(graph: &UnGraph<i64, f64>) -> HashMap<NodeIndex, usize> {
    let mut community: HashMap<NodeIndex, usize> = graph.node_indices().enumerate().map(|(i, idx)| (idx, i)).collect();

    let degree_weighted: HashMap<NodeIndex, f64> = graph.node_indices().map(|idx| (idx, graph.edges(idx).map(|e| *e.weight()).sum())).collect();

    let total_weight: f64 = graph.edge_indices().map(|e| graph[e]).sum();
    if total_weight <= 0.0 {
        return community;
    }
    let m = total_weight;

    let mut sigma_tot: HashMap<usize, f64> = community.iter().map(|(idx, &c)| (c, degree_weighted[idx])).collect();

    const MAX_ITERATIONS: usize = 20;
    for _ in 0..MAX_ITERATIONS {
        let mut improved = false;

        for idx in graph.node_indices() {
            let current_community = community[&idx];
            let k_i = degree_weighted[&idx];

            *sigma_tot.get_mut(&current_community).unwrap() -= k_i;

            // Sum of edge weights from `idx` into each community it has a
            // neighbor in (using neighbors' *current* community labels).
            let mut k_i_in: HashMap<usize, f64> = HashMap::new();
            for edge in graph.edges(idx) {
                let neighbor = if edge.source() == idx { edge.target() } else { edge.source() };
                if neighbor == idx {
                    continue;
                }
                *k_i_in.entry(community[&neighbor]).or_insert(0.0) += *edge.weight();
            }

            // Standard Louvain modularity-gain formula for moving `idx`
            // into community `c`: ΔQ = k_i,in(c)/m − (Σ_tot(c) · k_i)/(2m²).
            let gain = |c: usize, k_i_in_c: f64| k_i_in_c / m - (sigma_tot.get(&c).copied().unwrap_or(0.0) * k_i) / (2.0 * m * m);

            let mut best_community = current_community;
            let mut best_gain = gain(current_community, *k_i_in.get(&current_community).unwrap_or(&0.0));
            for (&c, &k_i_in_c) in &k_i_in {
                let g = gain(c, k_i_in_c);
                if g > best_gain {
                    best_gain = g;
                    best_community = c;
                }
            }

            *sigma_tot.entry(best_community).or_insert(0.0) += k_i;
            if best_community != current_community {
                community.insert(idx, best_community);
                improved = true;
            }
        }

        if !improved {
            break;
        }
    }

    community
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EdgeRelation, NewNode, NodeKind, SqliteGraphStore};

    fn add_symbol(store: &SqliteGraphStore, repo: &str, name: &str) -> i64 {
        store
            .add_node(NewNode { kind: NodeKind::Symbol, repo: repo.into(), path: Some(format!("{name}.rs")), name: Some(name.into()), container: None, start_line: None, end_line: None, content: Some(format!("fn {name}() {{}}")) })
            .unwrap()
    }

    fn link(store: &SqliteGraphStore, repo: &str, src: i64, dst: i64) {
        store.add_edge(repo, src, dst, EdgeRelation::References).unwrap();
    }

    #[test]
    fn node_degrees_counts_both_edge_directions() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let a = add_symbol(&store, "demo", "a");
        let b = add_symbol(&store, "demo", "b");
        let c = add_symbol(&store, "demo", "c");
        link(&store, "demo", a, b);
        link(&store, "demo", c, b);

        let degrees = node_degrees(&store, "demo").unwrap();
        assert_eq!(degrees[&a], 1);
        assert_eq!(degrees[&b], 2, "b is the target of both edges");
        assert_eq!(degrees[&c], 1);
    }

    #[test]
    fn detect_hotspots_is_empty_for_a_repo_with_no_edges() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        add_symbol(&store, "demo", "isolated");
        assert!(detect_hotspots(&store, "demo").unwrap().is_empty());
    }

    /// Two small, otherwise-disconnected triangles (clear clusters) plus one
    /// deliberately over-connected hub wired to every other node -- the hub
    /// must be flagged as a hotspot, and the two triangle members are not
    /// (identical, low, shared degree far below the hub's).
    #[test]
    fn detect_hotspots_flags_a_deliberately_over_connected_hub_node() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let repo = "demo";
        let hub = add_symbol(&store, repo, "hub");

        let cluster_a: Vec<i64> = (0..3).map(|i| add_symbol(&store, repo, &format!("a{i}"))).collect();
        let cluster_b: Vec<i64> = (0..3).map(|i| add_symbol(&store, repo, &format!("b{i}"))).collect();

        // Triangle within each cluster.
        for cluster in [&cluster_a, &cluster_b] {
            link(&store, repo, cluster[0], cluster[1]);
            link(&store, repo, cluster[1], cluster[2]);
            link(&store, repo, cluster[2], cluster[0]);
        }
        // The hub connects to every node in both clusters -- degree 6,
        // versus every cluster member's degree of 2 (their triangle) plus
        // 1 (the hub) = 3.
        for &member in cluster_a.iter().chain(cluster_b.iter()) {
            link(&store, repo, hub, member);
        }

        let hotspots = detect_hotspots(&store, repo).unwrap();
        assert!(hotspots.iter().any(|h| h.node_id == hub), "the hub must be flagged: {hotspots:?}");
        assert_eq!(hotspots[0].node_id, hub, "the hub must rank first by degree: {hotspots:?}");
        for &member in cluster_a.iter().chain(cluster_b.iter()) {
            assert!(!hotspots.iter().any(|h| h.node_id == member), "an ordinary triangle member must not be flagged: {hotspots:?}");
        }
    }

    #[test]
    fn louvain_groups_two_disconnected_triangles_into_different_communities() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let repo = "demo";
        let cluster_a: Vec<i64> = (0..3).map(|i| add_symbol(&store, repo, &format!("a{i}"))).collect();
        let cluster_b: Vec<i64> = (0..3).map(|i| add_symbol(&store, repo, &format!("b{i}"))).collect();
        for cluster in [&cluster_a, &cluster_b] {
            link(&store, repo, cluster[0], cluster[1]);
            link(&store, repo, cluster[1], cluster[2]);
            link(&store, repo, cluster[2], cluster[0]);
        }
        // A hub is needed for anything to clear the degree-2 floor and be
        // reported, but the actual thing under test here is community
        // separation, not the hotspot filter -- lower the bar by adding a
        // single bridge-adjacent hub connected to one member of each
        // cluster only (not every member, unlike the hotspot test above),
        // so it doesn't dominate/merge the two clusters' assignments.
        let bridge = add_symbol(&store, repo, "bridge");
        link(&store, repo, bridge, cluster_a[0]);
        link(&store, repo, bridge, cluster_b[0]);

        let nodes = store.all_nodes(repo).unwrap();
        let edges = store.all_edges(repo).unwrap();
        let mut graph: UnGraph<i64, f64> = UnGraph::new_undirected();
        let mut index_of: HashMap<i64, NodeIndex> = HashMap::new();
        for node in &nodes {
            index_of.insert(node.id, graph.add_node(node.id));
        }
        for edge in &edges {
            graph.add_edge(index_of[&edge.src_id], index_of[&edge.dst_id], edge.weight.max(0.0001));
        }
        let communities = louvain_single_pass(&graph);

        let community_of = |id: i64| communities[&index_of[&id]];
        assert_eq!(community_of(cluster_a[1]), community_of(cluster_a[2]), "cluster a's non-bridge members must share a community");
        assert_eq!(community_of(cluster_b[1]), community_of(cluster_b[2]), "cluster b's non-bridge members must share a community");
        assert_ne!(community_of(cluster_a[1]), community_of(cluster_b[1]), "the two triangles must land in different communities");
    }
}

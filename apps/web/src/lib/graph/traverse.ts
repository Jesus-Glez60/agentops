import type { GraphEdge, GraphNode } from "@/lib/api/types";

/**
 * BFS reachability from `startId` over `edges`, up to `depth` hops,
 * traversed as undirected (an edge's direction is `DependsOn`/`Affects`
 * semantics for display, but "what's within N hops of this node" for
 * exploration purposes doesn't care which way the edge points). Returns
 * the set of node ids within depth, including `startId` itself.
 */
export function neighborsWithinDepth(edges: GraphEdge[], startId: number, depth: number): Set<number> {
  const adjacency = buildAdjacency(edges);
  const visited = new Set<number>([startId]);
  let frontier = [startId];

  for (let hop = 0; hop < depth && frontier.length > 0; hop++) {
    const next: number[] = [];
    for (const id of frontier) {
      for (const neighbor of adjacency.get(id) ?? []) {
        if (!visited.has(neighbor)) {
          visited.add(neighbor);
          next.push(neighbor);
        }
      }
    }
    frontier = next;
  }

  return visited;
}

function buildAdjacency(edges: GraphEdge[]): Map<number, number[]> {
  const adjacency = new Map<number, number[]>();
  for (const edge of edges) {
    if (!adjacency.has(edge.src_id)) adjacency.set(edge.src_id, []);
    if (!adjacency.has(edge.dst_id)) adjacency.set(edge.dst_id, []);
    adjacency.get(edge.src_id)!.push(edge.dst_id);
    adjacency.get(edge.dst_id)!.push(edge.src_id);
  }
  return adjacency;
}

/** Dependency-chain view: DependsOn edges only, Symbol/File nodes only (excludes Gotcha/Decision). */
export function depChainSubgraph(nodes: GraphNode[], edges: GraphEdge[]): { nodes: GraphNode[]; edges: GraphEdge[] } {
  const filteredEdges = edges.filter((e) => e.relation === "DependsOn");
  const filteredNodes = nodes.filter((n) => n.kind === "Symbol" || n.kind === "File");
  const nodeIds = new Set(filteredNodes.map((n) => n.id));
  return { nodes: filteredNodes, edges: filteredEdges.filter((e) => nodeIds.has(e.src_id) && nodeIds.has(e.dst_id)) };
}

/** Knowledge view: Gotcha/Decision nodes plus their one-hop neighbors, via Affects/Documents edges. */
export function knowledgeSubgraph(nodes: GraphNode[], edges: GraphEdge[]): { nodes: GraphNode[]; edges: GraphEdge[] } {
  const knowledgeEdges = edges.filter((e) => e.relation === "Affects" || e.relation === "Documents");
  const knowledgeNodeIds = new Set(nodes.filter((n) => n.kind === "Gotcha" || n.kind === "Decision").map((n) => n.id));
  const neighborIds = new Set<number>(knowledgeNodeIds);
  for (const e of knowledgeEdges) {
    if (knowledgeNodeIds.has(e.src_id)) neighborIds.add(e.dst_id);
    if (knowledgeNodeIds.has(e.dst_id)) neighborIds.add(e.src_id);
  }
  return {
    nodes: nodes.filter((n) => neighborIds.has(n.id)),
    edges: knowledgeEdges.filter((e) => neighborIds.has(e.src_id) && neighborIds.has(e.dst_id)),
  };
}

/** Impact view: everything reachable from `selectedId` via DependsOn/Affects edges, up to `depth` hops. */
export function impactSubgraph(
  nodes: GraphNode[],
  edges: GraphEdge[],
  selectedId: number,
  depth: number,
): { nodes: GraphNode[]; edges: GraphEdge[] } {
  const impactEdges = edges.filter((e) => e.relation === "DependsOn" || e.relation === "Affects");
  const reachable = neighborsWithinDepth(impactEdges, selectedId, depth);
  return {
    nodes: nodes.filter((n) => reachable.has(n.id)),
    edges: impactEdges.filter((e) => reachable.has(e.src_id) && reachable.has(e.dst_id)),
  };
}

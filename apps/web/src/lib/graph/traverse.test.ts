import { describe, expect, it } from "vitest";
import { depChainSubgraph, impactSubgraph, knowledgeSubgraph, neighborsWithinDepth } from "./traverse";
import type { GraphEdge, GraphNode } from "@/lib/api/types";

function node(id: number, kind: GraphNode["kind"], name: string): GraphNode {
  return { id, kind, repo: "r", path: null, name, start_line: null, end_line: null, content: null };
}
function edge(id: number, src_id: number, dst_id: number, relation: GraphEdge["relation"]): GraphEdge {
  return { id, src_id, dst_id, relation };
}

describe("neighborsWithinDepth", () => {
  // 1 -> 2 -> 3 -> 4, a simple chain
  const edges = [edge(1, 1, 2, "DependsOn"), edge(2, 2, 3, "DependsOn"), edge(3, 3, 4, "DependsOn")];

  it("includes only the start node at depth 0", () => {
    expect(neighborsWithinDepth(edges, 1, 0)).toEqual(new Set([1]));
  });

  it("expands one hop at a time", () => {
    expect(neighborsWithinDepth(edges, 1, 1)).toEqual(new Set([1, 2]));
    expect(neighborsWithinDepth(edges, 1, 2)).toEqual(new Set([1, 2, 3]));
  });

  it("traverses edges as undirected", () => {
    // starting from node 4 (only has an incoming edge in the DependsOn direction)
    expect(neighborsWithinDepth(edges, 4, 1)).toEqual(new Set([4, 3]));
  });

  it("stops expanding once the frontier is exhausted", () => {
    expect(neighborsWithinDepth(edges, 1, 10)).toEqual(new Set([1, 2, 3, 4]));
  });
});

describe("depChainSubgraph", () => {
  const nodes = [node(1, "Symbol", "a"), node(2, "File", "b.ts"), node(3, "Gotcha", "watch out")];
  const edges = [edge(1, 1, 2, "DependsOn"), edge(2, 1, 3, "Affects")];

  it("keeps only DependsOn edges and Symbol/File nodes", () => {
    const result = depChainSubgraph(nodes, edges);
    expect(result.nodes.map((n) => n.id).sort()).toEqual([1, 2]);
    expect(result.edges).toEqual([edges[0]]);
  });
});

describe("knowledgeSubgraph", () => {
  const nodes = [node(1, "Symbol", "refreshSession"), node(2, "Gotcha", "token rotation"), node(3, "Symbol", "unrelated")];
  const edges = [edge(1, 2, 1, "Affects"), edge(2, 1, 3, "DependsOn")];

  it("keeps gotcha/decision nodes and their one-hop Affects/Documents neighbors, excludes unrelated nodes", () => {
    const result = knowledgeSubgraph(nodes, edges);
    expect(result.nodes.map((n) => n.id).sort()).toEqual([1, 2]);
    expect(result.edges).toEqual([edges[0]]);
  });
});

describe("impactSubgraph", () => {
  const nodes = [node(1, "Symbol", "a"), node(2, "Symbol", "b"), node(3, "Gotcha", "c"), node(4, "Symbol", "unrelated")];
  const edges = [edge(1, 1, 2, "DependsOn"), edge(2, 2, 3, "Affects"), edge(3, 1, 4, "Documents")];

  it("follows only DependsOn/Affects edges from the selected node within depth", () => {
    const result = impactSubgraph(nodes, edges, 1, 2);
    expect(result.nodes.map((n) => n.id).sort()).toEqual([1, 2, 3]);
    // The Documents edge to node 4 is excluded -- not a DependsOn/Affects relation.
    expect(result.nodes.some((n) => n.id === 4)).toBe(false);
  });
});

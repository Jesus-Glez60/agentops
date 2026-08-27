"use client";

import { useEffect } from "react";
import { forceCenter, forceCollide, forceLink, forceManyBody, forceSimulation, type SimulationNodeDatum } from "d3-force";
import { ReactFlow, ReactFlowProvider, Controls, Background, useNodesState, useEdgesState, type Node as FlowNode, type Edge as FlowEdge } from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import { Scale, TriangleAlert } from "lucide-react";
import type { NodeDetail, SubgraphEdge, SubgraphNode } from "@/lib/api/repos-api";
import { kindLabel } from "@/lib/node-detail-formatting";
import { GraphNode, type GraphFlowNode } from "@/components/graph/graph-node";

interface SimNode extends SimulationNodeDatum {
  id: string;
}

/** Common shape both `SubgraphResponse` (seed-centered BFS) and
 * `RepoGraphResponse` (whole-repo, no seed) reduce to for rendering --
 * `seed_id` is omitted entirely in whole-repo mode, in which case no node
 * is drawn as "the" seed and nothing gets pinned during layout. */
export interface GraphPayload {
  seed_id?: number;
  nodes: SubgraphNode[];
  edges: SubgraphEdge[];
}

const ANNOTATION_OFFSETS: [number, number][] = [
  [170, -70],
  [170, 70],
];

function AnnotationNode({ data }: { data: { icon: "Gotcha" | "Decision"; label: string; relation: string } }) {
  const Icon = data.icon === "Gotcha" ? TriangleAlert : Scale;
  const tone = data.icon === "Gotcha" ? "border-node-gotcha/40 bg-node-gotcha/5 text-node-gotcha" : "border-node-decision/40 bg-node-decision/5 text-node-decision";
  return (
    <div className={`max-w-52 rounded-md border px-2.5 py-2 text-mono-code leading-relaxed ${tone}`}>
      <div className="mb-1 flex items-center gap-1.5 font-semibold uppercase">
        <Icon className="size-3" />
        {data.icon}
      </div>
      <p className="truncate text-ink-300">{data.label}</p>
      <p className="mt-1 text-ink-500">{data.relation}</p>
    </div>
  );
}

const nodeTypes = { graphNode: GraphNode, annotation: AnnotationNode };

function runLayout(subgraph: GraphPayload): Map<string, { x: number; y: number }> {
  const simNodes: SimNode[] = subgraph.nodes.map((n) => ({ id: String(n.id) }));
  const seed = simNodes.find((n) => n.id === String(subgraph.seed_id));
  // Pin the seed at the origin so re-layouts (depth/filter/tab changes)
  // don't visually "swim" the centered node around.
  if (seed) {
    seed.fx = 0;
    seed.fy = 0;
  }
  const simLinks = subgraph.edges.map((e) => ({ source: String(e.src_id), target: String(e.dst_id) }));

  const simulation = forceSimulation(simNodes)
    .force("charge", forceManyBody().strength(-260))
    .force("link", forceLink(simLinks).id((d) => (d as SimNode).id).distance(110))
    .force("center", forceCenter(0, 0))
    .force("collide", forceCollide(46))
    .stop();
  // One-shot static layout, not an animated simulation -- matches the
  // mockup's stable layout and avoids per-frame render churn.
  simulation.tick(300);

  return new Map(simNodes.map((n) => [n.id, { x: n.x ?? 0, y: n.y ?? 0 }]));
}

function GraphCanvasInner({
  subgraph,
  seedDetail,
  onNodeClick,
  onNodeDoubleClick,
}: {
  subgraph: GraphPayload | undefined;
  seedDetail: NodeDetail | undefined;
  onNodeClick: (id: number) => void;
  onNodeDoubleClick: (id: number) => void;
}) {
  const [nodes, setNodes, onNodesChange] = useNodesState<FlowNode>([]);
  const [edges, setEdges, onEdgesChange] = useEdgesState<FlowEdge>([]);

  useEffect(() => {
    if (!subgraph) {
      setNodes([]);
      setEdges([]);
      return;
    }

    const positions = runLayout(subgraph);

    const flowNodes: GraphFlowNode[] = subgraph.nodes.map((n) => ({
      id: String(n.id),
      type: "graphNode",
      position: positions.get(String(n.id)) ?? { x: 0, y: 0 },
      data: {
        kind: n.kind,
        label: n.name ?? n.path ?? `#${n.id}`,
        kindLabel: kindLabel(n.kind),
        prominence: n.prominence,
        isSeed: n.id === subgraph.seed_id,
      },
    }));

    // Floating knowledge annotation cards -- only meaningful with a seed
    // (whole-repo mode has no single node to anchor them to). Capped to 2,
    // positioned near the seed at a fixed offset outside the force
    // simulation. `connected` carries no content/snippet field, only
    // name+relation (confirmed: widening that Rust struct is out of this
    // pass's scope), so these show name + relation, not a truncated body
    // excerpt.
    const seedPos = subgraph.seed_id !== undefined ? positions.get(String(subgraph.seed_id)) : undefined;
    const knowledge = seedPos ? (seedDetail?.connected ?? []).filter((n) => n.kind === "Gotcha" || n.kind === "Decision").slice(0, 2) : [];
    const annotationNodes: FlowNode[] = knowledge.map((n, i) => ({
      id: `annotation-${n.id}`,
      type: "annotation",
      draggable: false,
      selectable: false,
      position: { x: seedPos!.x + ANNOTATION_OFFSETS[i][0], y: seedPos!.y + ANNOTATION_OFFSETS[i][1] },
      data: { icon: n.kind as "Gotcha" | "Decision", label: n.name ?? `#${n.id}`, relation: n.relation.replace(/^←\s*/, "") },
    }));

    const flowEdges: FlowEdge[] = subgraph.edges.map((e) => ({
      id: String(e.id),
      source: String(e.src_id),
      target: String(e.dst_id),
      label: e.label,
      style: e.relation === "Affects" ? { strokeDasharray: "4 3" } : undefined,
    }));

    setNodes([...flowNodes, ...annotationNodes]);
    setEdges(flowEdges);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [subgraph, seedDetail]);

  return (
    <ReactFlow
      nodes={nodes}
      edges={edges}
      onNodesChange={onNodesChange}
      onEdgesChange={onEdgesChange}
      nodeTypes={nodeTypes}
      onNodeClick={(_, node) => !node.id.startsWith("annotation-") && onNodeClick(Number(node.id))}
      onNodeDoubleClick={(_, node) => !node.id.startsWith("annotation-") && onNodeDoubleClick(Number(node.id))}
      // Nodes are laid out by the force simulation, not manually arranged --
      // a stray drag (easy to trigger by mis-clicking while trying to pan)
      // would silently detach a node from that layout with no way back
      // short of re-fetching. Panning the canvas itself is unaffected.
      nodesDraggable={false}
      fitView
      minZoom={0.2}
      maxZoom={2}
    >
      <Background />
      <Controls />
    </ReactFlow>
  );
}

export function GraphCanvas(props: {
  subgraph: GraphPayload | undefined;
  seedDetail: NodeDetail | undefined;
  onNodeClick: (id: number) => void;
  onNodeDoubleClick: (id: number) => void;
}) {
  return (
    <div className="relative flex-1 overflow-hidden rounded-lg border border-border-strong bg-canvas">
      <ReactFlowProvider>
        <GraphCanvasInner {...props} />
      </ReactFlowProvider>
    </div>
  );
}

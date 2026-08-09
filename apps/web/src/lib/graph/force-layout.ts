import { forceCenter, forceCollide, forceLink, forceManyBody, forceSimulation } from "d3-force";
import type { GraphEdge, GraphNode } from "@/lib/api/types";

export interface LayoutPosition {
  x: number;
  y: number;
}

/**
 * One-shot force layout: not react-flow's own (it has none -- see the
 * plan's correction that xyflow's published "Force Layout" example is a
 * paid Pro-only example, so this is a direct integration against the free,
 * standalone d3-force package instead). Runs the simulation synchronously
 * for a fixed number of ticks rather than animating it, since this is a
 * layout computation, not a live physics view.
 */
export function computeForceLayout(nodes: GraphNode[], edges: GraphEdge[]): Map<number, LayoutPosition> {
  if (nodes.length === 0) return new Map();

  // Deliberately NOT seeding x/y here (e.g. to 0,0) -- d3-force only applies
  // its own default initial placement (a deterministic golden-angle spiral,
  // radius growing with node index) when x/y are unset. Every node starting
  // at an identical, defined (0,0) position defeats that: with no links to
  // differentiate them, a set of isolated nodes (same charge, same collide
  // radius, symmetric center-pull) all reach the same equilibrium *radius*
  // from center, just at whatever near-random angle Barnes-Hut's coincident-
  // point jitter happened to nudge them toward -- visually, a perfect ring
  // of barely-visible dots instead of an organically spread cluster.
  const simNodes: { id: number; x?: number; y?: number }[] = nodes.map((n) => ({ id: n.id }));
  const simLinks = edges
    .filter((e) => nodes.some((n) => n.id === e.src_id) && nodes.some((n) => n.id === e.dst_id))
    .map((e) => ({ source: e.src_id, target: e.dst_id }));

  // `distanceMax` on the charge force caps how far mutual repulsion can push
  // two nodes apart -- without it, a large, mostly-disconnected graph (many
  // nodes, few edges, e.g. a whole-monorepo scan) sprawls across an
  // effectively unbounded area, since nothing pulls unconnected nodes back
  // together. That sprawl is what actually made the canvas look empty: with
  // react-flow's default `minZoom` (0.5), fitting a huge bounding box isn't
  // possible, so the viewport ends up zoomed into a mostly-empty region
  // instead of showing the whole (correctly-rendered, just far-flung) graph.
  // Collide radius/link distance are sized against the actual rendered node
  // card (graph-node.tsx: up to ~192px wide, ~60px tall) -- the previous
  // radius of 40 was smaller than half the card's own width, so cards
  // overlapped each other into an unreadable solid mass once zoomed in,
  // even though the overall cluster shape looked fine zoomed out. `theta`
  // trades a bit of Barnes-Hut accuracy for speed, since this is a one-shot
  // layout, not a physically precise simulation.
  const simulation = forceSimulation(simNodes)
    .force("charge", forceManyBody().strength(-260).distanceMax(600).theta(1))
    .force("link", forceLink(simLinks).id((d) => (d as { id: number }).id).distance(170))
    .force("center", forceCenter(0, 0))
    .force("collide", forceCollide(110))
    .stop();

  // Fixed 300-tick loops on a graph with thousands of nodes block the main
  // thread long enough to feel like a hang. d3's own default alphaDecay
  // reaches alphaMin (0.001) in ~300 ticks regardless of graph size -- it's
  // the cost *per tick* that scales with node count, not the tick count
  // itself -- so cutting ticks for large graphs trades a little final
  // settling for a proportionally large cut in blocked time.
  const TICKS = nodes.length > 600 ? 120 : 300;
  for (let i = 0; i < TICKS; i++) simulation.tick();

  const positions = new Map<number, LayoutPosition>();
  for (const n of simNodes) {
    positions.set(n.id, { x: n.x ?? 0, y: n.y ?? 0 });
  }
  return positions;
}

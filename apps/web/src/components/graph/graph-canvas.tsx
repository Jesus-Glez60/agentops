"use client";

import { useEffect, useMemo, useState } from "react";
import { Background, Controls, ReactFlow, ReactFlowProvider, useReactFlow, type Edge, type Node } from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import { computeForceLayout, type LayoutPosition } from "@/lib/graph/force-layout";
import { GraphNodeRenderer, type GraphNodeData } from "@/components/graph/graph-node";
import type { GraphEdge, GraphNode } from "@/lib/api/types";

const EDGE_COLOR: Record<GraphEdge["relation"], string> = {
  DependsOn: "var(--slate-blue)",
  Documents: "var(--teal-400)",
  Affects: "var(--amber-500)",
};

const nodeTypes = { domainNode: GraphNodeRenderer };

/**
 * `fitView` as a prop on <ReactFlow> only runs once, at initial mount --
 * it does NOT re-run when `nodes` changes later (e.g. switching tabs/depth
 * recomputes the filtered set while this component stays mounted). Without
 * this, a later data change can leave the viewport panned/zoomed to fit an
 * earlier (possibly empty or differently-shaped) node set, making real
 * nodes render outside the visible viewport -- indistinguishable from "no
 * nodes rendered" without opening dev tools. Re-fitting whenever the set
 * of node ids actually changes fixes that for every case, not just mount.
 */
function FitViewOnDataChange({ nodeIdsKey }: { nodeIdsKey: string }) {
  const { fitView } = useReactFlow();
  useEffect(() => {
    // Let react-flow finish its own initial-mount fitView/measurement pass first.
    const id = requestAnimationFrame(() => fitView({ duration: 200, padding: 0.2 }));
    return () => cancelAnimationFrame(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [nodeIdsKey]);
  return null;
}

export function GraphCanvas({
  nodes,
  edges,
  selectedId,
  onSelect,
}: {
  nodes: GraphNode[];
  edges: GraphEdge[];
  selectedId: number | null;
  onSelect: (id: number) => void;
}) {
  // Re-run layout only when the filtered node/edge set actually changes
  // (identity via the ids involved), not on every render -- otherwise
  // react-flow's native node dragging would get fought by a re-layout on
  // every selection change.
  const layoutKey = useMemo(() => nodes.map((n) => n.id).join(",") + "|" + edges.map((e) => e.id).join(","), [nodes, edges]);

  // computeForceLayout is a synchronous, potentially expensive (thousands of
  // nodes) computation. Running it inside useMemo during render blocks the
  // very click (tab switch, filter toggle) that triggered it -- the browser
  // can't paint anything, including a "loading" state, until it returns,
  // which is what reads as the app hanging. Deferring it one frame via
  // requestAnimationFrame lets React commit first (so e.g. the active tab
  // highlight updates immediately), then run the heavy work, so at worst the
  // canvas itself briefly shows stale/empty content instead of the whole
  // page feeling frozen.
  const [positions, setPositions] = useState<Map<number, LayoutPosition>>(() => new Map());
  const [isLayingOut, setIsLayingOut] = useState(true);

  useEffect(() => {
    let cancelled = false;
    // Legitimate "synchronize with an external system" use of an effect (the
    // external system being the layout computation kicked off below, not
    // local component state) -- not the anti-pattern this rule otherwise
    // warns about.
    // eslint-disable-next-line react-hooks/set-state-in-effect
    setIsLayingOut(true);
    const id = requestAnimationFrame(() => {
      if (cancelled) return;
      const next = computeForceLayout(nodes, edges);
      if (!cancelled) {
        setPositions(next);
        setIsLayingOut(false);
      }
    });
    return () => {
      cancelled = true;
      cancelAnimationFrame(id);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [layoutKey]);

  const flowNodes: Node<GraphNodeData>[] = useMemo(
    () =>
      nodes.map((n) => ({
        id: String(n.id),
        type: "domainNode",
        position: positions.get(n.id) ?? { x: 0, y: 0 },
        data: { node: n, selected: n.id === selectedId },
      })),
    [nodes, positions, selectedId],
  );

  const flowEdges: Edge[] = useMemo(
    () =>
      edges.map((e) => ({
        id: String(e.id),
        source: String(e.src_id),
        target: String(e.dst_id),
        type: "smoothstep",
        style: { stroke: EDGE_COLOR[e.relation] },
        label: e.relation,
      })),
    [edges],
  );

  return (
    // Explicit `relative` + `absolute inset-0` on the ReactFlow wrapper,
    // rather than relying on `h-full` through a chain of flex-stretch
    // ancestors -- react-flow measures its container via ResizeObserver at
    // mount, and a 0-height measurement during any transient layout state
    // up that chain silently leaves it thinking there's no room to render
    // into, with no error, just an empty canvas.
    <div className="relative h-full w-full">
      <div className="absolute inset-0">
        <ReactFlowProvider>
          <ReactFlow
            nodes={flowNodes}
            edges={flowEdges}
            nodeTypes={nodeTypes}
            onNodeClick={(_, node) => onSelect(Number(node.id))}
            fitView
            // Defense in depth alongside capping the force layout's spread
            // (force-layout.ts): react-flow's default minZoom (0.5) can
            // make fitView physically unable to show a large/sparse graph
            // at all -- better to let it zoom out further than to silently
            // clip the view to an empty region.
            minZoom={0.02}
            colorMode="dark"
            proOptions={{ hideAttribution: true }}
          >
            <Background />
            <Controls />
            <FitViewOnDataChange nodeIdsKey={layoutKey} />
          </ReactFlow>
        </ReactFlowProvider>
        {isLayingOut && (
          <div className="pointer-events-none absolute top-3 right-3 rounded-md border border-border-strong bg-panel px-2.5 py-1 text-mono-path text-ink-300 shadow-sm">
            Laying out graph…
          </div>
        )}
      </div>
    </div>
  );
}

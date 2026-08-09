"use client";

import { Suspense, useMemo, useState } from "react";
import { useSearchParams } from "next/navigation";
import useSWR from "swr";
import { getGraph } from "@/lib/api/agentops-api";
import { useRepoPathParam } from "@/lib/use-repo-path-param";
import { depChainSubgraph, impactSubgraph, knowledgeSubgraph } from "@/lib/graph/traverse";
import type { GraphEdge, GraphNode, NodeKind } from "@/lib/api/types";
import { GraphCanvas } from "@/components/graph/graph-canvas";
import { NodeDetailPanel } from "@/components/graph/node-detail-panel";
import { ErrorState } from "@/components/shared/error-state";
import { EmptyState } from "@/components/shared/empty-state";
import { RepoSelect } from "@/components/shared/repo-select";
import { Skeleton } from "@/components/ui/skeleton";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Checkbox } from "@/components/ui/checkbox";
import { Slider } from "@/components/ui/slider";
import { Workflow } from "lucide-react";

type GraphTab = "local" | "dep-chain" | "knowledge" | "impact";

const NODE_KINDS: NodeKind[] = ["Symbol", "File", "Gotcha", "Decision"];
const GRAPH_TABS: GraphTab[] = ["local", "dep-chain", "knowledge", "impact"];

function GraphPageInner() {
  const { path, setPath } = useRepoPathParam();
  const searchParams = useSearchParams();
  // Deep-linkable from Overview's stat cards (e.g. "Gotchas requiring
  // review" -> ?tab=knowledge&kind=Gotcha) -- read once at mount, same
  // pattern as useRepoPathParam's initial-value read.
  const initialTabParam = searchParams.get("tab");
  const initialTab: GraphTab = GRAPH_TABS.includes(initialTabParam as GraphTab) ? (initialTabParam as GraphTab) : "local";
  const initialKindParam = searchParams.get("kind");
  const initialKinds =
    initialKindParam && (NODE_KINDS as string[]).includes(initialKindParam) ? new Set([initialKindParam as NodeKind]) : new Set(NODE_KINDS);
  const [tab, setTab] = useState<GraphTab>(initialTab);
  const [visibleKinds, setVisibleKinds] = useState<Set<NodeKind>>(initialKinds);
  const [depth, setDepth] = useState(2);
  // Deep-linkable from the Documentation page's symbol browser ("View in
  // Knowledge Graph" on a specific symbol) -- ?node=<id>, paired with
  // ?tab=impact to land directly on that symbol's impact subgraph instead
  // of an empty "select a node" state.
  const initialNodeParam = searchParams.get("node");
  const initialSelectedId = initialNodeParam ? Number(initialNodeParam) : null;
  const [selectedId, setSelectedId] = useState<number | null>(
    initialSelectedId != null && !Number.isNaN(initialSelectedId) ? initialSelectedId : null,
  );

  const { data, error, isLoading } = useSWR(path ? ["graph", path] : null, () => getGraph(path));

  const allNodes = useMemo(() => data?.nodes ?? [], [data]);
  const allEdges = useMemo(() => data?.edges ?? [], [data]);
  const nodesById = useMemo(() => new Map(allNodes.map((n) => [n.id, n])), [allNodes]);

  const tabbed = useMemo((): { nodes: GraphNode[]; edges: GraphEdge[] } => {
    if (tab === "dep-chain") return depChainSubgraph(allNodes, allEdges);
    if (tab === "knowledge") return knowledgeSubgraph(allNodes, allEdges);
    if (tab === "impact") {
      if (selectedId == null) return { nodes: [], edges: [] };
      return impactSubgraph(allNodes, allEdges, selectedId, depth);
    }
    return { nodes: allNodes, edges: allEdges };
  }, [tab, allNodes, allEdges, selectedId, depth]);

  const visible = useMemo(() => {
    const nodes = tabbed.nodes.filter((n) => visibleKinds.has(n.kind));
    const nodeIds = new Set(nodes.map((n) => n.id));
    return { nodes, edges: tabbed.edges.filter((e) => nodeIds.has(e.src_id) && nodeIds.has(e.dst_id)) };
  }, [tabbed, visibleKinds]);

  const selectedNode = selectedId != null ? nodesById.get(selectedId) : undefined;

  function toggleKind(kind: NodeKind) {
    setVisibleKinds((prev) => {
      const next = new Set(prev);
      if (next.has(kind)) next.delete(kind);
      else next.add(kind);
      return next;
    });
  }

  return (
    <div className="flex h-[calc(100vh-8rem)] flex-col gap-4">
      <div className="flex items-center justify-between gap-4">
        <h1 className="text-page-title font-bold">Knowledge Graph</h1>
        <RepoSelect value={path} onChange={setPath} placeholder="Select a repo…" />
      </div>

      {!path && <EmptyState icon={Workflow} title="No repo selected" description="Select a scanned repo above to browse its graph." />}

      {path && error && <ErrorState message={error instanceof Error ? error.message : String(error)} />}

      {path && isLoading && <Skeleton className="h-96 w-full" />}

      {path && !isLoading && !error && data && (
        <>
          <Tabs value={tab} onValueChange={(v) => setTab(v as GraphTab)}>
            <TabsList>
              <TabsTrigger value="local">Local graph</TabsTrigger>
              <TabsTrigger value="dep-chain">Dep. chain</TabsTrigger>
              <TabsTrigger value="knowledge">Knowledge</TabsTrigger>
              <TabsTrigger value="impact" disabled={selectedId == null}>
                Impact
              </TabsTrigger>
            </TabsList>
          </Tabs>

          <div className="flex min-h-0 flex-1 gap-4">
            <aside className="w-52 shrink-0 space-y-4 rounded-md border border-border-strong bg-panel p-3">
              <div>
                <p className="mb-2 text-label uppercase tracking-wide text-ink-500">Node types</p>
                <div className="space-y-2">
                  {NODE_KINDS.map((kind) => (
                    <label key={kind} className="flex items-center gap-2 text-body text-ink-300">
                      <Checkbox checked={visibleKinds.has(kind)} onCheckedChange={() => toggleKind(kind)} />
                      {kind}
                    </label>
                  ))}
                </div>
              </div>
              {tab === "impact" && (
                <div>
                  <p className="mb-2 flex items-center justify-between text-label uppercase tracking-wide text-ink-500">
                    Depth <span className="text-ink-300">{depth}</span>
                  </p>
                  <Slider value={[depth]} onValueChange={([v]) => setDepth(v)} min={1} max={5} step={1} />
                </div>
              )}
            </aside>

            <div className="min-w-0 flex-1 rounded-md border border-border-strong bg-raised">
              {visible.nodes.length === 0 ? (
                <EmptyState
                  icon={Workflow}
                  title={tab === "impact" && selectedId == null ? "Select a node to see its impact" : "No nodes match the current filters"}
                />
              ) : (
                <GraphCanvas nodes={visible.nodes} edges={visible.edges} selectedId={selectedId} onSelect={setSelectedId} />
              )}
            </div>

            {selectedNode && (
              <aside className="w-72 shrink-0 rounded-md border border-border-strong bg-panel">
                <NodeDetailPanel node={selectedNode} edges={allEdges} nodesById={nodesById} onSelect={setSelectedId} />
              </aside>
            )}
          </div>
        </>
      )}
    </div>
  );
}

export default function GraphPage() {
  return (
    <Suspense fallback={<Skeleton className="h-96 w-full" />}>
      <GraphPageInner />
    </Suspense>
  );
}

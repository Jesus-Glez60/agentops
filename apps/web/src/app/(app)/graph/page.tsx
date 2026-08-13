"use client";

import { Suspense, useState } from "react";
import { useSearchParams } from "next/navigation";
import useSWR from "swr";
import {
  getNodeDetail,
  getRepoGraph,
  getRepos,
  getSubgraph,
  REPOS_SWR_KEY,
  type ConnectedNode,
  type GraphMode,
  type NodeKind,
} from "@/lib/api/agentops-api";
import { GraphHeader } from "@/components/graph/graph-header";
import { GraphFilterPanel, GRAPH_FILTERABLE_KINDS } from "@/components/graph/graph-filter-panel";
import { GraphCanvas } from "@/components/graph/graph-canvas";
import { GraphDetailPanel } from "@/components/graph/graph-detail-panel";
import { GraphStatusCaption } from "@/components/graph/graph-status-caption";
import { RepoPicker } from "@/components/graph/repo-picker";
import { Sheet, SheetContent, SheetTitle } from "@/components/ui/sheet";

interface SeedRef {
  repo: string;
  id: number;
}

export default function GraphPage() {
  // useSearchParams requires a Suspense boundary during static generation.
  return (
    <Suspense fallback={null}>
      <GraphPageInner />
    </Suspense>
  );
}

function GraphPageInner() {
  const searchParams = useSearchParams();
  const initialRepo = searchParams.get("repo");
  const initialNode = searchParams.get("node");

  // `repoName` and `seedId` are independent: a repo can be picked with no
  // seed yet (whole-repo view, every node/edge in the repo), or a seed can
  // be centered within it (the existing BFS local-graph view).
  const [repoName, setRepoName] = useState<string | null>(initialRepo);
  const [seedId, setSeedId] = useState<number | null>(initialRepo && initialNode ? Number(initialNode) : null);
  const [mode, setMode] = useState<GraphMode>("local");
  const [depth, setDepth] = useState(2);
  const [kinds, setKinds] = useState<NodeKind[]>([]);
  // Decoupled from the seed -- clicking a node just inspects it in the
  // right panel; re-centering the graph is a separate explicit action
  // ("Center here"), so a single click doesn't disorient the layout.
  const [selected, setSelected] = useState<SeedRef | null>(repoName && seedId ? { repo: repoName, id: seedId } : null);

  const seed: SeedRef | null = repoName && seedId ? { repo: repoName, id: seedId } : null;

  const { data: seedSubgraph } = useSWR(
    seed ? ["subgraph", seed.repo, seed.id, mode, depth, kinds] : null,
    () => getSubgraph(seed!.repo, seed!.id, mode, { depth, kinds }),
  );
  const { data: repoGraph } = useSWR(repoName && !seed ? ["repo-graph", repoName, kinds] : null, () => getRepoGraph(repoName!, kinds));
  const graphData = seed ? seedSubgraph : repoGraph;

  const { data: detail } = useSWR(selected ? ["node", selected.repo, selected.id] : null, () => getNodeDetail(selected!.repo, selected!.id));
  const { data: seedDetail } = useSWR(seed ? ["node", seed.repo, seed.id] : null, () => getNodeDetail(seed!.repo, seed!.id));
  const { data: allRepos } = useSWR(REPOS_SWR_KEY, getRepos);
  const branch = repoName ? allRepos?.find((r) => r.name === repoName)?.branch : undefined;

  function toggleKind(kind: NodeKind) {
    setKinds((prev) => {
      // Empty means "all" -- unchecking one for the first time must expand
      // to the explicit "everything except this one" set, not silently do
      // nothing (there's nothing in an empty array to remove).
      const current = prev.length === 0 ? GRAPH_FILTERABLE_KINDS : prev;
      return current.includes(kind) ? current.filter((k) => k !== kind) : [...current, kind];
    });
  }

  function changeRepo(repo: string) {
    setRepoName(repo);
    setSeedId(null);
    setSelected(null);
  }

  function selectNodeById(id: number) {
    if (!repoName) return;
    setSelected({ repo: repoName, id });
  }

  function selectConnectedNode(node: ConnectedNode) {
    if (!repoName) return;
    setSelected({ repo: repoName, id: node.id });
  }

  function centerOnSelected() {
    if (selected) setSeedId(selected.id);
  }

  if (!repoName) {
    return (
      <div className="flex h-full flex-col">
        <div className="flex h-[52px] shrink-0 items-center border-b border-border-strong px-5 text-section font-medium text-ink-100">Knowledge Graph</div>
        <div className="mx-auto flex w-full max-w-md flex-1 flex-col justify-center gap-3">
          <div className="flex flex-col items-center gap-1 text-center">
            <p className="text-subheading text-ink-100">Pick a repository</p>
            <p className="text-body text-ink-500">See every node and edge in a repo, or search for a specific one to center on.</p>
          </div>
          <RepoPicker onSelect={changeRepo} />
        </div>
      </div>
    );
  }

  return (
    <div className="flex h-full flex-col">
      <GraphHeader
        seedLabel={seed ? (seedDetail?.name ?? seedDetail?.path ?? null) : null}
        mode={seed ? mode : null}
        onModeChange={setMode}
        repo={repoName}
        onChangeRepo={changeRepo}
      />

      <div className="flex min-h-0 flex-1 gap-4 overflow-hidden p-4">
        <GraphFilterPanel kinds={kinds} onToggleKind={toggleKind} depth={depth} onDepthChange={setDepth} showDepth={seed !== null} />

        <div className="relative flex min-h-0 flex-1">
          <GraphCanvas subgraph={graphData} seedDetail={seedDetail} onNodeClick={selectNodeById} onNodeDoubleClick={(id) => setSeedId(id)} />
          <GraphStatusCaption repo={repoName} branch={branch} mode={seed ? mode : null} depth={seed ? depth : null} />
          {graphData?.truncated && (
            <div className="pointer-events-none absolute left-1/2 top-3 -translate-x-1/2 rounded-md border border-curation-needs-curation/40 bg-canvas/90 px-3 py-1 text-mono-code text-curation-needs-curation">
              Showing the first {graphData.nodes.length} nodes — narrow with filters{seed ? " or depth" : ""} to see more.
            </div>
          )}
        </div>
      </div>

      {/* A drawer, not a side-by-side panel -- fixed-positioned and
          overlaying the graph rather than sharing the flex row with it, so
          a long detail body scrolls entirely within its own area and can
          never push the page itself into growing past the viewport (the
          canvas + filter row above already has a fixed, non-scrolling
          height on its own). Wider than the old inline panel so content
          "breathes" the same way the search page's detail pane does. */}
      <Sheet open={selected !== null} onOpenChange={(open) => !open && setSelected(null)}>
        {/* `data-[side=right]:` prefix required to match (and win the
            cascade against) SheetContent's own `data-[side=right]:sm:max-w-sm`
            -- a bare `sm:max-w-3xl` has a different modifier stack, so
            tailwind-merge doesn't dedupe it and the higher-specificity
            base class still wins, silently keeping the old width. */}
        <SheetContent side="right" showCloseButton={false} className="w-full overflow-y-auto data-[side=right]:sm:max-w-3xl">
          <SheetTitle className="sr-only">{detail?.name ?? detail?.path ?? "Node details"}</SheetTitle>
          {selected && (
            <GraphDetailPanel
              detail={detail}
              branch={branch}
              isSeed={seed !== null && selected.id === seed.id}
              onSelectConnected={selectConnectedNode}
              onClose={() => setSelected(null)}
              onCenter={centerOnSelected}
            />
          )}
        </SheetContent>
      </Sheet>
    </div>
  );
}

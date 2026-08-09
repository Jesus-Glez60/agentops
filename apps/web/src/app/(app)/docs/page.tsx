"use client";

import { Suspense, useMemo, useState } from "react";
import useSWR from "swr";
import { BookOpen, FolderTree, Lightbulb, TriangleAlert } from "lucide-react";
import { getDocs, getGraph, getRepos } from "@/lib/api/agentops-api";
import { useRepoPathParam } from "@/lib/use-repo-path-param";
import { extractToc } from "@/lib/markdown-toc";
import { parseDocgenMarkdown } from "@/lib/docgen-doc";
import { relativeTimeFromUnixSeconds } from "@/lib/relative-time";
import type { GraphNode } from "@/lib/api/types";
import { DocContent } from "@/components/docs/doc-content";
import { DocToc } from "@/components/docs/doc-toc";
import { SymbolBrowser } from "@/components/docs/symbol-browser";
import { ErrorState } from "@/components/shared/error-state";
import { EmptyState } from "@/components/shared/empty-state";
import { RepoSelect } from "@/components/shared/repo-select";
import { NotePreviewCard } from "@/components/shared/note-preview-card";
import { NodeDetailDialog } from "@/components/shared/node-detail-dialog";
import { Skeleton } from "@/components/ui/skeleton";
import { cn } from "@/lib/utils";

const SECTION_NAV = [
  { id: "overview", label: "Overview" },
  { id: "repository-map", label: "Repository map" },
  { id: "known-gotchas", label: "Known gotchas" },
  { id: "decisions", label: "Decisions" },
] as const;

function DocsPageInner() {
  const { path, setPath } = useRepoPathParam();

  const { data, error, isLoading } = useSWR(path ? ["docs", path] : null, () => getDocs(path));
  // Same cache key ("graph", path) the Knowledge Graph page uses -- SWR
  // dedupes this into one request if both pages have been visited, and this
  // is the source of the real Affects edges (Gotcha/Decision -> Symbol) that
  // make the repo browser below actually clickable/connected, instead of
  // re-deriving a weaker approximation from docgen's flat markdown text.
  const { data: graphData } = useSWR(path ? ["graph", path] : null, () => getGraph(path));
  const { data: repos } = useSWR("repos", getRepos);
  const repoSummary = repos?.find((r) => r.path === path);

  const [openNote, setOpenNote] = useState<GraphNode | null>(null);

  const parsed = useMemo(() => (data ? parseDocgenMarkdown(data.content) : null), [data]);
  // Fallback TOC only used when parsing fails (unexpected shape) and the raw
  // markdown dump renders instead -- the structured view below builds its
  // own fixed section nav rather than one entry per heading.
  const fallbackToc = useMemo(() => (data && !parsed ? extractToc(data.content) : []), [data, parsed]);

  const gotchaNodes = useMemo(() => graphData?.nodes.filter((n) => n.kind === "Gotcha") ?? [], [graphData]);
  const decisionNodes = useMemo(() => graphData?.nodes.filter((n) => n.kind === "Decision") ?? [], [graphData]);
  const affectsTargetName = useMemo(() => {
    if (!graphData) return new Map<number, string>();
    const nodesById = new Map(graphData.nodes.map((n) => [n.id, n]));
    const map = new Map<number, string>();
    for (const e of graphData.edges) {
      if (e.relation !== "Affects") continue;
      const target = nodesById.get(e.dst_id);
      if (target?.name) map.set(e.src_id, target.name);
    }
    return map;
  }, [graphData]);

  return (
    <div className="flex flex-col gap-4">
      <div className="flex items-center justify-between gap-4">
        <h1 className="text-page-title font-bold">Documentation</h1>
        <RepoSelect value={path} onChange={setPath} placeholder="Select a repo…" />
      </div>

      {!path && (
        <EmptyState icon={BookOpen} title="No repo selected" description="Select a scanned repo above to view its generated docs." />
      )}

      {path && error && (
        <ErrorState
          message={error instanceof Error ? error.message : String(error)}
          title="No onboarding doc found"
        />
      )}

      {path && isLoading && (
        <div className="space-y-3">
          <Skeleton className="h-8 w-64" />
          <Skeleton className="h-4 w-full" />
          <Skeleton className="h-4 w-full" />
          <Skeleton className="h-4 w-3/4" />
        </div>
      )}

      {path && !isLoading && !error && data && parsed && (
        <div className="flex gap-6">
          <aside className="hidden w-44 shrink-0 xl:block">
            <nav className="sticky top-6 space-y-1">
              <p className="mb-2 text-label uppercase tracking-wide text-ink-500">On this page</p>
              {SECTION_NAV.map((s) => {
                const count = s.id === "known-gotchas" ? gotchaNodes.length : s.id === "decisions" ? decisionNodes.length : undefined;
                if (count === 0) return null;
                return (
                  <a key={s.id} href={`#${s.id}`} className="flex items-center justify-between text-body text-ink-300 hover:text-ink-100">
                    {s.label}
                    {count !== undefined && <span className="text-mono-path text-ink-500">{count}</span>}
                  </a>
                );
              })}
            </nav>
          </aside>

          <div className="min-w-0 flex-1 space-y-8">
            <div id="overview">
              <p className="text-mono-path text-ink-500">{parsed.subtitle}</p>
              <h2 className="mt-1 text-page-title font-bold text-ink-100">{parsed.title}</h2>
              <div className="mt-3 flex flex-wrap gap-4">
                {parsed.stats.map((s) => (
                  <div key={s.label} className="text-body">
                    <span className="text-ink-100">{s.value}</span> <span className="text-ink-500">{s.label}</span>
                  </div>
                ))}
              </div>
            </div>

            {graphData && (
              <section id="repository-map" className="scroll-mt-6">
                <h3 className="mb-3 flex items-center gap-2 text-subheading font-semibold text-ink-100">
                  <FolderTree className="size-4 text-ink-500" />
                  Repository map
                </h3>
                <p className="mb-3 text-body text-ink-300">
                  Files with the most attached gotchas/decisions first -- click a symbol to see what&apos;s connected to it.
                </p>
                <SymbolBrowser nodes={graphData.nodes} edges={graphData.edges} repoPath={path} />
              </section>
            )}

            {gotchaNodes.length > 0 && (
              <section id="known-gotchas" className="scroll-mt-6">
                <h3 className="mb-3 flex items-center gap-2 text-subheading font-semibold text-ink-100">
                  <TriangleAlert className="size-4 text-node-gotcha" />
                  Known gotchas
                </h3>
                <div className="space-y-2">
                  {gotchaNodes.map((note) => (
                    <NotePreviewCard key={note.id} node={note} affectsLabel={affectsTargetName.get(note.id)} onOpen={() => setOpenNote(note)} />
                  ))}
                </div>
              </section>
            )}

            {decisionNodes.length > 0 && (
              <section id="decisions" className="scroll-mt-6">
                <h3 className="mb-3 flex items-center gap-2 text-subheading font-semibold text-ink-100">
                  <Lightbulb className="size-4 text-node-decision" />
                  Decisions
                </h3>
                <div className="space-y-2">
                  {decisionNodes.map((note) => (
                    <NotePreviewCard key={note.id} node={note} affectsLabel={affectsTargetName.get(note.id)} onOpen={() => setOpenNote(note)} />
                  ))}
                </div>
              </section>
            )}
          </div>

          <aside className="hidden w-56 shrink-0 lg:block">
            <div className="sticky top-6 space-y-3 rounded-md border border-border-strong bg-panel p-3">
              <p className="text-label uppercase tracking-wide text-ink-500">Repository context</p>
              <dl className="space-y-1.5 text-body">
                <div className="flex justify-between gap-2">
                  <dt className="text-ink-500">Path</dt>
                  <dd className={cn("truncate text-mono-path text-ink-100")} title={path}>
                    {path.split("/").filter(Boolean).pop()}
                  </dd>
                </div>
                {repoSummary && (
                  <div className="flex justify-between gap-2">
                    <dt className="text-ink-500">Last scanned</dt>
                    <dd className="text-ink-100">{relativeTimeFromUnixSeconds(repoSummary.last_scanned_at)}</dd>
                  </div>
                )}
                {repoSummary?.counts && (
                  <>
                    <div className="flex justify-between gap-2">
                      <dt className="text-ink-500">Files</dt>
                      <dd className="text-ink-100">{repoSummary.counts.files}</dd>
                    </div>
                    <div className="flex justify-between gap-2">
                      <dt className="text-ink-500">Symbols</dt>
                      <dd className="text-ink-100">{repoSummary.counts.symbols}</dd>
                    </div>
                  </>
                )}
              </dl>
            </div>
          </aside>
        </div>
      )}

      {/* Fallback: content that isn't agentops-docgen's expected shape (parseDocgenMarkdown returned null) still renders, just without the structured browser/nav. */}
      {path && !isLoading && !error && data && !parsed && (
        <div className="flex gap-6">
          <DocContent markdown={data.content} />
          <aside className="hidden w-56 shrink-0 lg:block">
            <div className="sticky top-6">
              <DocToc entries={fallbackToc} />
            </div>
          </aside>
        </div>
      )}

      <NodeDetailDialog node={openNote} open={openNote != null} onOpenChange={(o) => !o && setOpenNote(null)} />
    </div>
  );
}

export default function DocsPage() {
  return (
    <Suspense fallback={<Skeleton className="h-96 w-full" />}>
      <DocsPageInner />
    </Suspense>
  );
}

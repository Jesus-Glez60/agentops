"use client";

import { Suspense, useEffect, useState } from "react";
import { useRouter, useSearchParams } from "next/navigation";
import useSWR from "swr";
import { SearchX } from "lucide-react";
import { getNodeDetail, getRepos, REPOS_SWR_KEY, search, type ConnectedNode, type NodeKind } from "@/lib/api/repos-api";
import { getRecentSearches, pushRecentSearch } from "@/lib/recent-searches";
import { kindLabel } from "@/lib/node-detail-formatting";
import { displayRepoName } from "@/lib/utils";
import { nodeKindShape } from "@/lib/node-kind-shapes";
import { Button } from "@/components/ui/button";
import { NavDot } from "@/components/shell/nav-dot";
import { SearchFilters } from "@/components/search/search-filters";
import { ScopeSelector } from "@/components/search/scope-selector";
import { SearchResultCard } from "@/components/search/search-result-card";
import { CopyButton } from "@/components/shared/copy-button";
import { EmptyState } from "@/components/shared/empty-state";
import { NodeDetailSections } from "@/components/shared/node-detail-sections";

// What the detail panel needs to render its header immediately on click,
// before the `getNodeDetail` fetch resolves -- built from either a
// SearchResult (clicking a result card) or a ConnectedNode (clicking a row
// in the detail panel itself, which is always in the same repo as the node
// it's attached to, since GraphStore::edges_from/edges_to never cross repos).
interface NodePreview {
  repo: string;
  id: number;
  kind: NodeKind;
  name: string | null;
  path: string | null;
}

const EXAMPLE_QUESTIONS = [
  "How does authentication work in this codebase?",
  "What gotchas exist around database migrations?",
  "Where is the retry logic for failed requests?",
  "What decisions were made about session handling?",
];

export default function SearchPage() {
  // useSearchParams requires a Suspense boundary during static generation --
  // the actual page content lives in SearchPageInner below.
  return (
    <Suspense fallback={null}>
      <SearchPageInner />
    </Suspense>
  );
}

function SearchPageInner() {
  const router = useRouter();
  const searchParams = useSearchParams();
  const [query, setQuery] = useState("");
  const [submittedQuery, setSubmittedQuery] = useState<string | null>(null);
  const [kinds, setKinds] = useState<NodeKind[]>([]);
  const [repoScope, setRepoScope] = useState<string[]>([]);
  const [selected, setSelected] = useState<NodePreview | null>(null);
  const [recent, setRecent] = useState<string[]>([]);

  // Deferred to an effect -- localStorage isn't available during SSR, and
  // reading it during render would produce a hydration mismatch. This is a
  // one-time read of external state on mount, not a cascading-render loop.
  useEffect(() => {
    // eslint-disable-next-line react-hooks/set-state-in-effect
    setRecent(getRecentSearches());
  }, []);

  // A pre-filled query via `?q=` -- e.g. the graph screen's "Search" action
  // button links here instead of duplicating search UI. Runs once on mount
  // only (empty deps): editing the input afterward shouldn't keep getting
  // clobbered by the URL param.
  useEffect(() => {
    const q = searchParams.get("q");
    if (q?.trim()) {
      runSearch(q);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const { data: results, isLoading } = useSWR(
    submittedQuery ? ["search", submittedQuery, repoScope, kinds] : null,
    () => search(submittedQuery as string, { repos: repoScope, kinds }),
  );

  const { data: detail, error: detailError, mutate: mutateDetail } = useSWR(selected ? ["node", selected.repo, selected.id] : null, () => getNodeDetail(selected!.repo, selected!.id));

  // Same SWR key the Overview page/topbar/scope selector already share --
  // used here only to show a node's real branch, not a fabricated one.
  const { data: allRepos } = useSWR(REPOS_SWR_KEY, getRepos);
  const branch = selected ? allRepos?.connections.find((r) => r.id === selected.repo)?.branch : undefined;

  function runSearch(q: string) {
    const trimmed = q.trim();
    if (!trimmed) return;
    setQuery(trimmed);
    setSubmittedQuery(trimmed);
    setSelected(null);
    setRecent(pushRecentSearch(trimmed));
  }

  function selectConnectedNode(node: ConnectedNode) {
    if (!selected) return;
    setSelected({ repo: selected.repo, id: node.id, kind: node.kind, name: node.name, path: node.path });
  }

  function toggleKind(kind: NodeKind) {
    setKinds((prev) => (prev.includes(kind) ? prev.filter((k) => k !== kind) : [...prev, kind]));
  }

  const hasSubmitted = submittedQuery !== null;
  const scopeLabel = repoScope.length === 0 ? "all repositories" : repoScope.length === 1 ? repoScope[0] : `${repoScope.length} repositories`;
  const selectedShape = selected ? nodeKindShape(selected.kind) : null;

  return (
    <div className="flex h-full flex-col overflow-hidden">
      {/* Header/hero band -- glow only shown pre-submit (the prototype's own
          `sHeroPad` shrinks this band once results are showing, but keeps
          the search bar + filters persistent across both states). */}
      <div className="relative shrink-0 overflow-hidden border-b bg-canvas">
        {!hasSubmitted && (
          <div
            aria-hidden
            className="pointer-events-none absolute inset-0"
            style={{
              background: "radial-gradient(ellipse 50% 110% at 50% -40%, rgba(137,180,250,0.24), transparent 70%), radial-gradient(ellipse 30% 80% at 15% -10%, rgba(203,166,247,0.14), transparent 70%)",
            }}
          />
        )}
        <div className={`relative mx-auto flex max-w-[980px] flex-col gap-[18px] ${hasSubmitted ? "px-8 py-5" : "px-8 py-14"}`}>
          {!hasSubmitted && (
            <div className="flex flex-col items-center gap-3 text-center">
              <span className="font-mono text-[12px] uppercase tracking-wide text-[color:var(--blue-500)]">Hybrid search · dense + BM25 + exact name + graph</span>
              <h1 className="text-[clamp(34px,4vw,52px)] font-extrabold leading-[1.02] tracking-[-0.04em] text-ink-100">Ask about your codebase</h1>
              <p className="max-w-[36em] text-[17px] text-ink-300">Plain questions work. Results come back with the source, the gotchas attached to it, and why each one matched.</p>
            </div>
          )}

          <form
            onSubmit={(e) => {
              e.preventDefault();
              runSearch(query);
            }}
            className="flex items-center gap-2.5 rounded-full border border-border-strong bg-raised py-2 pl-5 pr-2 focus-within:shadow-[0_0_0_4px_rgba(137,180,250,0.08)]"
          >
            <span className="size-[9px] shrink-0 rounded-full bg-[color:var(--blue-500)]" />
            <input
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="How does hybrid search rank results?"
              className="min-w-0 flex-1 bg-transparent py-2 text-[17px] text-ink-100 outline-none placeholder:text-ink-500"
            />
            <Button type="submit" size="cta" disabled={!query.trim()}>
              Search
            </Button>
          </form>

          <div className="flex flex-wrap items-center justify-center gap-2">
            <SearchFilters selected={kinds} onToggle={toggleKind} />
            <ScopeSelector selected={repoScope} onChange={setRepoScope} />
            <span className="ml-1 text-[13px] text-ink-500">in {scopeLabel}</span>
          </div>

          {!hasSubmitted && (
            <div className="mt-2 grid grid-cols-1 gap-2.5 sm:grid-cols-2">
              {EXAMPLE_QUESTIONS.map((q) => (
                <button
                  key={q}
                  type="button"
                  onClick={() => runSearch(q)}
                  className="rounded-[14px] border border-border-strong bg-panel/80 p-4 text-left text-[14.5px] leading-snug text-ink-300 transition-colors hover:border-[color:var(--blue-500)] hover:text-ink-100"
                >
                  {q}
                </button>
              ))}
            </div>
          )}

          {!hasSubmitted && recent.length > 0 && (
            <div className="flex flex-col gap-1.5">
              <p className="text-center text-section text-ink-500">Recent searches</p>
              <div className="flex flex-wrap justify-center gap-2">
                {recent.map((q) => (
                  <button
                    key={q}
                    type="button"
                    onClick={() => runSearch(q)}
                    className="rounded-md border border-border-strong px-2.5 py-1 text-mono-code text-ink-300 hover:text-ink-100"
                  >
                    {q}
                  </button>
                ))}
              </div>
            </div>
          )}
        </div>
      </div>

      {hasSubmitted && (
        <div className="flex min-h-0 flex-1 overflow-hidden">
          <div className="flex w-full max-w-md flex-col gap-2 overflow-y-auto p-4">
            {isLoading && <p className="text-body text-ink-500">Searching…</p>}
            {!isLoading && results?.length === 0 && (
              <EmptyState icon={SearchX} title="No matches" description="No repos scanned with embeddings match this query yet -- try a rescan or a different scope." />
            )}
            {!isLoading && !!results?.length && <p className="px-1.5 text-[13.5px] text-ink-500">{results.length} results for &ldquo;{submittedQuery}&rdquo;</p>}
            {results?.map((result) => (
              <SearchResultCard
                key={`${result.repo}:${result.id}`}
                kind={result.kind}
                kindLabel={kindLabel(result.kind)}
                title={result.name ?? result.path ?? `${displayRepoName(result.repo)}#${result.id}`}
                snippet={result.snippet ?? ""}
                score={result.similarity}
                location={result.start_line ? `L${result.start_line}${result.end_line ? `-${result.end_line}` : ""}` : displayRepoName(result.repo)}
                selected={selected?.repo === result.repo && selected?.id === result.id}
                onClick={() => setSelected({ repo: result.repo, id: result.id, kind: result.kind, name: result.name, path: result.path })}
              />
            ))}
          </div>

          {selected && (
            <div className="flex min-w-0 flex-1 flex-col gap-6 overflow-y-auto border-l bg-panel p-7">
              <div className="flex flex-wrap items-start justify-between gap-3.5">
                <div className="flex min-w-0 flex-col gap-1.5">
                  <span className="flex items-center gap-1.5 font-mono text-[11px] uppercase tracking-wide" style={{ color: selectedShape!.color }}>
                    <NavDot color={selectedShape!.color} shape={selectedShape!.shape} />
                    {kindLabel(selected.kind)} · {displayRepoName(selected.repo)}
                  </span>
                  <p className="text-display-card font-bold text-ink-100">{selected.name ?? selected.path ?? `Node ${selected.id}`}</p>
                </div>
                <div className="flex shrink-0 items-center gap-2">
                  <Button
                    size="cta"
                    variant="outline"
                    onClick={() => router.push(`/graph?repo=${encodeURIComponent(selected.repo)}&node=${selected.id}`)}
                  >
                    Open in graph
                  </Button>
                  <CopyButton value={`${selected.repo}:${selected.kind}:${selected.id}`} label="Copy ID" />
                </div>
              </div>

              {!detail && !detailError && <p className="text-body text-ink-500">Loading details…</p>}

              {detailError && (
                <div className="flex flex-col gap-2">
                  <p className="text-body text-destructive">Couldn&apos;t load this item&apos;s details.</p>
                  <p className="text-section text-ink-500">{detailError instanceof Error ? detailError.message : "Please try again."}</p>
                  <Button size="sm" variant="outline" className="self-start" onClick={() => mutateDetail()}>
                    Retry
                  </Button>
                </div>
              )}

              {detail && <NodeDetailSections detail={detail} branch={branch} onSelectConnected={selectConnectedNode} splitKnowledge />}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

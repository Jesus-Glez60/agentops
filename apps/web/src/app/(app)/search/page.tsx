"use client";

import { useEffect, useState } from "react";
import useSWR from "swr";
import { SearchIcon, SearchX } from "lucide-react";
import { getNodeDetail, getRepos, REPOS_SWR_KEY, search, type ConnectedNode, type NodeKind } from "@/lib/api/agentops-api";
import { getRecentSearches, pushRecentSearch } from "@/lib/recent-searches";
import { kindLabel } from "@/lib/node-detail-formatting";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
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

  const { data: results, isLoading } = useSWR(
    submittedQuery ? ["search", submittedQuery, repoScope, kinds] : null,
    () => search(submittedQuery as string, { repos: repoScope, kinds }),
  );

  const { data: detail } = useSWR(selected ? ["node", selected.repo, selected.id] : null, () => getNodeDetail(selected!.repo, selected!.id));

  // Same SWR key the Overview page/topbar/scope selector already share --
  // used here only to show a node's real branch, not a fabricated one.
  const { data: allRepos } = useSWR(REPOS_SWR_KEY, getRepos);
  const branch = selected ? allRepos?.find((r) => r.name === selected.repo)?.branch : undefined;

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

  return (
    <div className="flex h-full flex-col gap-4 p-6">
      <div className={hasSubmitted ? "flex flex-col gap-3" : "mx-auto flex w-full max-w-2xl flex-1 flex-col justify-center gap-6"}>
        {!hasSubmitted && (
          <div className="flex flex-col items-center gap-1.5 text-center">
            <p className="text-page-title text-ink-100">Ask about your codebase</p>
            <p className="text-body text-ink-500">Search across symbols, files, gotchas, and decisions using natural language.</p>
          </div>
        )}

        <form
          onSubmit={(e) => {
            e.preventDefault();
            runSearch(query);
          }}
          className="flex items-center gap-2"
        >
          <div className="relative flex-1">
            <SearchIcon className="pointer-events-none absolute left-2.5 top-1/2 size-4 -translate-y-1/2 text-ink-500" />
            <Input
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="Ask a question about your codebase…"
              className="h-10 pl-8"
            />
            <span className="pointer-events-none absolute right-2.5 top-1/2 -translate-y-1/2 rounded border border-border-strong px-1.5 py-0.5 text-mono-code text-ink-500">⌘K</span>
          </div>
          <Button type="submit" disabled={!query.trim()}>
            Search
          </Button>
        </form>

        <div className="flex flex-wrap items-center justify-between gap-2">
          <SearchFilters selected={kinds} onToggle={toggleKind} />
          <ScopeSelector selected={repoScope} onChange={setRepoScope} />
        </div>

        {!hasSubmitted && (
          <>
            <div className="flex flex-col gap-1.5">
              <p className="text-section text-ink-500">Example questions</p>
              <div className="flex flex-wrap gap-2">
                {EXAMPLE_QUESTIONS.map((q) => (
                  <button
                    key={q}
                    type="button"
                    onClick={() => runSearch(q)}
                    className="rounded-md border border-border-strong px-2.5 py-1.5 text-left text-body text-ink-300 transition-colors hover:border-primary/50 hover:text-ink-100"
                  >
                    {q}
                  </button>
                ))}
              </div>
            </div>

            {recent.length > 0 && (
              <div className="flex flex-col gap-1.5">
                <p className="text-section text-ink-500">Recent searches</p>
                <div className="flex flex-wrap gap-2">
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
          </>
        )}
      </div>

      {hasSubmitted && (
        <div className="flex flex-1 gap-4 overflow-hidden">
          <div className="flex w-full max-w-md flex-col gap-2 overflow-y-auto">
            {isLoading && <p className="text-body text-ink-500">Searching…</p>}
            {!isLoading && results?.length === 0 && (
              <EmptyState icon={SearchX} title="No matches" description="No repos scanned with embeddings match this query yet -- try a rescan or a different scope." />
            )}
            {results?.map((result) => (
              <SearchResultCard
                key={`${result.repo}:${result.id}`}
                kind={result.kind}
                kindLabel={kindLabel(result.kind)}
                title={result.name ?? result.path ?? `${result.repo}#${result.id}`}
                snippet={result.snippet ?? ""}
                score={result.similarity}
                selected={selected?.repo === result.repo && selected?.id === result.id}
                onClick={() => setSelected({ repo: result.repo, id: result.id, kind: result.kind, name: result.name, path: result.path })}
              />
            ))}
          </div>

          {selected && (
            <div className="flex min-w-0 flex-1 flex-col gap-6 overflow-y-auto rounded-lg border border-border-strong bg-panel p-5">
              <div className="flex items-start justify-between gap-2">
                <div className="flex flex-col gap-0.5">
                  <p className="text-subheading text-ink-100">{selected.name ?? selected.path ?? `Node ${selected.id}`}</p>
                  <p className="text-mono-path text-ink-500">{selected.path ?? selected.repo}</p>
                </div>
                <CopyButton value={`${selected.repo}:${selected.kind}:${selected.id}`} label="Copy ID" />
              </div>

              {!detail && <p className="text-body text-ink-500">Loading details…</p>}

              {detail && <NodeDetailSections detail={detail} branch={branch} onSelectConnected={selectConnectedNode} />}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

"use client";

import { useEffect, useState } from "react";
import useSWR from "swr";
import { Search as SearchIcon } from "lucide-react";
import { search, docsSearch } from "@/lib/api/heavy-api";
import { getGraph } from "@/lib/api/agentops-api";
import { ApiError } from "@/lib/api/fetcher";
import { getRecentSearches, pushRecentSearch } from "@/lib/recent-searches";
import { RelationshipChip } from "@/components/shared/relationship-chip";
import { CodeBlock } from "@/components/shared/code-block";
import { RepoSelect } from "@/components/shared/repo-select";
import { SearchResultCard } from "@/components/search/search-result-card";
import { LicenseRequiredState } from "@/components/shared/license-required-state";
import { ErrorState } from "@/components/shared/error-state";
import { EmptyState } from "@/components/shared/empty-state";
import { Skeleton } from "@/components/ui/skeleton";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";

const EXAMPLE_QUESTIONS = [
  "Where is authentication state refreshed?",
  "What can break when changing the billing webhook?",
  "Why does this service avoid database transactions?",
];

type Mode = "code" | "docs";

export default function SearchPage() {
  const [mode, setMode] = useState<Mode>("code");
  const [scope, setScope] = useState(""); // repo path (code) or library slug (docs)
  const [scopeInput, setScopeInput] = useState("");
  const [query, setQuery] = useState("");
  const [queryInput, setQueryInput] = useState("");
  const [selectedIndex, setSelectedIndex] = useState<number | null>(null);
  // Must NOT read localStorage in the initializer -- that runs during the
  // client's hydrating render too, and localStorage doesn't exist on the
  // server (getRecentSearches returns `[]` there). If the two renders
  // disagree on whether this list is empty, the "Recent searches" block's
  // presence/absence mismatches between server and client HTML, which is a
  // real hydration error, not just a cosmetic flash. Start empty (matching
  // the server) and fill in the real value after mount instead.
  const [recent, setRecent] = useState<string[]>([]);
  useEffect(() => {
    // eslint-disable-next-line react-hooks/set-state-in-effect
    setRecent(getRecentSearches("code"));
  }, []);
  // Search silently no-ops without a scope (SWR's key is `null`), which with
  // no scope input filled in reads as "I typed a question, hit Search, and
  // nothing happened" -- no error, no results, no explanation. Track whether
  // Search was actually pressed so a missing scope can say so instead.
  const [hasSubmitted, setHasSubmitted] = useState(false);

  const canSearch = Boolean(scope && query);
  const missingScope = hasSubmitted && !scopeInput.trim();

  const {
    data: codeResults,
    error: codeError,
    isLoading: codeLoading,
  } = useSWR(mode === "code" && canSearch ? ["search", scope, query] : null, () => search(scope, query));

  const {
    data: docResults,
    error: docError,
    isLoading: docLoading,
  } = useSWR(mode === "docs" && canSearch ? ["docs-search", scope, query] : null, () => docsSearch(scope, query));

  const selectedHit = mode === "code" ? codeResults?.results[selectedIndex ?? -1] : docResults?.results[selectedIndex ?? -1];

  const { data: graphData } = useSWR(
    mode === "code" && scope ? ["graph-for-search", scope] : null,
    () => getGraph(scope),
  );
  const connectedEdges = (() => {
    if (mode !== "code" || !graphData || !selectedHit) return [];
    const id = Number(selectedHit.id);
    return graphData.edges.filter((e) => e.src_id === id || e.dst_id === id);
  })();

  function runSearch() {
    setHasSubmitted(true);
    setScope(scopeInput.trim());
    setQuery(queryInput.trim());
    setSelectedIndex(null);
    if (scopeInput.trim() && queryInput.trim()) {
      setRecent(pushRecentSearch(mode === "code" ? "code" : "docs", queryInput));
    }
  }

  const error = mode === "code" ? codeError : docError;
  const isLoading = mode === "code" ? codeLoading : docLoading;
  const results = mode === "code" ? codeResults?.results : docResults?.results;

  return (
    <div className="flex flex-col gap-4">
      <div className="flex items-center justify-between">
        <h1 className="text-page-title font-bold">Semantic Search</h1>
        <Tabs
          value={mode}
          onValueChange={(v) => {
            setMode(v as Mode);
            setSelectedIndex(null);
            setRecent(getRecentSearches(v === "code" ? "code" : "docs"));
          }}
        >
          <TabsList>
            <TabsTrigger value="code">Code</TabsTrigger>
            <TabsTrigger value="docs">Docs</TabsTrigger>
          </TabsList>
        </Tabs>
      </div>

      <form
        className="flex gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          runSearch();
        }}
      >
        {mode === "code" ? (
          <RepoSelect
            value={scopeInput}
            onChange={(path) => {
              setScopeInput(path);
              if (hasSubmitted) setHasSubmitted(false);
            }}
            className={missingScope ? "border-destructive ring-3 ring-destructive/20" : undefined}
          />
        ) : (
          <Input
            value={scopeInput}
            onChange={(e) => {
              setScopeInput(e.target.value);
              if (hasSubmitted) setHasSubmitted(false);
            }}
            placeholder="library slug"
            aria-invalid={missingScope}
            className="w-64 font-mono text-mono-path"
          />
        )}
        <Input
          value={queryInput}
          onChange={(e) => setQueryInput(e.target.value)}
          placeholder="Ask a question about your codebase..."
          className="flex-1"
        />
        <Button type="submit" size="sm" className="gap-1.5">
          <SearchIcon className="size-4" />
          Search
        </Button>
      </form>

      {missingScope && (
        <p className="text-body text-destructive">
          Enter {mode === "code" ? "a repo path" : "a library slug"} above — search needs a scope to run against.
        </p>
      )}

      {!canSearch && (
        <div className="flex flex-col gap-3">
          <div>
            <p className="mb-2 text-label uppercase tracking-wide text-ink-500">Example questions</p>
            <div className="flex flex-wrap gap-2">
              {EXAMPLE_QUESTIONS.map((q) => (
                <button
                  key={q}
                  onClick={() => setQueryInput(q)}
                  className="rounded-md border border-border-strong bg-panel px-3 py-1.5 text-left text-body text-ink-300 hover:border-primary/50"
                >
                  {q}
                </button>
              ))}
            </div>
          </div>
          {recent.length > 0 && (
            <div>
              <p className="mb-2 text-label uppercase tracking-wide text-ink-500">Recent searches</p>
              <div className="flex flex-col gap-1">
                {recent.map((q) => (
                  <button key={q} onClick={() => setQueryInput(q)} className="text-left text-body text-ink-300 hover:text-ink-100 hover:underline">
                    {q}
                  </button>
                ))}
              </div>
            </div>
          )}
        </div>
      )}

      {canSearch && error instanceof ApiError && error.status === 402 && <LicenseRequiredState />}
      {canSearch && error && !(error instanceof ApiError && error.status === 402) && (
        <ErrorState message={error instanceof Error ? error.message : String(error)} />
      )}
      {canSearch && isLoading && <Skeleton className="h-64 w-full" />}

      {canSearch && !isLoading && !error && results && results.length === 0 && (
        <EmptyState icon={SearchIcon} title="No results" description="Try adjusting your query or scope." />
      )}

      {canSearch && !isLoading && !error && results && results.length > 0 && (
        <div className="flex gap-4">
          <div className="flex w-96 shrink-0 flex-col gap-2">
            {mode === "code" &&
              codeResults!.results.map((hit, i) => (
                <SearchResultCard
                  key={`${hit.id}-${i}`}
                  kindLabel={hit.kind}
                  title={hit.name ?? hit.path ?? `#${hit.id}`}
                  snippet={hit.text}
                  score={hit.score}
                  selected={selectedIndex === i}
                  onClick={() => setSelectedIndex(i)}
                />
              ))}
            {mode === "docs" &&
              docResults!.results.map((hit, i) => (
                <SearchResultCard
                  key={`${hit.id}-${i}`}
                  kindLabel="doc"
                  title={hit.topic ?? hit.slug}
                  snippet={hit.text}
                  score={hit.score}
                  selected={selectedIndex === i}
                  onClick={() => setSelectedIndex(i)}
                />
              ))}
          </div>

          <div className="min-w-0 flex-1 rounded-md border border-border-strong bg-panel p-4">
            {!selectedHit && <p className="text-body text-ink-500">Select a result to inspect it.</p>}
            {selectedHit && (
              <div className="flex flex-col gap-3">
                <p className="text-section font-medium text-ink-100">
                  {"name" in selectedHit ? (selectedHit.name ?? selectedHit.path) : (selectedHit.topic ?? selectedHit.slug)}
                </p>
                {mode === "code" && connectedEdges.length > 0 && (
                  <div>
                    <p className="mb-2 text-label uppercase tracking-wide text-ink-500">Connected nodes</p>
                    <div className="flex flex-wrap gap-2">
                      {connectedEdges.map((e) => {
                        const id = Number(selectedHit.id);
                        const isSource = e.src_id === id;
                        const otherId = isSource ? e.dst_id : e.src_id;
                        const other = graphData?.nodes.find((n) => n.id === otherId);
                        return (
                          <RelationshipChip
                            key={e.id}
                            relation={isSource ? e.relation : `← ${e.relation}`}
                            target={other?.name ?? other?.path ?? `#${otherId}`}
                          />
                        );
                      })}
                    </div>
                  </div>
                )}
                <CodeBlock code={selectedHit.text} />
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

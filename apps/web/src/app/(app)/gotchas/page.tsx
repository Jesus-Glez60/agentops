"use client";

import { useEffect, useRef, useState } from "react";
import useSWR, { useSWRConfig } from "swr";
import { toast } from "sonner";
import { ArrowDown, CheckCircle2, FileCode, GitBranch, Pencil, Pin, RotateCcw, SearchIcon, TriangleAlert } from "lucide-react";
import {
  getGotchas,
  getNodeDetail,
  getRepos,
  GOTCHAS_SWR_KEY,
  REPOS_SWR_KEY,
  setCuration,
  type GotchaBucket,
  type GotchaSummary,
  type NodeProminence,
} from "@/lib/api/repos-api";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { ScopeSelector } from "@/components/search/scope-selector";
import { CopyButton } from "@/components/shared/copy-button";
import { EmptyState } from "@/components/shared/empty-state";
import { displayRepoName } from "@/lib/utils";
import { NodeDetailSections } from "@/components/shared/node-detail-sections";
import { CurationReasonDialog } from "@/components/gotchas/curation-reason-dialog";
import { mapWithConcurrency } from "@/lib/concurrency";
import { cn } from "@/lib/utils";

const BUCKET_TABS: { label: string; value: GotchaBucket | "all" }[] = [
  { label: "All", value: "all" },
  { label: "Needs curation", value: "needs_curation" },
  { label: "Pinned", value: "pinned" },
  { label: "Kept", value: "kept" },
  { label: "Reduced", value: "reduced" },
];

// `Pinned` checked first -- a pinned gotcha is also `curated` (`set_curation`
// always sets it, regardless of prominence), so it would otherwise also
// match "kept"'s condition. `Pinned` is the stronger signal and should own
// the bucket exclusively, matching `agentops_api::repos::matches_bucket`'s
// own arm ordering.
function bucketOf(gotcha: Pick<GotchaSummary, "curated" | "prominence">): GotchaBucket {
  if (gotcha.prominence === "Pinned") return "pinned";
  if (!gotcha.curated) return "needs_curation";
  return gotcha.prominence === "Reduced" ? "reduced" : "kept";
}

const BUCKET_BADGE: Record<GotchaBucket, { label: string; className: string }> = {
  needs_curation: { label: "Needs curation", className: "border-curation-needs-curation/40 bg-curation-needs-curation/10 text-curation-needs-curation" },
  pinned: { label: "Pinned", className: "border-curation-pinned/40 bg-curation-pinned/10 text-curation-pinned" },
  kept: { label: "Kept", className: "border-curation-kept/40 bg-curation-kept/10 text-curation-kept" },
  reduced: { label: "Reduced", className: "border-curation-reduced/40 text-curation-reduced" },
};

function BucketBadge({ bucket }: { bucket: GotchaBucket }) {
  const { label, className } = BUCKET_BADGE[bucket];
  return <span className={cn("inline-flex items-center rounded-md border px-2 py-0.5 text-mono-code font-medium", className)}>{label}</span>;
}

export default function GotchasPage() {
  const [bucketTab, setBucketTab] = useState<GotchaBucket | "all">("all");
  const [repoScope, setRepoScope] = useState<string[]>([]);
  const [filterText, setFilterText] = useState("");
  const [selected, setSelected] = useState<{ repo: string; id: number } | null>(null);
  // What the reason dialog applies to when submitted -- the single
  // selected node, or every currently-filtered gotcha (the "Reduce all"
  // bulk action). `null` means the dialog is closed.
  const [reduceTarget, setReduceTarget] = useState<"selected" | "bulk" | null>(null);
  // Session-local "reviewed" tracking for the progress bar -- a gotcha ID
  // joins this the moment it gets a *committed* action (Keep/Pin/confirmed
  // Reduce/explicit Skip), never removed again even if its curation changes
  // later (matches this screen's "nothing is ever deleted, curation only
  // reorders" philosophy -- reviewing isn't undone by a later re-curation).
  // Not persisted -- resets on mount and whenever the bucket tab changes,
  // since switching buckets starts a new review pass over that queue.
  const [reviewed, setReviewed] = useState<Set<string>>(new Set());
  function markReviewed(repo: string, id: number) {
    setReviewed((prev) => new Set(prev).add(`${repo}:${id}`));
  }
  function handleBucketTabChange(tab: GotchaBucket | "all") {
    setBucketTab(tab);
    setReviewed(new Set());
  }
  const { mutate } = useSWRConfig();

  const { data: gotchas, isLoading } = useSWR(GOTCHAS_SWR_KEY, () => getGotchas());
  const { data: allRepos } = useSWR(REPOS_SWR_KEY, getRepos);

  const scoped = (gotchas ?? []).filter((g) => repoScope.length === 0 || repoScope.includes(g.repo));
  const needsCurationCount = scoped.filter((g) => bucketOf(g) === "needs_curation").length;
  const pinnedCount = scoped.filter((g) => bucketOf(g) === "pinned").length;
  const keptCount = scoped.filter((g) => bucketOf(g) === "kept").length;
  const reducedCount = scoped.filter((g) => bucketOf(g) === "reduced").length;

  const filtered = scoped.filter((g) => {
    if (bucketTab !== "all" && bucketOf(g) !== bucketTab) return false;
    if (!filterText.trim()) return true;
    const haystack = `${g.name ?? ""} ${g.snippet ?? ""}`.toLowerCase();
    return haystack.includes(filterText.trim().toLowerCase());
  });

  // Driven by `detail`, not the gotchas list -- clicking a Connected Node
  // row can navigate to a Symbol/File/Decision that was never in the
  // gotchas list at all, so a lookup keyed against that list would go
  // undefined and drop the whole detail panel. `NodeDetail` (any kind)
  // already carries curation fields, so it's the one source of truth here.
  const detailKey = selected ? (["node", selected.repo, selected.id] as const) : null;
  const { data: detail, error: detailError } = useSWR(detailKey, () => getNodeDetail(selected!.repo, selected!.id));
  const branch = selected ? allRepos?.connections.find((r) => r.id === selected.repo)?.branch : undefined;

  async function applyCuration(prominence: NodeProminence, reason: string | null) {
    if (!selected) return;
    try {
      const updated = await setCuration(selected.repo, selected.id, prominence, reason);
      const patch = { curated: true, prominence: updated.prominence, curation_reason: prominence === "Reduced" ? reason : null };
      mutate(GOTCHAS_SWR_KEY, (current: GotchaSummary[] | undefined) => current?.map((g) => (g.repo === selected.repo && g.id === selected.id ? { ...g, ...patch } : g)), { revalidate: false });
      mutate(detailKey, (current: typeof detail) => (current ? { ...current, ...patch } : current), { revalidate: false });
      mutate(REPOS_SWR_KEY);
      markReviewed(selected.repo, selected.id);
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Couldn't update curation. Please try again.");
    }
  }

  // Moves `selected` by `delta` within the currently-filtered list (↑/↓
  // shortcuts), or Skip -- advances past the current item without curating
  // it, still counted as reviewed (the user looked at it and decided no
  // action was needed).
  function moveSelection(delta: 1 | -1) {
    if (filtered.length === 0) return;
    const currentIndex = selected ? filtered.findIndex((g) => g.repo === selected.repo && g.id === selected.id) : -1;
    const nextIndex = currentIndex === -1 ? 0 : Math.min(Math.max(currentIndex + delta, 0), filtered.length - 1);
    const next = filtered[nextIndex];
    if (next) setSelected({ repo: next.repo, id: next.id });
  }
  function skipSelected() {
    if (!selected) return;
    markReviewed(selected.repo, selected.id);
    moveSelection(1);
  }

  // "Keep all" / "Reduce all" -- applies to every currently-filtered
  // gotcha (bucket tab + repo scope + text filter), not the whole list, so
  // it does what's visible on screen, not a silent global action.
  async function applyCurationToMany(targets: GotchaSummary[], prominence: NodeProminence, reason: string | null) {
    if (targets.length === 0) return;
    // Concurrency-limited, not a bare `Promise.allSettled(targets.map(...))`
    // -- the backend opens a fresh Postgres connection pool per request
    // rather than sharing one, so firing every target at once is a burst
    // of concurrent pool creations. Confirmed live: an unthrottled "Keep
    // all" against 54 gotchas failed 32 of them.
    const results = await mapWithConcurrency(targets, 5, (g) => setCuration(g.repo, g.id, prominence, reason));
    const failed = results.filter((r) => r.status === "rejected").length;
    const succeeded = new Set(targets.filter((_, i) => results[i].status === "fulfilled").map((g) => `${g.repo}:${g.id}`));
    const patch = { curated: true, prominence, curation_reason: prominence === "Reduced" ? reason : null };

    mutate(GOTCHAS_SWR_KEY, (current: GotchaSummary[] | undefined) => current?.map((g) => (succeeded.has(`${g.repo}:${g.id}`) ? { ...g, ...patch } : g)), { revalidate: false });
    if (selected && succeeded.has(`${selected.repo}:${selected.id}`)) {
      mutate(detailKey, (current: typeof detail) => (current ? { ...current, ...patch } : current), { revalidate: false });
    }
    mutate(REPOS_SWR_KEY);

    if (failed > 0) toast.error(`${failed} of ${targets.length} gotcha${targets.length === 1 ? "" : "s"} couldn't be updated. Please try again.`);
  }

  async function submitReduceDialog(reason: string) {
    if (reduceTarget === "bulk") {
      await applyCurationToMany(filtered, "Reduced", reason);
    } else {
      await applyCuration("Reduced", reason);
    }
    setReduceTarget(null);
  }

  // K/P/R/↑/↓ triage shortcuts -- scoped to this page (not sidebar-global
  // like ⌘K), ignored while focus is in a text field or the reduce-reason
  // dialog is open (typing "r" in the reason textarea shouldn't open a
  // second reduce flow). Kept in a ref so the listener can be registered
  // once on mount rather than re-subscribing on every selection change.
  const handlersRef = useRef({ applyCuration, moveSelection, setReduceTarget, detail, selected });
  // No deps array -- this must re-sync on every render, it's not a normal
  // "run when X changes" effect.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  useEffect(() => {
    handlersRef.current = { applyCuration, moveSelection, setReduceTarget, detail, selected };
  });
  useEffect(() => {
    function handleKeydown(e: KeyboardEvent) {
      if (reduceTarget !== null) return;
      const target = e.target as HTMLElement | null;
      if (target && (target.tagName === "INPUT" || target.tagName === "TEXTAREA" || target.isContentEditable)) return;
      const { applyCuration, moveSelection, setReduceTarget, detail, selected } = handlersRef.current;
      if (!selected) return;
      switch (e.key) {
        case "k":
        case "K":
          e.preventDefault();
          applyCuration("Full", null);
          break;
        case "p":
        case "P":
          e.preventDefault();
          if (detail?.prominence !== "Pinned") applyCuration("Pinned", null);
          break;
        case "r":
        case "R":
          e.preventDefault();
          setReduceTarget("selected");
          break;
        case "ArrowUp":
          e.preventDefault();
          moveSelection(-1);
          break;
        case "ArrowDown":
          e.preventDefault();
          moveSelection(1);
          break;
      }
    }
    document.addEventListener("keydown", handleKeydown);
    return () => document.removeEventListener("keydown", handleKeydown);
  }, [reduceTarget]);

  // "This session" progress -- reviewed count against everything currently
  // needing curation in-scope, matching the prototype's "{reviewed} reviewed
  // · {needs} left" framing.
  const sessionTotal = reviewed.size + needsCurationCount;
  const sessionPct = sessionTotal === 0 ? 0 : Math.round((reviewed.size / sessionTotal) * 100);

  return (
    <div className="flex h-full flex-col gap-4 p-6">
      <div className="flex flex-wrap items-end justify-between gap-6">
        <div className="flex flex-col gap-2">
          <span className="font-mono text-[12px] uppercase tracking-wide text-peach">Curate</span>
          <h1 className="text-display-hero font-extrabold tracking-[-0.04em] text-ink-100">Gotchas</h1>
          <p className="max-w-[640px] text-body-lg text-ink-300">Agents and teammates write these down as they work. Nothing is ever deleted: curation decides how prominently each one is shown to agents.</p>
        </div>
        {sessionTotal > 0 && (
          <div className="flex min-w-[220px] flex-col gap-1.5">
            <div className="flex justify-between text-section">
              <span className="text-ink-300">This session</span>
              <span className="font-bold text-ink-100">
                {reviewed.size} reviewed · {needsCurationCount} left
              </span>
            </div>
            <div className="h-1.5 overflow-hidden rounded-full bg-raised">
              <div className="h-full rounded-full bg-gradient-to-r from-peach to-yellow" style={{ width: `${sessionPct}%` }} />
            </div>
          </div>
        )}
      </div>

      <div className="grid grid-cols-4 gap-4">
        <StatBox label="Needs curation" value={needsCurationCount} valueClassName="text-curation-needs-curation" />
        <StatBox label="Pinned" value={pinnedCount} valueClassName="text-curation-pinned" />
        <StatBox label="Kept" value={keptCount} valueClassName="text-curation-kept" />
        <StatBox label="Reduced" value={reducedCount} valueClassName="text-curation-reduced" />
      </div>

      <div className="flex flex-col gap-2.5">
        <div className="relative">
          <SearchIcon className="pointer-events-none absolute left-2.5 top-1/2 size-4 -translate-y-1/2 text-ink-500" />
          <Input value={filterText} onChange={(e) => setFilterText(e.target.value)} placeholder="Filter gotchas…" className="pl-8" />
        </div>
        <div className="flex flex-wrap items-center justify-between gap-2">
          <div className="flex flex-wrap gap-1.5">
            {BUCKET_TABS.map((tab) => (
              <button
                key={tab.value}
                type="button"
                onClick={() => handleBucketTabChange(tab.value)}
                aria-pressed={bucketTab === tab.value}
                className={cn(
                  "rounded-md border px-2.5 py-1 text-section transition-colors",
                  bucketTab === tab.value ? "border-primary bg-raised text-ink-100" : "border-border-strong text-ink-500 hover:text-ink-300",
                )}
              >
                {tab.label}
              </button>
            ))}
            <span className="ml-2 hidden self-center font-mono text-[12px] text-ink-500 sm:inline">K keep · P pin · R reduce · ↑↓ move</span>
          </div>
          <ScopeSelector selected={repoScope} onChange={setRepoScope} />
        </div>
        <div className="flex items-center gap-2">
          <span className="text-mono-code text-ink-500">
            {filtered.length} shown —
          </span>
          <Button size="sm" variant="outline" className="gap-1.5" disabled={filtered.length === 0} onClick={() => applyCurationToMany(filtered, "Full", null)}>
            <CheckCircle2 className="size-3.5" />
            Keep all
          </Button>
          <Button size="sm" variant="outline" className="gap-1.5" disabled={filtered.length === 0} onClick={() => setReduceTarget("bulk")}>
            <TriangleAlert className="size-3.5" />
            Reduce all
          </Button>
        </div>
      </div>

      <div className="flex flex-1 gap-4 overflow-hidden">
        <div className="flex w-full max-w-md flex-col gap-2 overflow-y-auto">
          {isLoading && <p className="text-body text-ink-500">Loading…</p>}
          {!isLoading && filtered.length === 0 && <EmptyState icon={TriangleAlert} title="No gotchas" description="Nothing matches this filter." />}
          {filtered.map((gotcha) => (
            <button
              key={`${gotcha.repo}:${gotcha.id}`}
              onClick={() => setSelected({ repo: gotcha.repo, id: gotcha.id })}
              className={cn(
                "flex w-full flex-col gap-1.5 rounded-md border p-3 text-left transition-colors",
                selected?.repo === gotcha.repo && selected?.id === gotcha.id ? "border-primary bg-raised" : "border-border-strong bg-panel hover:border-border-strong/80",
              )}
            >
              <BucketBadge bucket={bucketOf(gotcha)} />
              <p className="text-section font-medium text-ink-100">{gotcha.name ?? gotcha.path ?? `${displayRepoName(gotcha.repo)}#${gotcha.id}`}</p>
              {gotcha.snippet && <p className="line-clamp-2 text-body text-ink-500">{gotcha.snippet}</p>}
              <div className="flex items-center gap-2 text-mono-path text-ink-500">
                <GitBranch className="size-3" />
                {displayRepoName(gotcha.repo)}
                {gotcha.path && (
                  <>
                    <span className="text-border-strong">·</span>
                    <FileCode className="size-3" />
                    {gotcha.path}
                  </>
                )}
              </div>
            </button>
          ))}
        </div>

        {selected && (
          <div className="flex min-w-0 flex-1 flex-col gap-6 overflow-y-auto rounded-lg border border-border-strong bg-panel p-5">
            {!detail && !detailError && <p className="text-body text-ink-500">Loading details…</p>}
            {detailError && (
              <div className="flex flex-col gap-2">
                <p className="text-body text-destructive">Couldn&apos;t load this item&apos;s details.</p>
                <p className="text-section text-ink-500">{detailError instanceof Error ? detailError.message : "Please try again."}</p>
                <Button size="sm" variant="outline" className="self-start" onClick={() => mutate(detailKey)}>
                  Retry
                </Button>
              </div>
            )}
            {detail && (
              <>
                <div className="flex items-start justify-between gap-2">
                  <div className="flex flex-col gap-1.5">
                    <div className="flex items-center gap-2">
                      <BucketBadge bucket={bucketOf(detail)} />
                      <span className="rounded-md border border-border-strong px-2 py-0.5 text-mono-code text-ink-400">{detail.repo}</span>
                    </div>
                    <p className="text-display-card font-bold text-ink-100">{detail.name ?? detail.path ?? `Node ${detail.id}`}</p>
                  </div>
                  <div className="flex shrink-0 items-center gap-2">
                    {/* No dismiss/delete action anywhere -- gotchas are
                        permanent knowledge, curation only reorders them. */}
                    {!(detail.curated && detail.prominence === "Full") && (
                      <Button size="sm" className="gap-1.5 bg-curation-kept/15 text-curation-kept hover:bg-curation-kept/25" variant="ghost" onClick={() => applyCuration("Full", null)}>
                        <CheckCircle2 className="size-3.5" />
                        Keep as permanent knowledge
                      </Button>
                    )}
                    {/* No reason dialog for Pin/Unpin -- mirrors the Full
                        path, not the Reduced one. */}
                    {detail.prominence === "Pinned" ? (
                      <Button size="sm" variant="outline" className="gap-1.5" onClick={() => applyCuration("Full", null)}>
                        <Pin className="size-3.5" />
                        Unpin
                      </Button>
                    ) : (
                      <Button size="sm" className="gap-1.5 bg-curation-pinned/15 text-curation-pinned hover:bg-curation-pinned/25" variant="ghost" onClick={() => applyCuration("Pinned", null)}>
                        <Pin className="size-3.5" />
                        Pin
                      </Button>
                    )}
                    {detail.prominence === "Reduced" ? (
                      <>
                        <Button size="sm" variant="outline" className="gap-1.5" onClick={() => setReduceTarget("selected")}>
                          <Pencil className="size-3.5" />
                          Edit reason
                        </Button>
                        <Button size="sm" variant="outline" className="gap-1.5" onClick={() => applyCuration("Full", null)}>
                          <RotateCcw className="size-3.5" />
                          Restore full prominence
                        </Button>
                      </>
                    ) : detail.prominence !== "Pinned" ? (
                      <Button size="sm" variant="outline" className="gap-1.5" onClick={() => setReduceTarget("selected")}>
                        <TriangleAlert className="size-3.5" />
                        Reduce prominence
                      </Button>
                    ) : null}
                    <CopyButton value={`${detail.repo}:${detail.kind}:${detail.id}`} label="Copy ID" />
                    <Button size="sm" variant="ghost" className="ml-auto gap-1.5 text-ink-500" onClick={skipSelected}>
                      <ArrowDown className="size-3.5" />
                      Skip
                    </Button>
                  </div>
                </div>

                <NodeDetailSections detail={detail} branch={branch} onSelectConnected={(node) => setSelected({ repo: detail.repo, id: node.id })} />
              </>
            )}
          </div>
        )}
      </div>

      <CurationReasonDialog
        open={reduceTarget !== null}
        onOpenChange={(open) => !open && setReduceTarget(null)}
        initialReason={reduceTarget === "selected" ? detail?.curation_reason : null}
        onSubmit={submitReduceDialog}
      />
    </div>
  );
}

function StatBox({ label, value, valueClassName }: { label: string; value: number; valueClassName: string }) {
  return (
    <div className="rounded-lg border border-border-strong bg-panel px-4 py-3">
      <p className="text-label uppercase tracking-wide text-ink-500">{label}</p>
      <p className={cn("text-stat-value font-bold", valueClassName)}>{value}</p>
    </div>
  );
}

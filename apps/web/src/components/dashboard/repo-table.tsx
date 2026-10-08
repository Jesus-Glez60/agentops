"use client";

import { useState } from "react";
import useSWR, { useSWRConfig } from "swr";
import { toast } from "sonner";
import { ExternalLink, RefreshCw } from "lucide-react";
import Link from "next/link";
import { getRepos, startIndexing, REPOS_SWR_KEY, parseRepoStatus, type RepoConnection } from "@/lib/api/repos-api";
import { useRemoveRepo } from "@/hooks/use-remove-repo";
import { repoHealthWithReason } from "@/lib/repo-health";
import { HealthBadge } from "@/components/dashboard/health-badge";
import { NodeCountBar } from "@/components/dashboard/node-count-bar";
import { BranchSelect } from "@/components/repositories/branch-select";
import { Button } from "@/components/ui/button";
import { EmptyState } from "@/components/shared/empty-state";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";

/**
 * Flat bordered row list, not a `Table` inside a `Card` -- matches the
 * prototype's own Overview `repoRows` section exactly (plain "Repositories"
 * heading + "View all →" link, each row a loose flex layout with a pill
 * branch tag, a colored-dot health label, a segmented node-count bar, and
 * a single pill action button, legend dots at the bottom). The previous
 * `Table`/`CardTitle`+icon version was carried over from before this
 * redesign pass and never actually reconciled against the prototype.
 */
export function RepoTable() {
  const { data, isLoading } = useSWR(REPOS_SWR_KEY, getRepos);
  const repos = data?.connections;
  // The context-bound mutate (not the top-level `import { mutate } from
  // "swr"`) -- guarantees this always targets whatever cache this
  // component's own useSWR call actually reads from, rather than assuming
  // it's always the implicit default cache.
  const { mutate } = useSWRConfig();
  const [reindexingIds, setReindexingIds] = useState<Set<string>>(new Set());
  const { removeRepo, removingIds } = useRemoveRepo();

  // Fires a background reindex job (async, polled elsewhere) -- unlike the
  // retired manifest-based `rescanRepo`, this is not a synchronous
  // rescan-and-return; the connection's `counts` only reflect the new data
  // once the job finishes and this list is revalidated.
  async function handleReindex(repo: RepoConnection) {
    setReindexingIds((prev) => new Set(prev).add(repo.id));
    try {
      await startIndexing(repo.id, "reindex");
      toast.success("Reindexing started.");
      mutate(REPOS_SWR_KEY);
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Reindex failed. Please try again.");
    } finally {
      setReindexingIds((prev) => {
        const next = new Set(prev);
        next.delete(repo.id);
        return next;
      });
    }
  }

  return (
    <section className="flex flex-col gap-3.5">
      <div className="flex items-center justify-between">
        <h2 className="text-page-title font-extrabold tracking-[-0.02em] text-ink-100">Repositories</h2>
        <Link href="/repositories" className="text-[14px] font-semibold text-mauve">
          View all →
        </Link>
      </div>

      <div className="flex flex-col divide-y divide-border rounded-2xl border bg-panel">
        {isLoading && <p className="px-5 py-6 text-center text-body text-ink-500">Loading…</p>}
        {!isLoading && repos?.length === 0 && <EmptyState icon={RefreshCw} title="No repositories connected yet" description="Connect one from Settings to see it here." className="min-h-0 py-10" />}
        {repos?.map((repo) => {
          const reindexing = reindexingIds.has(repo.id);
          const discovered = repo.method === "discovered";
          const status = parseRepoStatus(repo.status);
          const totalNodes = repo.counts ? repo.counts.symbols + repo.counts.files + repo.counts.gotchas + repo.counts.decisions : null;
          return (
            <div key={repo.id} className="flex flex-wrap items-center gap-[18px] px-5 py-[18px]">
              <div className="flex min-w-0 flex-[2_1_220px] flex-col gap-1">
                <span className="text-[16px] font-bold text-ink-100">{repo.id}</span>
                <span className="truncate font-mono text-[11.5px] text-ink-500">{repo.repo_url}</span>
              </div>

              <BranchSelect repo={repo} onChanged={() => mutate(REPOS_SWR_KEY)} className="h-auto w-auto rounded-full border-border-strong bg-transparent px-2.5 py-[3px] font-mono text-[12px] text-ink-300" />

              <div className="min-w-[120px]">{reindexing ? <HealthBadgeScanning /> : <HealthBadge {...repoHealthWithReason(repo)} />}</div>

              <div className="flex flex-[1_1_150px] flex-col gap-1.5">
                {repo.counts ? (
                  <>
                    <NodeCountBar counts={repo.counts} />
                    <span className="font-mono text-[11.5px] text-ink-500">{totalNodes} total</span>
                  </>
                ) : (
                  <>
                    <div className="h-1.5 w-full rounded-full bg-raised" />
                    <span className="font-mono text-[11.5px] text-ink-500">not yet scanned</span>
                  </>
                )}
              </div>

              <div className="flex shrink-0 items-center gap-1.5">
                {discovered ? (
                  <>
                    <Button asChild className="h-auto rounded-full border-border-strong px-3.5 py-[7px] text-[13px] font-semibold" variant="outline">
                      <Link href={`/repositories/connect/ssh?repo_url=${encodeURIComponent(repo.repo_url)}`}>Finish connecting</Link>
                    </Button>
                    <Button
                      variant="outline"
                      disabled={removingIds.has(repo.id)}
                      onClick={() => removeRepo(repo)}
                      className="h-auto rounded-full border-border-strong px-3.5 py-[7px] text-[13px] font-semibold"
                    >
                      {removingIds.has(repo.id) ? "Removing…" : "Remove"}
                    </Button>
                  </>
                ) : (
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <Button
                        variant="outline"
                        disabled={reindexing || repo.path_missing || status.kind !== "active"}
                        onClick={() => handleReindex(repo)}
                        aria-label="Rescan repository"
                        className="h-auto rounded-full border-border-strong px-3.5 py-[7px] text-[13px] font-semibold"
                      >
                        Rescan
                      </Button>
                    </TooltipTrigger>
                    <TooltipContent>{repo.path_missing ? "Repo path no longer exists" : "Rescan"}</TooltipContent>
                  </Tooltip>
                )}
                <Tooltip>
                  <TooltipTrigger asChild>
                    <Button variant="outline" size="icon" asChild aria-label="View details">
                      <Link href={`/repositories/${encodeURIComponent(repo.id)}`}>
                        <ExternalLink className="size-4" />
                      </Link>
                    </Button>
                  </TooltipTrigger>
                  <TooltipContent>View details</TooltipContent>
                </Tooltip>
              </div>
            </div>
          );
        })}
      </div>

      {!!repos?.length && (
        <div className="flex flex-wrap gap-4 font-mono text-[11px] text-ink-500">
          <LegendItem colorClassName="bg-node-symbol" label="symbols" shape="circle" />
          <LegendItem colorClassName="bg-node-file" label="files" shape="square" />
          <LegendItem colorClassName="bg-node-gotcha" label="gotchas" shape="diamond" />
          <LegendItem colorClassName="bg-node-decision" label="decisions" shape="diamond" />
        </div>
      )}
    </section>
  );
}

function LegendItem({ colorClassName, label, shape }: { colorClassName: string; label: string; shape: "circle" | "square" | "diamond" }) {
  return (
    <span className="flex items-center gap-1.5">
      <span
        className={`size-2 shrink-0 ${colorClassName}`}
        style={{ borderRadius: shape === "circle" ? "50%" : "2px", transform: shape === "diamond" ? "rotate(45deg)" : undefined }}
      />
      {label}
    </span>
  );
}

function HealthBadgeScanning() {
  return (
    <span className="inline-flex items-center gap-1.5 text-[13.5px] font-semibold text-health-scanning">
      <RefreshCw className="size-3.5 animate-spin" />
      Scanning…
    </span>
  );
}

"use client";

import { useState } from "react";
import useSWR, { useSWRConfig } from "swr";
import { toast } from "sonner";
import { ExternalLink, GitBranch, RefreshCw } from "lucide-react";
import Link from "next/link";
import { getRepos, startIndexing, REPOS_SWR_KEY, parseRepoStatus, type RepoConnection } from "@/lib/api/repos-api";
import { repoHealthWithReason } from "@/lib/repo-health";
import { HealthBadge } from "@/components/dashboard/health-badge";
import { NodeCountBar } from "@/components/dashboard/node-count-bar";
import { BranchSelect } from "@/components/repositories/branch-select";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";

export function RepoTable() {
  const { data, isLoading } = useSWR(REPOS_SWR_KEY, getRepos);
  const repos = data?.connections;
  // The context-bound mutate (not the top-level `import { mutate } from
  // "swr"`) -- guarantees this always targets whatever cache this
  // component's own useSWR call actually reads from, rather than assuming
  // it's always the implicit default cache.
  const { mutate } = useSWRConfig();
  const [reindexingIds, setReindexingIds] = useState<Set<string>>(new Set());

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
    <Card className="border-border-strong bg-panel">
      <CardHeader>
        <CardTitle className="flex items-center gap-2 text-subheading">
          <GitBranch className="size-4 text-ink-500" />
          Repository Intelligence
        </CardTitle>
      </CardHeader>
      <CardContent>
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Repository</TableHead>
              <TableHead>Branch</TableHead>
              <TableHead>Health</TableHead>
              <TableHead>Nodes</TableHead>
              <TableHead>Status</TableHead>
              <TableHead className="text-right">Actions</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {isLoading && (
              <TableRow>
                <TableCell colSpan={6} className="text-center text-ink-500">
                  Loading…
                </TableCell>
              </TableRow>
            )}
            {!isLoading && repos?.length === 0 && (
              <TableRow>
                <TableCell colSpan={6} className="text-center text-ink-500">
                  No repositories connected yet — connect one from Settings to see it here.
                </TableCell>
              </TableRow>
            )}
            {repos?.map((repo) => {
              const reindexing = reindexingIds.has(repo.id);
              const status = parseRepoStatus(repo.status);
              return (
                <TableRow key={repo.id}>
                  <TableCell>
                    <div className="font-medium text-ink-100">{repo.id}</div>
                    <div className="truncate text-mono-path text-ink-500">{repo.repo_url}</div>
                  </TableCell>
                  <TableCell className="text-mono-code text-ink-300">
                    <BranchSelect repo={repo} onChanged={() => mutate(REPOS_SWR_KEY)} />
                  </TableCell>
                  <TableCell>{reindexing ? <HealthBadgeScanning /> : <HealthBadge {...repoHealthWithReason(repo)} />}</TableCell>
                  <TableCell>
                    {repo.counts ? (
                      <div className="flex flex-col gap-1">
                        <NodeCountBar counts={repo.counts} className="w-32" />
                        <span className="text-mono-code text-ink-500">{repo.counts.symbols + repo.counts.files + repo.counts.gotchas + repo.counts.decisions} total</span>
                      </div>
                    ) : (
                      <span className="text-mono-code text-ink-500">not yet scanned</span>
                    )}
                  </TableCell>
                  <TableCell className="text-mono-code text-ink-300">{status.kind === "failed" ? status.reason : status.kind}</TableCell>
                  <TableCell>
                    <div className="flex justify-end gap-1">
                      <Tooltip>
                        <TooltipTrigger asChild>
                          <Button variant="outline" size="icon" disabled={reindexing || repo.path_missing || status.kind !== "active"} onClick={() => handleReindex(repo)} aria-label="Rescan repository">
                            <RefreshCw className={reindexing ? "size-4 animate-spin" : "size-4"} />
                          </Button>
                        </TooltipTrigger>
                        <TooltipContent>{repo.path_missing ? "Repo path no longer exists" : "Rescan"}</TooltipContent>
                      </Tooltip>
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
                  </TableCell>
                </TableRow>
              );
            })}
          </TableBody>
        </Table>
      </CardContent>
    </Card>
  );
}

function HealthBadgeScanning() {
  return (
    <span className="inline-flex items-center gap-1.5 text-section font-medium text-health-scanning">
      <RefreshCw className="size-3.5 animate-spin" />
      Scanning…
    </span>
  );
}

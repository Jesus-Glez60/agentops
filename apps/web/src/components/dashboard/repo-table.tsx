"use client";

import { useState } from "react";
import useSWR, { useSWRConfig } from "swr";
import { toast } from "sonner";
import { ExternalLink, GitBranch, RefreshCw } from "lucide-react";
import { getRepos, rescanRepo, REPOS_SWR_KEY, type RepoSummary } from "@/lib/api/agentops-api";
import { repoHealth } from "@/lib/repo-health";
import { relativeTimeFromUnixSeconds } from "@/lib/relative-time";
import { HealthBadge } from "@/components/dashboard/health-badge";
import { NodeCountBar } from "@/components/dashboard/node-count-bar";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";

export function RepoTable() {
  const { data: repos, isLoading } = useSWR(REPOS_SWR_KEY, getRepos);
  // The context-bound mutate (not the top-level `import { mutate } from
  // "swr"`) -- guarantees this always targets whatever cache this
  // component's own useSWR call actually reads from, rather than assuming
  // it's always the implicit default cache.
  const { mutate } = useSWRConfig();
  const [scanningNames, setScanningNames] = useState<Set<string>>(new Set());

  async function handleRescan(repo: RepoSummary) {
    setScanningNames((prev) => new Set(prev).add(repo.name));
    try {
      await mutate(
        REPOS_SWR_KEY,
        async (current: RepoSummary[] | undefined) => {
          const updated = await rescanRepo(repo.name);
          return current?.map((r) => (r.name === repo.name ? updated : r));
        },
        { revalidate: false },
      );
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Rescan failed. Please try again.");
    } finally {
      setScanningNames((prev) => {
        const next = new Set(prev);
        next.delete(repo.name);
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
              <TableHead>Last scan</TableHead>
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
                  No repositories scanned yet — run <code className="text-mono-code">agentops install</code> in a repo to see it here.
                </TableCell>
              </TableRow>
            )}
            {repos?.map((repo) => {
              const scanning = scanningNames.has(repo.name);
              return (
                <TableRow key={repo.name}>
                  <TableCell>
                    <div className="font-medium text-ink-100">{repo.name}</div>
                    <div className="truncate text-mono-path text-ink-500">{repo.path}</div>
                  </TableCell>
                  <TableCell className="text-mono-code text-ink-300">{repo.branch ?? "—"}</TableCell>
                  <TableCell>{scanning ? <HealthBadgeScanning /> : <HealthBadge status={repoHealth(repo)} />}</TableCell>
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
                  <TableCell className="text-mono-code text-ink-300">{relativeTimeFromUnixSeconds(repo.last_scanned_at)}</TableCell>
                  <TableCell>
                    <div className="flex justify-end gap-1">
                      <Tooltip>
                        <TooltipTrigger asChild>
                          <Button variant="outline" size="icon" disabled={scanning || repo.path_missing} onClick={() => handleRescan(repo)} aria-label="Rescan repository">
                            <RefreshCw className={scanning ? "size-4 animate-spin" : "size-4"} />
                          </Button>
                        </TooltipTrigger>
                        <TooltipContent>{repo.path_missing ? "Repo path no longer exists" : "Rescan"}</TooltipContent>
                      </Tooltip>
                      <Tooltip>
                        <TooltipTrigger asChild>
                          {/* No repo-detail page exists yet -- same
                              "present but honestly non-functional"
                              precedent as the topbar's notification bell. */}
                          <Button variant="outline" size="icon" disabled aria-label="View details">
                            <ExternalLink className="size-4" />
                          </Button>
                        </TooltipTrigger>
                        <TooltipContent>Coming soon</TooltipContent>
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

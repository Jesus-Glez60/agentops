"use client";

import Link from "next/link";
import useSWR from "swr";
import { Database, GitBranch, Plus, ShieldAlert, Clock, ExternalLink, FileText, RefreshCw } from "lucide-react";
import { getRepos } from "@/lib/api/agentops-api";
import { getConnections } from "@/lib/api/heavy-api";
import { useTenant } from "@/lib/tenant-context";
import { repoHealth, connectionHealth } from "@/lib/repo-health";
import { relativeTimeFromUnixSeconds, relativeTimeFromIsoString } from "@/lib/relative-time";
import type { RepoSummary, ConnectionView } from "@/lib/api/types";
import { StatCard } from "@/components/shared/stat-card";
import { HealthBadge } from "@/components/shared/health-badge";
import { ProgressStack } from "@/components/shared/progress-stack";
import { ErrorState } from "@/components/shared/error-state";
import { EmptyState } from "@/components/shared/empty-state";
import { Skeleton } from "@/components/ui/skeleton";
import { Button } from "@/components/ui/button";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";

interface RepoRow {
  key: string;
  displayName: string;
  source: string;
  health: ReturnType<typeof repoHealth>;
  counts: RepoSummary["counts"];
  lastActivityLabel: string;
  lastActivityMs: number;
  graphHref?: string;
  docsHref?: string;
}

function localRowsFrom(repos: RepoSummary[]): RepoRow[] {
  return repos.map((r) => ({
    key: `local:${r.path}`,
    displayName: r.path,
    source: "local scan",
    health: repoHealth(r),
    counts: r.counts,
    lastActivityLabel: relativeTimeFromUnixSeconds(r.last_scanned_at),
    lastActivityMs: r.last_scanned_at * 1000,
    graphHref: `/graph?path=${encodeURIComponent(r.path)}`,
    docsHref: `/docs?path=${encodeURIComponent(r.path)}`,
  }));
}

function connectionRowsFrom(connections: ConnectionView[]): RepoRow[] {
  return connections.map((c) => ({
    key: `conn:${c.id}`,
    displayName: c.repo_url,
    source: c.method === "github_app" ? "GitHub App" : "SSH Deploy Key",
    health: connectionHealth(c.status),
    counts: null,
    lastActivityLabel: relativeTimeFromIsoString(c.created_at),
    lastActivityMs: new Date(c.created_at).getTime(),
  }));
}

export default function OverviewPage() {
  const { tenant, hasTenant } = useTenant();

  const { data: repos, error: reposError, isLoading: reposLoading } = useSWR("repos", getRepos);
  const {
    data: connections,
    error: connectionsError,
    isLoading: connectionsLoading,
  } = useSWR(hasTenant ? ["connections", tenant] : null, () => getConnections(tenant!));

  const isLoading = reposLoading || (hasTenant && connectionsLoading);
  const error = reposError ?? (hasTenant ? connectionsError : undefined);

  const rows: RepoRow[] = [...(repos ? localRowsFrom(repos) : []), ...(connections ? connectionRowsFrom(connections) : [])].sort(
    (a, b) => b.lastActivityMs - a.lastActivityMs,
  );

  const totalNodes = rows.reduce((sum, r) => sum + (r.counts ? r.counts.files + r.counts.symbols + r.counts.gotchas + r.counts.decisions : 0), 0);
  const totalGotchas = rows.reduce((sum, r) => sum + (r.counts?.gotchas ?? 0), 0);
  const staleCount = rows.filter((r) => r.health === "stale").length;

  // "Gotchas requiring review" has nowhere to click through to unless we pick
  // a specific repo's graph to jump into -- the aggregate count spans every
  // scanned repo, but the graph view only ever shows one at a time. Send the
  // user to whichever repo has the most gotchas, pre-filtered to the
  // Knowledge tab / Gotcha node kind, since that's the repo most worth
  // reviewing first.
  const topGotchaRow = rows
    .filter((r) => r.graphHref && (r.counts?.gotchas ?? 0) > 0)
    .sort((a, b) => (b.counts?.gotchas ?? 0) - (a.counts?.gotchas ?? 0))[0];
  const gotchasHref = topGotchaRow ? `${topGotchaRow.graphHref}&tab=knowledge&kind=Gotcha` : undefined;

  return (
    <div className="flex flex-col gap-6">
      <div className="flex items-center justify-between">
        <h1 className="text-page-title font-bold">Overview</h1>
        <Button asChild size="sm" className="gap-1.5">
          <Link href="/repos/connect">
            <Plus className="size-4" />
            Connect Repository
          </Link>
        </Button>
      </div>

      {error && <ErrorState message={error instanceof Error ? error.message : String(error)} />}

      <div className="grid grid-cols-2 gap-4 md:grid-cols-4">
        <StatCard label="Repositories" value={isLoading ? "—" : rows.length} icon={GitBranch} href="#repository-intelligence" />
        <StatCard
          label="Knowledge nodes"
          value={isLoading ? "—" : totalNodes.toLocaleString()}
          icon={Database}
          href="#repository-intelligence"
        />
        <StatCard
          label="Gotchas requiring review"
          value={isLoading ? "—" : totalGotchas}
          icon={ShieldAlert}
          valueClassName={totalGotchas > 0 ? "text-health-warning" : undefined}
          href={gotchasHref}
        />
        <StatCard
          label="Stale index"
          value={isLoading ? "—" : staleCount}
          icon={Clock}
          valueClassName={staleCount > 0 ? "text-health-stale" : undefined}
          href="#repository-intelligence"
        />
      </div>

      {!isLoading && rows.length > 0 && (
        <div className="flex items-center gap-4 overflow-x-auto rounded-md border border-border-strong bg-panel px-4 py-2 text-mono-code">
          {rows.slice(0, 6).map((row) => (
            <span key={row.key} className="flex shrink-0 items-center gap-1.5 text-ink-300">
              <span className="max-w-32 truncate text-ink-100">{row.displayName}</span>
              <span className="text-ink-500">{row.source} · {row.lastActivityLabel}</span>
            </span>
          ))}
        </div>
      )}

      <div id="repository-intelligence" className="scroll-mt-6 rounded-md border border-border-strong bg-panel">
        <div className="flex items-center justify-between border-b border-border-strong px-4 py-3">
          <h2 className="text-section font-medium text-ink-100">Repository Intelligence</h2>
        </div>

        {isLoading && (
          <div className="space-y-2 p-4">
            <Skeleton className="h-10 w-full" />
            <Skeleton className="h-10 w-full" />
            <Skeleton className="h-10 w-full" />
          </div>
        )}

        {!isLoading && rows.length === 0 && !error && (
          <EmptyState
            icon={GitBranch}
            title="No repositories yet"
            description={
              hasTenant
                ? "Run `agentops install --path <repo>` locally, or connect a hosted repo above."
                : "Run `agentops install --path <repo>` to index one, or set an organization to see hosted connections."
            }
          />
        )}

        {!isLoading && rows.length > 0 && (
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Repository</TableHead>
                <TableHead>Source</TableHead>
                <TableHead>Health</TableHead>
                <TableHead>Nodes</TableHead>
                <TableHead>Last activity</TableHead>
                <TableHead className="text-right">Actions</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {rows.map((row) => (
                <TableRow key={row.key}>
                  <TableCell className="max-w-xs truncate font-mono text-mono-path">{row.displayName}</TableCell>
                  <TableCell className="text-body text-ink-300">{row.source}</TableCell>
                  <TableCell>
                    <HealthBadge status={row.health} />
                  </TableCell>
                  <TableCell className="w-40">
                    {row.counts ? (
                      <div className="flex flex-col gap-1">
                        <ProgressStack counts={row.counts} />
                        <span className="text-mono-path text-ink-500">
                          {row.counts.files + row.counts.symbols + row.counts.gotchas + row.counts.decisions} nodes
                        </span>
                      </div>
                    ) : (
                      <span className="text-mono-path text-ink-500">—</span>
                    )}
                  </TableCell>
                  <TableCell className="text-body text-ink-300">{row.lastActivityLabel}</TableCell>
                  <TableCell className="text-right">
                    <div className="flex justify-end gap-1">
                      {row.graphHref && (
                        <Tooltip>
                          <TooltipTrigger asChild>
                            <Button asChild variant="ghost" size="icon">
                              <Link href={row.graphHref}>
                                <ExternalLink className="size-4" />
                              </Link>
                            </Button>
                          </TooltipTrigger>
                          <TooltipContent>Open graph</TooltipContent>
                        </Tooltip>
                      )}
                      {row.docsHref && (
                        <Tooltip>
                          <TooltipTrigger asChild>
                            <Button asChild variant="ghost" size="icon">
                              <Link href={row.docsHref}>
                                <FileText className="size-4" />
                              </Link>
                            </Button>
                          </TooltipTrigger>
                          <TooltipContent>Open docs</TooltipContent>
                        </Tooltip>
                      )}
                      {!row.graphHref && !row.docsHref && (
                        <Tooltip>
                          <TooltipTrigger asChild>
                            <Button variant="ghost" size="icon" disabled>
                              <RefreshCw className="size-4" />
                            </Button>
                          </TooltipTrigger>
                          <TooltipContent>No structural index yet for hosted connections</TooltipContent>
                        </Tooltip>
                      )}
                    </div>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        )}
      </div>
    </div>
  );
}

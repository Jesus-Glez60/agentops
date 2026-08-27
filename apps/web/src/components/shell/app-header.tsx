"use client";

import useSWR from "swr";
import { Bell, Database } from "lucide-react";
import { getRepos, REPOS_SWR_KEY } from "@/lib/api/repos-api";
import { repoHealth } from "@/lib/repo-health";
import { BreadcrumbHeader } from "@/components/shell/breadcrumb-header";
import { CommandPalette } from "@/components/shell/command-palette";
import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";

export function AppHeader() {
  // Same SWR key as the Overview page's repo table -- one shared cache
  // entry/one network request, not two independent fetches.
  const { data } = useSWR(REPOS_SWR_KEY, getRepos);
  const repos = data?.connections;

  const repoCount = repos?.length;
  const allHealthy = repos?.every((r) => repoHealth(r) === "healthy") ?? false;
  const anyIssue = repos?.some((r) => repoHealth(r) === "warning" || repoHealth(r) === "stale") ?? false;

  return (
    <header className="flex h-14 shrink-0 items-center justify-between gap-4 border-b border-border bg-panel px-4">
      <BreadcrumbHeader />

      <div className="flex items-center gap-2">
        <div className="flex items-center gap-2 rounded-md border border-border px-3 py-1.5 text-section text-ink-300">
          <Database className="size-3.5 text-ink-500" />
          All repositories{repoCount !== undefined && ` (${repoCount})`}
        </div>
        <div className="flex items-center gap-1.5 text-section text-ink-500">
          <span className={cn("size-1.5 rounded-full", repos === undefined ? "bg-ink-500" : anyIssue ? "bg-health-warning" : allHealthy ? "bg-health-healthy" : "bg-ink-500")} />
          {repos === undefined ? "—" : anyIssue ? "Needs attention" : allHealthy ? "All indexed" : "—"}
        </div>

        <CommandPalette />

        <Tooltip>
          <TooltipTrigger asChild>
            {/* No notifications backend exists -- rendered disabled/empty
                rather than showing a fake unread count. */}
            <Button variant="outline" size="icon" disabled aria-label="Notifications">
              <Bell className="size-4" />
            </Button>
          </TooltipTrigger>
          <TooltipContent>No notifications</TooltipContent>
        </Tooltip>
      </div>
    </header>
  );
}

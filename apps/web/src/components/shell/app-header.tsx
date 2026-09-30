"use client";

import Link from "next/link";
import useSWR from "swr";
import { Bell } from "lucide-react";
import { getRepos, REPOS_SWR_KEY } from "@/lib/api/repos-api";
import { repoHealth } from "@/lib/repo-health";
import type { SessionUser } from "@/lib/auth/types";
import { BreadcrumbHeader } from "@/components/shell/breadcrumb-header";
import { CommandPaletteTrigger } from "@/components/shell/command-palette";
import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";

export function AppHeader({ user, onOpenPalette }: { user: SessionUser; onOpenPalette: () => void }) {
  // Same SWR key as the Overview page's repo table -- one shared cache
  // entry/one network request, not two independent fetches.
  const { data } = useSWR(REPOS_SWR_KEY, getRepos);
  const repos = data?.connections;
  // The "All repositories (N)" pill this used to show is now redundant with
  // the sidebar's own ScopeSwitcher subtitle + repository list -- replaced
  // with the prototype's simpler "N repo(s) need attention" summary, shown
  // only when something actually does (never a fabricated "1 repo" the way
  // the prototype's own mockup copy hardcodes it).
  const unhealthyCount = repos?.filter((r) => repoHealth(r) !== "healthy").length ?? 0;

  return (
    <header className="flex h-14 shrink-0 items-center justify-between gap-4 border-b border-border bg-panel px-4">
      <BreadcrumbHeader user={user} />

      <div className="flex items-center gap-3">
        {unhealthyCount > 0 && (
          <span className="flex items-center gap-1.5 text-section text-ink-300">
            <span className="size-1.5 rounded-full bg-health-warning" />
            {unhealthyCount} repo{unhealthyCount === 1 ? "" : "s"} need{unhealthyCount === 1 ? "s" : ""} attention
          </span>
        )}
        <Link href="/docs" className="text-mono-path text-ink-500 hover:text-ink-300">
          Docs ↗
        </Link>

        <CommandPaletteTrigger onOpen={onOpenPalette} />

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

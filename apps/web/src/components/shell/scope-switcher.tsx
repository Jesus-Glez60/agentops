"use client";

import useSWR from "swr";
import { Check } from "lucide-react";
import { useRouter } from "next/navigation";
import type { SessionUser } from "@/lib/auth/types";
import type { RepoConnection } from "@/lib/api/repos-api";
import { getMyMemberships, resolveOrgDisplayName, MY_MEMBERSHIPS_SWR_KEY } from "@/lib/api/team-api";
import { useOrgSwitch } from "@/hooks/use-org-switch";
import { repoHealth } from "@/lib/repo-health";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { SidebarMenuButton } from "@/components/ui/sidebar";
import { cn } from "@/lib/utils";

const HEALTH_DOT_CLASSNAME: Record<ReturnType<typeof repoHealth>, string> = {
  healthy: "bg-health-healthy",
  warning: "bg-health-warning",
  stale: "bg-health-stale",
  "not-indexed": "bg-ink-500",
};

// Sidebar-header org switcher -- org identity, initial-avatar, dropdown of
// orgs the user belongs to, plus (below) a read-only repository list so the
// "All repositories · N" subtitle actually corresponds to something when
// clicked, rather than opening a menu with no repository content at all.
// Org switching used to live buried in UserMenu's dropdown; this is now the
// one place to do it (see redesign plan Phase 2). The repo list here is
// pure navigation (links to each repo's detail page), not a real cross-page
// scope/filter state -- that's still page-local (see search/gotchas'
// ScopeSelector), deliberately not invented at the shell level.
//
// Memberships are fetched unconditionally (not gated on the dropdown being
// open) -- this is the sidebar's own always-visible identity display, not an
// optional on-demand lookup, so the real org name must be ready on first
// paint rather than flashing the raw tenant slug until the user opens the
// menu once.
export function ScopeSwitcher({ user, repos }: { user: SessionUser; repos?: RepoConnection[] }) {
  const router = useRouter();
  const { data: membershipsData } = useSWR(MY_MEMBERSHIPS_SWR_KEY, getMyMemberships);
  const memberships = membershipsData?.memberships ?? [];
  const { switchOrg, switching } = useOrgSwitch(user);

  const currentOrgLabel = resolveOrgDisplayName(memberships, user);
  const scopeLabel = repos === undefined ? "—" : `All repositories · ${repos.length}`;

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <SidebarMenuButton
          size="lg"
          className="border bg-raised data-[state=open]:bg-sidebar-accent data-[state=open]:text-sidebar-accent-foreground"
        >
          <div
            className="flex size-[26px] shrink-0 items-center justify-center rounded-[7px] text-[12px] font-extrabold text-canvas"
            style={{ background: "linear-gradient(135deg, var(--mauve), var(--peach))" }}
          >
            {currentOrgLabel.charAt(0).toUpperCase()}
          </div>
          <div className="min-w-0 flex-1 group-data-[collapsible=icon]:hidden">
            <p className="truncate text-section text-ink-100">{currentOrgLabel}</p>
            <p className="truncate text-mono-path text-ink-500">{scopeLabel}</p>
          </div>
          <span className="ml-auto shrink-0 text-ink-500 group-data-[collapsible=icon]:hidden">▾</span>
        </SidebarMenuButton>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" side="bottom" className="w-56">
        {memberships.length > 1 && (
          <>
            <div className="px-2 pt-1.5 pb-1 text-mono-path text-ink-500">Organization</div>
            {memberships.map((m) => (
              <DropdownMenuItem key={m.tenant} onSelect={() => switchOrg(m.tenant)} disabled={switching !== null}>
                <span className="size-2 shrink-0 rounded-full bg-mauve" />
                <span className="min-w-0 flex-1 truncate">{m.name || m.tenant}</span>
                {m.tenant === user.tenant ? <Check className="size-4 shrink-0 text-ink-300" /> : switching === m.tenant ? <span className="text-mono-path text-ink-500">Switching…</span> : null}
              </DropdownMenuItem>
            ))}
            <DropdownMenuSeparator />
          </>
        )}
        {!!repos?.length && (
          <>
            <div className="px-2 pt-1.5 pb-1 text-mono-path text-ink-500">Repositories</div>
            <DropdownMenuItem onSelect={() => router.push("/repositories")}>
              <span className="size-2 rounded-full bg-mauve" />
              All repositories
              <span className="ml-auto text-mono-path text-ink-500">{repos.length}</span>
            </DropdownMenuItem>
            {repos.map((repo) => (
              <DropdownMenuItem key={repo.id} onSelect={() => router.push(`/repositories/${encodeURIComponent(repo.id)}`)}>
                <span className={cn("size-2 shrink-0 rounded-full", HEALTH_DOT_CLASSNAME[repoHealth(repo)])} />
                <span className="min-w-0 flex-1 truncate">{repo.id}</span>
              </DropdownMenuItem>
            ))}
            <DropdownMenuSeparator />
          </>
        )}
        <DropdownMenuItem onSelect={() => router.push("/repositories/connect")} className="text-mauve">
          + Connect repository
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

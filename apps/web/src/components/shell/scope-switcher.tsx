"use client";

import useSWR from "swr";
import { Building2, Check, ChevronsUpDown, Plug } from "lucide-react";
import { useRouter } from "next/navigation";
import type { SessionUser } from "@/lib/auth/types";
import { getMyMemberships, resolveOrgDisplayName, MY_MEMBERSHIPS_SWR_KEY } from "@/lib/api/team-api";
import { useOrgSwitch } from "@/hooks/use-org-switch";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { SidebarMenuButton } from "@/components/ui/sidebar";

// Sidebar-header org switcher -- org identity, initial-avatar, dropdown of
// orgs the user belongs to. Org switching used to live buried in UserMenu's
// dropdown; this is now the one place to do it (see redesign plan Phase 2).
// Deliberately org-only: there's no shell-level "repo scope" concept in this
// app today (repo filtering is page-local, see search/gotchas' ScopeSelector),
// so this doesn't invent one.
//
// Memberships are fetched unconditionally (not gated on the dropdown being
// open) -- this is the sidebar's own always-visible identity display, not an
// optional on-demand lookup, so the real org name must be ready on first
// paint rather than flashing the raw tenant slug until the user opens the
// menu once.
export function ScopeSwitcher({ user, repoCount }: { user: SessionUser; repoCount?: number }) {
  const router = useRouter();
  const { data: membershipsData } = useSWR(MY_MEMBERSHIPS_SWR_KEY, getMyMemberships);
  const memberships = membershipsData?.memberships ?? [];
  const { switchOrg, switching } = useOrgSwitch(user);

  const currentOrgLabel = resolveOrgDisplayName(memberships, user);
  const scopeLabel = repoCount === undefined ? "—" : `All repositories · ${repoCount}`;

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <SidebarMenuButton size="lg" className="data-[state=open]:bg-sidebar-accent data-[state=open]:text-sidebar-accent-foreground">
          <div className="flex size-6 shrink-0 items-center justify-center rounded-md bg-mauve text-canvas text-label font-bold">{currentOrgLabel.charAt(0).toUpperCase()}</div>
          <div className="min-w-0 flex-1 group-data-[collapsible=icon]:hidden">
            <p className="truncate text-section text-ink-100">{currentOrgLabel}</p>
            <p className="truncate text-mono-path text-ink-500">{scopeLabel}</p>
          </div>
          <ChevronsUpDown className="ml-auto size-4 shrink-0 text-ink-500 group-data-[collapsible=icon]:hidden" />
        </SidebarMenuButton>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" side="bottom" className="w-56">
        {memberships.length > 1 && (
          <>
            <div className="px-2 pt-1.5 pb-1 text-mono-path text-ink-500">Organization</div>
            {memberships.map((m) => (
              <DropdownMenuItem key={m.tenant} onSelect={() => switchOrg(m.tenant)} disabled={switching !== null}>
                <Building2 className="size-4" />
                <span className="min-w-0 flex-1 truncate">{m.name || m.tenant}</span>
                {m.tenant === user.tenant ? <Check className="size-4 shrink-0 text-ink-300" /> : switching === m.tenant ? <span className="text-mono-path text-ink-500">Switching…</span> : null}
              </DropdownMenuItem>
            ))}
            <DropdownMenuSeparator />
          </>
        )}
        <DropdownMenuItem onSelect={() => router.push("/repositories/connect")}>
          <Plug className="size-4" />
          Connect repository
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

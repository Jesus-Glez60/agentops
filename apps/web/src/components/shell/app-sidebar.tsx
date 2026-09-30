"use client";

import { useState } from "react";
import Link from "next/link";
import { usePathname, useRouter } from "next/navigation";
import useSWR from "swr";
import { LogOut, Plug, Search } from "lucide-react";
import { NAV_GROUP_ORDER, NAV_ITEMS } from "@/lib/nav-config";
import { getRepos, REPOS_SWR_KEY } from "@/lib/api/repos-api";
import { logout } from "@/lib/auth/client";
import type { SessionUser } from "@/lib/auth/types";
import { UserMenu } from "@/components/shell/user-menu";
import { ScopeSwitcher } from "@/components/shell/scope-switcher";
import { LogoMark } from "@/components/shared/logo-mark";
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarGroupContent,
  SidebarGroupLabel,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuBadge,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarTrigger,
} from "@/components/ui/sidebar";

export function AppSidebar({ user, onOpenPalette }: { user: SessionUser; onOpenPalette: () => void }) {
  const pathname = usePathname();
  const router = useRouter();
  const [loggingOut, setLoggingOut] = useState(false);
  // Same shared cache the Overview page/topbar already read -- no new fetch
  // just to show the Gotchas nav badge, and now also the ScopeSwitcher's
  // "All repositories · N" subtitle.
  const { data } = useSWR(REPOS_SWR_KEY, getRepos);
  const gotchasNeedingCuration = data?.connections.reduce((sum, r) => sum + (r.counts?.gotchas_needing_curation ?? 0), 0);

  async function handleLogout() {
    setLoggingOut(true);
    try {
      await logout();
    } finally {
      router.push("/login");
      router.refresh();
    }
  }

  // NAV_ITEMS stays in its flat design-mock order; group here by filtering
  // rather than requiring the source array to be group-contiguous.
  const ungroupedItems = NAV_ITEMS.filter((item) => !item.group);

  function renderItem(item: (typeof NAV_ITEMS)[number]) {
    const active = pathname === item.href;
    return (
      <SidebarMenuItem key={item.href}>
        <SidebarMenuButton asChild isActive={active} tooltip={item.label}>
          <Link href={item.href}>
            <item.icon />
            <span>{item.label}</span>
          </Link>
        </SidebarMenuButton>
        {item.href === "/gotchas" && !!gotchasNeedingCuration && <SidebarMenuBadge>{gotchasNeedingCuration}</SidebarMenuBadge>}
      </SidebarMenuItem>
    );
  }

  return (
    <Sidebar variant="sidebar" collapsible="icon">
      <SidebarHeader>
        <div className="flex items-center justify-between px-1">
          <Link href="/" className="flex items-center gap-2">
            <LogoMark className="size-5 shrink-0" />
            <span className="text-page-title font-bold group-data-[collapsible=icon]:hidden">AgentOps</span>
          </Link>
          <SidebarTrigger className="group-data-[collapsible=icon]:hidden" />
        </div>
        <SidebarMenu>
          <SidebarMenuItem>
            <ScopeSwitcher user={user} repos={data?.connections} />
          </SidebarMenuItem>
          <SidebarMenuItem>
            <SidebarMenuButton onClick={onOpenPalette} tooltip="Search or jump to…" className="text-ink-500">
              <Search />
              <span className="flex-1 text-left">Search or jump to…</span>
              <span className="rounded border border-border-strong px-1.5 text-mono-path text-ink-500 group-data-[collapsible=icon]:hidden">⌘K</span>
            </SidebarMenuButton>
          </SidebarMenuItem>
        </SidebarMenu>
      </SidebarHeader>

      <SidebarContent>
        {ungroupedItems.length > 0 && (
          <SidebarGroup>
            <SidebarGroupContent>
              <SidebarMenu>{ungroupedItems.map(renderItem)}</SidebarMenu>
            </SidebarGroupContent>
          </SidebarGroup>
        )}
        {NAV_GROUP_ORDER.map((group) => {
          const items = NAV_ITEMS.filter((item) => item.group === group);
          if (items.length === 0) return null;
          return (
            <SidebarGroup key={group}>
              <SidebarGroupLabel>{group}</SidebarGroupLabel>
              <SidebarGroupContent>
                <SidebarMenu>{items.map(renderItem)}</SidebarMenu>
              </SidebarGroupContent>
            </SidebarGroup>
          );
        })}
      </SidebarContent>

      {/* Unlike before, the footer itself is never force-hidden in icon mode
          -- its pieces degrade individually the same way nav items do
          (icon-only, label carried in the `tooltip` prop instead), so the
          profile avatar and a compact log-out control stay reachable even
          when the sidebar is collapsed, matching the prototype's Rail mode. */}
      <SidebarFooter>
        <SidebarMenu>
          <SidebarMenuItem>
            <SidebarMenuButton asChild tooltip="Connect a coding tool" className="h-auto flex-col items-start gap-0.5 border border-mauve/35 bg-mauve/8 py-2.5">
              <Link href="/welcome">
                <div className="flex w-full items-center gap-2">
                  <Plug className="size-4 shrink-0" />
                  <span className="text-section font-medium">Connect a coding tool</span>
                </div>
                <span className="pl-6 text-mono-path text-ink-500 group-data-[collapsible=icon]:hidden">Claude Code, Cursor, Codex CLI, Gemini CLI</span>
              </Link>
            </SidebarMenuButton>
          </SidebarMenuItem>
        </SidebarMenu>
        <div className="flex items-center gap-1 group-data-[collapsible=icon]:flex-col">
          <div className="min-w-0 flex-1">
            <UserMenu user={user} />
          </div>
          <SidebarMenuButton onClick={handleLogout} disabled={loggingOut} tooltip={loggingOut ? "Logging out…" : "Log out"} aria-label={loggingOut ? "Logging out…" : "Log out"} className="w-auto shrink-0">
            <LogOut className="size-4" />
          </SidebarMenuButton>
        </div>
      </SidebarFooter>
    </Sidebar>
  );
}

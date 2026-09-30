"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import useSWR from "swr";
import { NAV_GROUP_ORDER, NAV_ITEMS } from "@/lib/nav-config";
import { getRepos, REPOS_SWR_KEY } from "@/lib/api/repos-api";
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

export function AppSidebar({ user }: { user: SessionUser }) {
  const pathname = usePathname();
  // Same shared cache the Overview page/topbar already read -- no new fetch
  // just to show the Gotchas nav badge.
  const { data } = useSWR(REPOS_SWR_KEY, getRepos);
  const gotchasNeedingCuration = data?.connections.reduce((sum, r) => sum + (r.counts?.gotchas_needing_curation ?? 0), 0);

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
            <ScopeSwitcher user={user} />
          </SidebarMenuItem>
        </SidebarMenu>
      </SidebarHeader>

      <SidebarContent>
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
        {ungroupedItems.length > 0 && (
          <SidebarGroup>
            <SidebarGroupContent>
              <SidebarMenu>{ungroupedItems.map(renderItem)}</SidebarMenu>
            </SidebarGroupContent>
          </SidebarGroup>
        )}
      </SidebarContent>

      <SidebarFooter className="group-data-[collapsible=icon]:hidden">
        <UserMenu user={user} />
      </SidebarFooter>
    </Sidebar>
  );
}

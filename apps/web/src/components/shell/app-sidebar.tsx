"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import useSWR from "swr";
import { Zap } from "lucide-react";
import { NAV_ITEMS } from "@/lib/nav-config";
import { getRepos, REPOS_SWR_KEY } from "@/lib/api/repos-api";
import type { SessionUser } from "@/lib/auth/types";
import { UserMenu } from "@/components/shell/user-menu";
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarGroupContent,
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

  return (
    <Sidebar variant="sidebar" collapsible="icon">
      <SidebarHeader>
        <div className="flex items-center justify-between px-1">
          <Link href="/" className="flex items-center gap-2">
            <div className="flex size-6 shrink-0 items-center justify-center rounded-md bg-primary text-primary-foreground">
              <Zap className="size-3.5 fill-current" />
            </div>
            <span className="text-page-title font-bold group-data-[collapsible=icon]:hidden">AgentOps</span>
          </Link>
          <SidebarTrigger className="group-data-[collapsible=icon]:hidden" />
        </div>
      </SidebarHeader>

      <SidebarContent>
        <SidebarGroup>
          <SidebarGroupContent>
            <SidebarMenu>
              {NAV_ITEMS.map((item) => {
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
              })}
            </SidebarMenu>
          </SidebarGroupContent>
        </SidebarGroup>
      </SidebarContent>

      <SidebarFooter className="group-data-[collapsible=icon]:hidden">
        <UserMenu user={user} />
      </SidebarFooter>
    </Sidebar>
  );
}

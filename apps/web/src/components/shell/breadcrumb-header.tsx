"use client";

import { usePathname } from "next/navigation";
import useSWR from "swr";
import { navLabelForPath } from "@/lib/nav-config";
import { getMyMemberships, resolveOrgDisplayName, MY_MEMBERSHIPS_SWR_KEY } from "@/lib/api/team-api";
import type { SessionUser } from "@/lib/auth/types";
import { Breadcrumb, BreadcrumbItem, BreadcrumbLink, BreadcrumbList, BreadcrumbPage, BreadcrumbSeparator } from "@/components/ui/breadcrumb";

export function BreadcrumbHeader({ user }: { user: SessionUser }) {
  const pathname = usePathname();
  const isRoot = pathname === "/";
  const label = navLabelForPath(pathname);
  // Same cache key ScopeSwitcher already reads -- SWR dedupes this, no
  // extra request, just the org name available here too for the
  // "{org} / {page}" breadcrumb prefix the prototype's header uses.
  const { data: membershipsData } = useSWR(MY_MEMBERSHIPS_SWR_KEY, getMyMemberships);
  const orgName = resolveOrgDisplayName(membershipsData?.memberships ?? [], user);

  return (
    <Breadcrumb>
      <BreadcrumbList>
        <BreadcrumbItem className="text-ink-500">{orgName}</BreadcrumbItem>
        <BreadcrumbSeparator />
        <BreadcrumbItem>{isRoot ? <BreadcrumbPage>Overview</BreadcrumbPage> : <BreadcrumbLink href="/">Overview</BreadcrumbLink>}</BreadcrumbItem>
        {!isRoot && (
          <>
            <BreadcrumbSeparator />
            <BreadcrumbItem>
              <BreadcrumbPage>{label}</BreadcrumbPage>
            </BreadcrumbItem>
          </>
        )}
      </BreadcrumbList>
    </Breadcrumb>
  );
}

"use client";

import { usePathname } from "next/navigation";
import { navLabelForPath } from "@/lib/nav-config";
import { Breadcrumb, BreadcrumbItem, BreadcrumbLink, BreadcrumbList, BreadcrumbPage, BreadcrumbSeparator } from "@/components/ui/breadcrumb";

export function BreadcrumbHeader() {
  const pathname = usePathname();
  const isRoot = pathname === "/";
  const label = navLabelForPath(pathname);

  return (
    <Breadcrumb>
      <BreadcrumbList>
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

import type { ReactNode } from "react";
import { Sidebar } from "@/components/shell/sidebar";
import { BreadcrumbHeader } from "@/components/shell/breadcrumb-header";

export function AppShell({ children }: { children: ReactNode }) {
  return (
    <div className="flex h-full min-h-screen w-full">
      <Sidebar />
      <div className="flex min-w-0 flex-1 flex-col">
        <BreadcrumbHeader />
        <main className="flex-1 overflow-y-auto p-6">{children}</main>
      </div>
    </div>
  );
}

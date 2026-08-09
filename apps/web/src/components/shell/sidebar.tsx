"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import { NAV_ITEMS } from "@/lib/nav-config";
import { cn } from "@/lib/utils";
import { OrgSwitcher } from "@/components/shell/org-switcher";
import { Avatar, AvatarFallback } from "@/components/ui/avatar";

export function Sidebar() {
  const pathname = usePathname();

  return (
    <aside className="flex h-full w-60 shrink-0 flex-col border-r border-border bg-panel">
      <div className="flex items-center gap-2 border-b border-border px-4 py-4">
        <div className="flex size-6 items-center justify-center rounded-md bg-primary text-xs font-bold text-primary-foreground">
          A
        </div>
        <span className="text-page-title font-bold">AgentOps</span>
      </div>

      <div className="border-b border-border px-3 py-3">
        <OrgSwitcher />
      </div>

      <nav className="flex-1 space-y-1 overflow-y-auto px-2 py-3">
        {NAV_ITEMS.map((item) => {
          const active = pathname === item.href;
          const Icon = item.icon;
          return (
            <Link
              key={item.href}
              href={item.href}
              className={cn(
                "flex items-center gap-2 rounded-md px-3 py-2 text-section transition-colors",
                active ? "bg-accent text-accent-foreground" : "text-ink-300 hover:bg-accent/50 hover:text-ink-100",
              )}
            >
              <Icon className="size-4" />
              {item.label}
            </Link>
          );
        })}
      </nav>

      <div className="flex items-center gap-2 border-t border-border px-4 py-3">
        <Avatar className="size-7">
          <AvatarFallback className="text-label">?</AvatarFallback>
        </Avatar>
        <div className="min-w-0">
          <p className="truncate text-section text-ink-100">Not signed in</p>
          <p className="truncate text-mono-path text-ink-500">local session</p>
        </div>
      </div>
    </aside>
  );
}

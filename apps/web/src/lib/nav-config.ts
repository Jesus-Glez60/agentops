import type { LucideIcon } from "lucide-react";
import { BookOpen, LayoutDashboard, Library, Search, Settings, Workflow } from "lucide-react";

export interface NavItem {
  href: string;
  label: string;
  icon: LucideIcon;
}

// Single source of truth for the sidebar, the command palette, and
// breadcrumb label lookups -- one list, three consumers, so adding a page
// never means updating three places by hand.
export const NAV_ITEMS: NavItem[] = [
  { href: "/", label: "Overview", icon: LayoutDashboard },
  { href: "/search", label: "Search", icon: Search },
  { href: "/graph", label: "Knowledge Graph", icon: Workflow },
  { href: "/docs", label: "Documentation", icon: BookOpen },
  { href: "/libraries", label: "Libraries", icon: Library },
  { href: "/settings", label: "Settings", icon: Settings },
];

export function navLabelForPath(pathname: string): string {
  const match = NAV_ITEMS.find((item) => item.href === pathname);
  if (match) return match.label;
  // Fallback for sub-routes (e.g. /libraries/prisma) not in the flat nav list.
  const segment = pathname.split("/").filter(Boolean).pop();
  if (!segment) return "Overview";
  return segment
    .split("-")
    .map((word) => word.charAt(0).toUpperCase() + word.slice(1))
    .join(" ");
}

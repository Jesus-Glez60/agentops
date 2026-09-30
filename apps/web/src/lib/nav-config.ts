import type { LucideIcon } from "lucide-react";
import { BookOpen, GitBranch, LayoutDashboard, Library, Search, Settings, TriangleAlert, Workflow } from "lucide-react";

export interface NavItem {
  href: string;
  label: string;
  icon: LucideIcon;
  // Sidebar section this item renders under (see NAV_GROUP_ORDER). Omitted
  // for items that render in their own unlabeled trailing group (Settings).
  group?: string;
}

// Rendering order for grouped sidebar sections -- NAV_ITEMS itself stays in
// its original flat order (command palette / breadcrumb lookups don't care
// about grouping, and a design-mock-order test pins that array), so the
// sidebar clusters by this list instead of relying on NAV_ITEMS being
// group-contiguous.
export const NAV_GROUP_ORDER = ["Explore", "Curate", "Sources", "Workspace"] as const;

// Single source of truth for the sidebar, the command palette, and
// breadcrumb label lookups -- one list, three consumers, so adding a page
// never means updating three places by hand.
export const NAV_ITEMS: NavItem[] = [
  { href: "/", label: "Overview", icon: LayoutDashboard },
  { href: "/search", label: "Search", icon: Search, group: "Explore" },
  { href: "/graph", label: "Knowledge Graph", icon: Workflow, group: "Explore" },
  { href: "/docs", label: "Documentation", icon: BookOpen, group: "Explore" },
  { href: "/libraries", label: "Libraries", icon: Library, group: "Sources" },
  { href: "/repositories", label: "Repositories", icon: GitBranch, group: "Sources" },
  { href: "/gotchas", label: "Gotchas", icon: TriangleAlert, group: "Curate" },
  { href: "/settings", label: "Settings", icon: Settings, group: "Workspace" },
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

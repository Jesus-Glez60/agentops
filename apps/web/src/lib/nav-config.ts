export type NavShape = "circle" | "square" | "diamond";

export interface NavItem {
  href: string;
  label: string;
  // Color + shape marker, not an icon -- matches the prototype's own `nav`
  // array (`item(route, label, short, color, shape)`): every nav row is a
  // small colored dot/square/diamond, never a lucide icon.
  color: string;
  shape: NavShape;
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
// never means updating three places by hand. Colors/shapes are exact
// values from the prototype's own `nav` array.
export const NAV_ITEMS: NavItem[] = [
  { href: "/", label: "Overview", color: "var(--mauve)", shape: "circle" },
  { href: "/search", label: "Search", color: "var(--blue-500)", shape: "circle", group: "Explore" },
  { href: "/graph", label: "Knowledge Graph", color: "var(--lavender)", shape: "circle", group: "Explore" },
  { href: "/docs", label: "Documentation", color: "var(--teal-400)", shape: "square", group: "Explore" },
  { href: "/libraries", label: "Libraries", color: "var(--yellow)", shape: "square", group: "Sources" },
  { href: "/repositories", label: "Repositories", color: "var(--node-file)", shape: "square", group: "Sources" },
  { href: "/gotchas", label: "Gotchas", color: "var(--peach)", shape: "diamond", group: "Curate" },
  { href: "/settings", label: "Settings", color: "var(--ink-500)", shape: "circle", group: "Workspace" },
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

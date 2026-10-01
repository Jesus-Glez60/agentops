import { clsx, type ClassValue } from "clsx"
import { extendTailwindMerge } from "tailwind-merge"

// Plain `twMerge` has no idea this project's `globals.css` `@theme inline`
// block defines custom named utilities (`text-stat-value`, `text-ink-100`,
// ...) -- Tailwind v4's CSS-based theming isn't visible to tailwind-merge's
// static default config at all. Confirmed empirically (not assumed): every
// unrecognized `text-{word}` utility falls into one shared "unknown text
// utility" bucket in twMerge's default heuristic, so `cn("text-stat-value",
// "text-ink-100")` -- a font-size token plus a color token, two completely
// different CSS properties -- silently collapsed to just `text-ink-100`,
// dropping the font-size class entirely. This was a real, confirmed bug
// (not a hypothesis) caught via direct DOM inspection: a rendered
// `<span class="font-extrabold text-ink-100">` with no size class at all,
// despite the component's source clearly including `text-stat-value`.
// It silently broke font sizing on most of this redesign's headings/values
// any time a custom size token and a custom color token landed in the same
// `cn()` call -- which is nearly everywhere. Registering both lists here,
// matching `globals.css`'s own `@theme inline` block exactly, is the fix:
// it tells tailwind-merge these belong to the real `font-size`/`text-color`
// conflict groups, so it only dedupes within each group (the correct
// behavior) instead of across them.
//
// Whenever a new named token is added to `globals.css`, add it to the
// matching list below too, or it silently reintroduces this exact bug.
const customTwMerge = extendTailwindMerge({
  extend: {
    classGroups: {
      "font-size": [
        {
          text: ["page-title", "subheading", "section", "body", "mono-path", "mono-code", "label", "display-auth", "display-hero", "display-card", "heading-lg", "stat-value", "body-lg"],
        },
      ],
      "text-color": [
        {
          text: [
            "canvas",
            "panel",
            "raised",
            "border-strong",
            "ink-100",
            "ink-200",
            "ink-300",
            "ink-400",
            "ink-500",
            "ink-600",
            "slate-blue",
            "mauve",
            "pink",
            "peach",
            "yellow",
            "lavender",
            "node-symbol",
            "node-file",
            "node-gotcha",
            "node-decision",
            "node-note",
            "node-hotspot",
            "health-healthy",
            "health-scanning",
            "health-warning",
            "health-stale",
            "health-failed",
            "health-local",
            "relevance-strong",
            "relevance-related",
            "relevance-supporting",
            "curation-needs-curation",
            "curation-kept",
            "curation-reduced",
            "curation-pinned",
            "background",
            "foreground",
            "card",
            "card-foreground",
            "popover",
            "popover-foreground",
            "primary",
            "primary-foreground",
            "secondary",
            "secondary-foreground",
            "muted",
            "muted-foreground",
            "accent",
            "accent-foreground",
            "destructive",
            "destructive-foreground",
            "border",
            "input",
            "ring",
            "sidebar",
            "sidebar-foreground",
            "sidebar-primary",
            "sidebar-primary-foreground",
            "sidebar-accent",
            "sidebar-accent-foreground",
            "sidebar-border",
            "sidebar-ring",
          ],
        },
      ],
    },
  },
})

export function cn(...inputs: ClassValue[]) {
  return customTwMerge(clsx(inputs))
}

/** Backend `repo` strings (`ActivityEvent.repo`/`GotchaSummary.repo`/`SearchResult.repo`/`NodeDetail.repo`) are `<connection_id>--<32-hex-char-tenant-id>` since `checkout_path` (agentops-heavy-api's Rust source of truth) must embed the tenant to stay collision-free across tenants sharing a connection id -- see that function's doc comment. Strips the tenant suffix for human display only; the full string is still what identity comparisons (`selected?.repo === x.repo`) and SWR keys use, unchanged. */
export function displayRepoName(repo: string): string {
  return repo.replace(/--[0-9a-f]{32}$/, "")
}

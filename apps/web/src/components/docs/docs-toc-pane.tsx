import { Share2 } from "lucide-react";
import type { DocSection } from "@/lib/api/repos-api";
import { cn } from "@/lib/utils";

/** Right 240px pane -- a flat "On this page" anchor list of every section
 * title (the design's `dc.toc` is a flat list, not a scrollspy of in-prose
 * sub-headings, which the API doesn't model anyway -- `DocSection` has one
 * `title` per section, no sub-heading structure), plus the same "View in
 * graph" action already used elsewhere on this screen and a one-line
 * footnote matching the centrality-ordering copy already shown in the
 * center pane. Same fixed-width + independent-scroll pattern as the left
 * `DocsNav` pane, mirrored on the opposite border. */
export function DocsTocPane({
  sections,
  activeSectionId,
  onSelectSection,
  onViewInGraph,
}: {
  sections: DocSection[];
  activeSectionId: string;
  onSelectSection: (id: string) => void;
  onViewInGraph: () => void;
}) {
  return (
    <aside className="hidden h-full w-[240px] shrink-0 flex-col gap-3 overflow-y-auto border-l border-border-strong px-4 py-4 lg:flex">
      <p className="text-label uppercase tracking-wide text-ink-500">On this page</p>
      <nav className="flex flex-col gap-0.5">
        {sections.map((s) => (
          <button
            key={s.id}
            type="button"
            onClick={() => onSelectSection(s.id)}
            className={cn(
              "truncate rounded px-1.5 py-1 text-left text-section transition-colors",
              activeSectionId === s.id ? "font-medium text-primary" : "text-ink-500 hover:text-ink-200",
            )}
          >
            {s.title}
          </button>
        ))}
      </nav>
      <button type="button" onClick={onViewInGraph} className="mt-1 flex items-center gap-1.5 self-start text-section text-ink-400 transition-colors hover:text-ink-100">
        <Share2 className="size-3.5" />
        View in graph
      </button>
      <p className="mt-auto text-label text-ink-500">Sections are ordered by graph centrality.</p>
    </aside>
  );
}

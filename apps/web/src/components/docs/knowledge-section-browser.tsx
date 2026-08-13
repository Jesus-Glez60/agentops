import { ChevronLeft, ChevronRight } from "lucide-react";
import type { DocBlock } from "@/lib/api/agentops-api";
import { kindLabel } from "@/lib/node-detail-formatting";
import { KnowledgeCallout, CALLOUT_STYLE } from "@/components/docs/knowledge-callout";

type CalloutBlock = Extract<DocBlock, { block_type: "knowledge_callout" }>;

/**
 * A section made entirely of knowledge callouts (Notes/Known Gotchas/
 * Architectural Decisions) is a list of individually-long entries, not one
 * page of prose -- rendering all of them stacked forced 30+ full gotcha
 * write-ups into a single scroll with no breathing room. This renders a
 * compact clickable list instead; picking one shows just that entry, full
 * width, with Previous/Next paging at the bottom to move through the rest
 * without going back to the list every time.
 *
 * Controlled, not self-managed: `selected` is driven by `DocsPageInner`
 * (lifted so the left nav's book-index sub-list -- `docs-nav.tsx` -- can
 * also open/advance the same item, not just this component's own list).
 */
export function KnowledgeSectionBrowser({
  blocks,
  selected,
  onSelect,
  onViewInGraph,
}: {
  blocks: CalloutBlock[];
  selected: number | null;
  onSelect: (index: number | null) => void;
  onViewInGraph?: (nodeId: number) => void;
}) {
  if (selected === null) {
    return (
      <div className="flex flex-col gap-2">
        {blocks.map((block, i) => {
          const { colorClass, Icon } = CALLOUT_STYLE[block.kind as keyof typeof CALLOUT_STYLE] ?? CALLOUT_STYLE.Note;
          const preview = block.body.split("\n").find((line) => line.trim().length > 0) ?? "";
          return (
            <button
              key={i}
              type="button"
              onClick={() => onSelect(i)}
              className={`rounded-md border px-4 py-3 text-left transition-colors hover:brightness-125 ${colorClass}`}
            >
              <div className="mb-1 flex items-center gap-2">
                <Icon className="size-3.5 shrink-0" />
                <span className="text-label font-semibold uppercase tracking-wide">{kindLabel(block.kind)}</span>
                {block.affects && (
                  <>
                    <span className="text-ink-500">·</span>
                    <span className="truncate text-mono-code opacity-70">{block.affects}</span>
                  </>
                )}
              </div>
              <p className="font-medium text-ink-100">{block.title}</p>
              <p className="mt-1 truncate text-body text-ink-400">{preview}</p>
            </button>
          );
        })}
      </div>
    );
  }

  const block = blocks[selected];
  return (
    <div>
      <button type="button" onClick={() => onSelect(null)} className="flex items-center gap-1 text-mono-code text-ink-400 transition-colors hover:text-ink-100">
        <ChevronLeft className="size-3" /> Back to list
      </button>

      <KnowledgeCallout block={block} onViewInGraph={onViewInGraph} />

      <div className="mt-2 flex items-center justify-between border-t border-border-strong pt-3">
        <button
          type="button"
          disabled={selected === 0}
          onClick={() => onSelect(selected - 1)}
          className="flex items-center gap-1 text-mono-code text-ink-400 transition-colors hover:text-ink-100 disabled:opacity-30 disabled:hover:text-ink-400"
        >
          <ChevronLeft className="size-3" /> Previous
        </button>
        <span className="text-mono-code text-ink-500">
          {selected + 1} of {blocks.length}
        </span>
        <button
          type="button"
          disabled={selected === blocks.length - 1}
          onClick={() => onSelect(selected + 1)}
          className="flex items-center gap-1 text-mono-code text-ink-400 transition-colors hover:text-ink-100 disabled:opacity-30 disabled:hover:text-ink-400"
        >
          Next <ChevronRight className="size-3" />
        </button>
      </div>
    </div>
  );
}

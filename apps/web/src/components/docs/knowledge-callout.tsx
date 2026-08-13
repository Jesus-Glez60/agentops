import { FileText, Lightbulb, Share2, TriangleAlert } from "lucide-react";
import type { DocBlock } from "@/lib/api/agentops-api";
import { kindLabel } from "@/lib/node-detail-formatting";
import { Prose } from "@/components/shared/prose";

type KnowledgeCalloutBlock = Extract<DocBlock, { block_type: "knowledge_callout" }>;

/** Exported so `knowledge-section-browser.tsx`'s list rows use the exact
 * same kind → icon/color mapping instead of a second copy. */
export const CALLOUT_STYLE = {
  Gotcha: { colorClass: "border-node-gotcha/40 bg-node-gotcha/5 text-node-gotcha", Icon: TriangleAlert },
  Decision: { colorClass: "border-node-decision/35 bg-node-decision/5 text-node-decision", Icon: Lightbulb },
  // `Note` covers both `context` and `knowledge`-typed vault notes -- see
  // `kindLabel`, which already displays this kind as "Docs" everywhere else
  // in the app (search results, connected-node rows); this callout matches
  // that convention rather than inventing a second label for the same kind.
  Note: { colorClass: "border-node-note/40 bg-node-note/5 text-node-note", Icon: FileText },
} as const;

/**
 * Inline Gotcha/Decision/Note card for the Documentation Viewer's center
 * pane. No standalone callout component existed to reuse (checked) -- the
 * closest precedent is `node-detail-sections.tsx`'s "Reduced prominence"
 * warning box, whose border/background-opacity convention (`border-X/40
 * bg-X/10`, `--node-*` color tokens) this follows rather than the design
 * mock's raw amber/teal hex values.
 */
export function KnowledgeCallout({ block, onViewInGraph }: { block: KnowledgeCalloutBlock; onViewInGraph?: (nodeId: number) => void }) {
  const { colorClass, Icon } = CALLOUT_STYLE[block.kind as keyof typeof CALLOUT_STYLE] ?? CALLOUT_STYLE.Note;

  return (
    <div className={`my-4 rounded-md border px-4 py-3.5 ${colorClass}`}>
      <div className="flex items-start gap-3">
        <Icon className="mt-0.5 size-4 shrink-0" />
        <div className="min-w-0 flex-1">
          <div className="mb-1.5 flex items-center gap-2">
            <span className="text-label font-semibold uppercase tracking-wide">{kindLabel(block.kind)}</span>
            {block.affects && (
              <>
                <span className="text-ink-500">·</span>
                <span className="text-mono-code opacity-70">{block.affects}</span>
              </>
            )}
          </div>
          <Prose text={block.body} className="text-body leading-relaxed text-ink-300" />
          <div className="mt-2.5 flex items-center gap-3 text-mono-code">
            <button type="button" onClick={() => onViewInGraph?.(block.node_id)} className="flex items-center gap-1 hover:underline">
              <Share2 className="size-2.5" /> View in graph
            </button>
            {block.source && (
              <span className="text-ink-500">
                {block.source[0]}:{block.source[1]}
              </span>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}

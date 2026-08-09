import { NodeBadge } from "@/components/shared/node-badge";
import { excerptFromMarkdown } from "@/lib/markdown-excerpt";
import type { GraphNode } from "@/lib/api/types";
import { cn } from "@/lib/utils";

/**
 * Compact, click-to-expand preview of a Gotcha/Decision node -- title, kind
 * badge, and a short plain-text excerpt (not the full body, which is what
 * made both the Knowledge Graph panel and the Documentation page's notes
 * sections balloon into an unbounded wall of text). Opens the full content
 * via `NodeDetailDialog` on click.
 */
export function NotePreviewCard({ node, affectsLabel, onOpen }: { node: GraphNode; affectsLabel?: string; onOpen: () => void }) {
  const borderClass = node.kind === "Gotcha" ? "border-node-gotcha/40 hover:border-node-gotcha/70" : "border-node-decision/40 hover:border-node-decision/70";
  return (
    <button
      type="button"
      onClick={onOpen}
      className={cn("w-full rounded-md border-l-4 bg-panel p-3 text-left transition-colors hover:bg-raised", borderClass)}
    >
      <div className="mb-1 flex items-center gap-2">
        <NodeBadge kind={node.kind} />
        {affectsLabel && <span className="truncate text-mono-code text-ink-500">affects {affectsLabel}</span>}
      </div>
      <p className="text-body font-medium text-ink-100">{node.name}</p>
      <p className="mt-0.5 line-clamp-2 text-body text-ink-500">{excerptFromMarkdown(node.content)}</p>
    </button>
  );
}

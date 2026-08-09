import { useState } from "react";
import { Maximize2 } from "lucide-react";
import { NodeBadge } from "@/components/shared/node-badge";
import { NotePreviewCard } from "@/components/shared/note-preview-card";
import { NodeDetailDialog } from "@/components/shared/node-detail-dialog";
import { RelationshipChip } from "@/components/shared/relationship-chip";
import { CodeBlock } from "@/components/shared/code-block";
import { Button } from "@/components/ui/button";
import type { GraphEdge, GraphNode } from "@/lib/api/types";

function isProseNode(kind: GraphNode["kind"]): boolean {
  return kind === "Gotcha" || kind === "Decision";
}

export function NodeDetailPanel({
  node,
  edges,
  nodesById,
  onSelect,
}: {
  node: GraphNode;
  edges: GraphEdge[];
  nodesById: Map<number, GraphNode>;
  onSelect: (id: number) => void;
}) {
  const [expanded, setExpanded] = useState(false);
  const [openNote, setOpenNote] = useState<GraphNode | null>(null);
  const touching = edges.filter((e) => e.src_id === node.id || e.dst_id === node.id);
  const gotchaOrDecisionNeighbors = touching
    .map((e) => {
      const otherId = e.src_id === node.id ? e.dst_id : e.src_id;
      const other = nodesById.get(otherId);
      return other && (other.kind === "Gotcha" || other.kind === "Decision") ? { edge: e, other } : null;
    })
    .filter((x): x is { edge: GraphEdge; other: GraphNode } => x !== null);

  return (
    <div className="flex h-full flex-col gap-4 overflow-y-auto p-4">
      <div className="flex items-center gap-2">
        <NodeBadge kind={node.kind} />
        <h3 className="truncate text-section font-medium text-ink-100">{node.name ?? node.path ?? `#${node.id}`}</h3>
      </div>

      {node.path && (
        <div>
          <p className="text-label uppercase tracking-wide text-ink-500">Source</p>
          <p className="mt-1 text-mono-path text-ink-300">
            {node.repo} / {node.path}
            {node.start_line != null && `:${node.start_line}-${node.end_line}`}
          </p>
        </div>
      )}

      {gotchaOrDecisionNeighbors.length > 0 && (
        <div className="flex flex-col gap-2">
          <p className="text-label uppercase tracking-wide text-ink-500">Attached knowledge</p>
          {gotchaOrDecisionNeighbors.map(({ edge, other }) => (
            <NotePreviewCard key={edge.id} node={other} onOpen={() => setOpenNote(other)} />
          ))}
        </div>
      )}

      {touching.length > 0 && (
        <div>
          <p className="mb-2 text-label uppercase tracking-wide text-ink-500">Relationships</p>
          <div className="flex flex-wrap gap-2">
            {touching.map((e) => {
              const isSource = e.src_id === node.id;
              const otherId = isSource ? e.dst_id : e.src_id;
              const other = nodesById.get(otherId);
              return (
                <RelationshipChip
                  key={e.id}
                  relation={isSource ? e.relation : `← ${e.relation}`}
                  target={other?.name ?? other?.path ?? `#${otherId}`}
                  onClick={() => onSelect(otherId)}
                />
              );
            })}
          </div>
        </div>
      )}

      {node.content && (
        <div>
          <div className="mb-2 flex items-center justify-between">
            <p className="text-label uppercase tracking-wide text-ink-500">Content</p>
            <Button variant="ghost" size="icon-sm" onClick={() => setExpanded(true)}>
              <Maximize2 className="size-3.5" />
              <span className="sr-only">Expand</span>
            </Button>
          </div>
          {isProseNode(node.kind) ? (
            <NotePreviewCard node={node} onOpen={() => setExpanded(true)} />
          ) : (
            <CodeBlock code={node.content} />
          )}
        </div>
      )}

      <NodeDetailDialog node={node} open={expanded} onOpenChange={setExpanded} />
      <NodeDetailDialog node={openNote} open={openNote != null} onOpenChange={(o) => !o && setOpenNote(null)} />
    </div>
  );
}

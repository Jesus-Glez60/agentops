import { Handle, Position } from "@xyflow/react";
import { NodeBadge } from "@/components/shared/node-badge";
import type { GraphNode } from "@/lib/api/types";
import { cn } from "@/lib/utils";

export interface GraphNodeData extends Record<string, unknown> {
  node: GraphNode;
  selected: boolean;
}

export function GraphNodeRenderer({ data }: { data: GraphNodeData }) {
  const { node, selected } = data;
  return (
    <div
      className={cn(
        "flex max-w-48 flex-col gap-1 rounded-md border bg-panel px-2.5 py-1.5 shadow-sm",
        selected ? "border-primary ring-1 ring-primary" : "border-border-strong",
      )}
    >
      <Handle type="target" position={Position.Top} className="!bg-border-strong" />
      <NodeBadge kind={node.kind} />
      <span className="truncate text-mono-code text-ink-100">{node.name ?? node.path ?? `#${node.id}`}</span>
      <Handle type="source" position={Position.Bottom} className="!bg-border-strong" />
    </div>
  );
}

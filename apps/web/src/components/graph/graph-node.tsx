import { BookOpen, FileCode, FileText, Scale, SquareFunction, TriangleAlert } from "lucide-react";
import type { NodeProps, Node as FlowNode } from "@xyflow/react";
import { Handle, Position } from "@xyflow/react";
import type { NodeKind, NodeProminence } from "@/lib/api/repos-api";
import { KIND_TAG_CLASSNAME } from "@/lib/node-kind-colors";
import { cn } from "@/lib/utils";

const KIND_ICON: Record<NodeKind, typeof SquareFunction> = {
  Symbol: SquareFunction,
  File: FileCode,
  Gotcha: TriangleAlert,
  Decision: Scale,
  Definition: SquareFunction,
  Note: BookOpen,
  DocSection: FileText,
};

export interface GraphNodeData {
  kind: NodeKind;
  label: string;
  kindLabel: string;
  prominence: NodeProminence;
  /** `depth === 0` -- rendered larger with a glow ring. */
  isSeed: boolean;
  /** Set only when the hotspot overlay is toggled on and this node came
   * back from `GET /repos/{name}/hotspots` -- its (unweighted) degree in
   * the repo's graph. `undefined` means either the overlay is off or this
   * node isn't a hotspot; both render identically (no ring/badge). */
  hotspotDegree?: number;
  [key: string]: unknown;
}

export type GraphFlowNode = FlowNode<GraphNodeData, "graphNode">;

export function GraphNode({ data }: NodeProps<GraphFlowNode>) {
  const Icon = KIND_ICON[data.kind];
  const size = data.isSeed ? "size-16" : "size-11";
  const isHotspot = data.hotspotDegree !== undefined;

  return (
    <div className="flex flex-col items-center gap-1">
      {/* Structural edges only connect through these handles -- invisible,
          just anchor points, since the visible circle below is the node. */}
      <Handle type="target" position={Position.Top} className="opacity-0" />
      <div className="relative">
        <div
          className={cn(
            "flex items-center justify-center rounded-full border-2 bg-panel transition-transform",
            size,
            KIND_TAG_CLASSNAME[data.kind],
            data.isSeed && "shadow-[0_0_0_4px_rgba(59,130,246,0.25)]",
            // A hotspot ring layers on top of (never replaces) the
            // kind-based border color above -- a hotspot is orthogonal to
            // node kind, so this is an overlay, not a recoloring.
            isHotspot && "shadow-[0_0_0_3px_var(--node-hotspot)]",
          )}
        >
          <Icon className={data.isSeed ? "size-6" : "size-4"} />
        </div>
        {isHotspot && (
          <span
            title={`Over-connected: ${data.hotspotDegree} edges`}
            className="absolute -right-1 -top-1 flex min-w-4 items-center justify-center rounded-full border border-node-hotspot/60 bg-canvas px-1 text-[9px] font-semibold leading-4 text-node-hotspot"
          >
            {data.hotspotDegree}
          </span>
        )}
      </div>
      <Handle type="source" position={Position.Bottom} className="opacity-0" />
      <p className={cn("max-w-24 truncate text-center font-mono text-ink-300", data.isSeed ? "text-section" : "text-mono-code")}>{data.label}</p>
      <p className="text-mono-code text-ink-500">{data.kindLabel.toUpperCase()}{data.isSeed ? " · selected" : ""}</p>
    </div>
  );
}

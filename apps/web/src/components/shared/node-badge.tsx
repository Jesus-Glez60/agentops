import { Braces, FileText, Lightbulb, TriangleAlert } from "lucide-react";
import type { NodeKind } from "@/lib/api/types";
import { cn } from "@/lib/utils";

const NODE_KIND_CONFIG: Record<NodeKind, { label: string; icon: typeof Braces; className: string }> = {
  Symbol: { label: "SYMBOL", icon: Braces, className: "border-node-symbol/40 text-node-symbol" },
  File: { label: "FILE", icon: FileText, className: "border-node-file/40 text-node-file" },
  Gotcha: { label: "GOTCHA", icon: TriangleAlert, className: "border-node-gotcha/40 bg-node-gotcha/10 text-node-gotcha" },
  Decision: { label: "DECISION", icon: Lightbulb, className: "border-node-decision/40 text-node-decision" },
};

/**
 * Consolidates three previously-separate, non-identical implementations
 * (graph/page.tsx's 2-way gotcha/decision color ternary being the closest
 * precedent) into one component covering all four real NodeKind values.
 */
export function NodeBadge({ kind, className }: { kind: NodeKind; className?: string }) {
  const config = NODE_KIND_CONFIG[kind];
  const Icon = config.icon;
  return (
    <span
      className={cn(
        "inline-flex items-center gap-1 rounded-md border px-1.5 py-0.5 text-mono-code font-medium uppercase tracking-wide",
        config.className,
        className,
      )}
    >
      <Icon className="size-3" />
      {config.label}
    </span>
  );
}

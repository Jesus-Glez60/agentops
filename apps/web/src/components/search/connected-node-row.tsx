import { ArrowRight, ExternalLink } from "lucide-react";
import type { NodeKind } from "@/lib/api/repos-api";
import { NODE_KIND_SHAPE } from "@/lib/node-kind-shapes";
import { NavDot } from "@/components/shell/nav-dot";

export function ConnectedNodeRow({
  relation,
  kind,
  kindLabel,
  label,
  onClick,
}: {
  relation: string;
  kind: NodeKind;
  kindLabel: string;
  label: string;
  onClick: () => void;
}) {
  const { color, shape } = NODE_KIND_SHAPE[kind];
  return (
    <button
      onClick={onClick}
      className="flex w-full items-center gap-2 rounded-md border border-border-strong bg-raised px-3 py-2 text-left transition-colors hover:border-primary/50"
    >
      <span className="w-16 shrink-0 text-mono-code text-ink-500">{relation}</span>
      <ArrowRight className="size-3 shrink-0 text-ink-500" />
      <span className="flex shrink-0 items-center gap-1.5 font-mono text-[11px] uppercase tracking-wide" style={{ color }}>
        <NavDot color={color} shape={shape} />
        {kindLabel}
      </span>
      <span className="min-w-0 flex-1 truncate text-body text-ink-100">{label}</span>
      <ExternalLink className="size-3.5 shrink-0 text-ink-500" />
    </button>
  );
}

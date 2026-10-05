import type { NodeKind } from "@/lib/api/repos-api";
import { RelevanceBadge, relevanceForScore } from "@/components/shared/relevance-badge";
import { nodeKindShape } from "@/lib/node-kind-shapes";
import { NavDot } from "@/components/shell/nav-dot";
import { cn } from "@/lib/utils";

export function SearchResultCard({
  kind,
  kindLabel,
  title,
  snippet,
  score,
  location,
  selected,
  onClick,
}: {
  kind: NodeKind;
  kindLabel: string;
  title: string;
  snippet: string;
  score: number;
  /** `L{start}-{end}` when the node has source lines, else the repo name -- matches the prototype's own `r.loc` fallback. */
  location?: string;
  selected: boolean;
  onClick: () => void;
}) {
  const { color, shape } = nodeKindShape(kind);
  return (
    <button
      onClick={onClick}
      className={cn(
        "flex w-full flex-col gap-2 rounded-[14px] border p-3.5 text-left transition-colors",
        selected ? "border-primary bg-raised" : "border-border-strong bg-panel hover:border-ink-500",
      )}
    >
      <div className="flex items-center gap-2">
        <span className="flex items-center gap-1.5 font-mono text-[11px] uppercase tracking-wide" style={{ color }}>
          <NavDot color={color} shape={shape} />
          {kindLabel}
        </span>
        <span className="flex-1" />
        <RelevanceBadge level={relevanceForScore(score)} />
      </div>
      <p className="truncate text-[15px] font-semibold text-ink-100">{title}</p>
      <p className="line-clamp-2 text-[13.5px] leading-relaxed text-ink-300">{snippet}</p>
      {location && <p className="font-mono text-[10.5px] text-ink-600">{location}</p>}
    </button>
  );
}

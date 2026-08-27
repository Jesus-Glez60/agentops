import type { NodeKind } from "@/lib/api/repos-api";
import { RelevanceBadge, relevanceForScore } from "@/components/shared/relevance-badge";
import { KIND_TAG_CLASSNAME } from "@/lib/node-kind-colors";
import { cn } from "@/lib/utils";

export function SearchResultCard({
  kind,
  kindLabel,
  title,
  snippet,
  score,
  selected,
  onClick,
}: {
  kind: NodeKind;
  kindLabel: string;
  title: string;
  snippet: string;
  score: number;
  selected: boolean;
  onClick: () => void;
}) {
  return (
    <button
      onClick={onClick}
      className={cn(
        "flex w-full flex-col gap-1 rounded-md border p-3 text-left transition-colors",
        selected ? "border-primary bg-raised" : "border-border-strong bg-panel hover:border-border-strong/80",
      )}
    >
      <div className="flex items-center justify-between gap-2">
        <span className={cn("rounded border px-1.5 py-0.5 text-mono-code uppercase", KIND_TAG_CLASSNAME[kind])}>{kindLabel}</span>
        <RelevanceBadge level={relevanceForScore(score)} />
      </div>
      <p className="truncate text-section font-medium text-ink-100">{title}</p>
      <p className="line-clamp-2 text-body text-ink-500">{snippet}</p>
    </button>
  );
}

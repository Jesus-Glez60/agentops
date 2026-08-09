import { RelevanceBadge, relevanceForScore } from "@/components/shared/relevance-badge";
import { cn } from "@/lib/utils";

export function SearchResultCard({
  kindLabel,
  title,
  snippet,
  score,
  selected,
  onClick,
}: {
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
        <span className="rounded border border-border-strong px-1.5 py-0.5 text-mono-code uppercase text-ink-500">{kindLabel}</span>
        <RelevanceBadge level={relevanceForScore(score)} />
      </div>
      <p className="truncate text-section font-medium text-ink-100">{title}</p>
      <p className="line-clamp-2 text-body text-ink-500">{snippet}</p>
    </button>
  );
}

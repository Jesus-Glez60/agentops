import { cn } from "@/lib/utils";

export type RelevanceLevel = "strong" | "related" | "supporting";

const RELEVANCE_CONFIG: Record<RelevanceLevel, { label: string; className: string }> = {
  strong: { label: "Strong match", className: "border-relevance-strong/40 text-relevance-strong" },
  related: { label: "Related", className: "border-relevance-related/40 bg-relevance-related/10 text-relevance-related" },
  supporting: { label: "Supporting context", className: "border-border-strong text-ink-500" },
};

export function relevanceForScore(score: number): RelevanceLevel {
  if (score >= 0.7) return "strong";
  if (score >= 0.4) return "related";
  return "supporting";
}

export function RelevanceBadge({ level, className }: { level: RelevanceLevel; className?: string }) {
  const config = RELEVANCE_CONFIG[level];
  return (
    <span className={cn("inline-flex items-center rounded-md border px-1.5 py-0.5 text-mono-code font-medium", config.className, className)}>
      {config.label}
    </span>
  );
}

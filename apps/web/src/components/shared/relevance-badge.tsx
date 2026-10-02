import { cn } from "@/lib/utils";

export type RelevanceLevel = "strong" | "related" | "supporting";

// Plain colored text, not a bordered badge -- matches the prototype's own
// search-result relevance label exactly (`font-size:12px;font-weight:600;
// color:{{r.relC}}`, no border/background).
const RELEVANCE_CONFIG: Record<RelevanceLevel, { label: string; className: string }> = {
  strong: { label: "Strong match", className: "text-relevance-strong" },
  related: { label: "Related", className: "text-relevance-related" },
  supporting: { label: "Supporting context", className: "text-ink-500" },
};

export function relevanceForScore(score: number): RelevanceLevel {
  if (score >= 0.7) return "strong";
  if (score >= 0.4) return "related";
  return "supporting";
}

export function RelevanceBadge({ level, className }: { level: RelevanceLevel; className?: string }) {
  const config = RELEVANCE_CONFIG[level];
  return <span className={cn("text-[12px] font-semibold", config.className, className)}>{config.label}</span>;
}

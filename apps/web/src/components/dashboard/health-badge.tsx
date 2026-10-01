import type { HealthStatus } from "@/lib/repo-health";
import { cn } from "@/lib/utils";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";

// Only the 4 states repoHealth() actually produces -- not main's full
// 6-state union, since scanning/failed were always connection-registry
// states (out of scope this pass; extend when/if that gets joined in).
// Colored dot, not a lucide icon -- matches the prototype's own repo-row
// health marker (`<span style="width:7px;height:7px;border-radius:50%;
// background:{{r.hc}}">`).
const HEALTH_CONFIG: Record<HealthStatus, { label: string; dotClassName: string; textClassName: string }> = {
  healthy: { label: "Healthy", dotClassName: "bg-health-healthy", textClassName: "text-health-healthy" },
  warning: { label: "Warning", dotClassName: "bg-health-warning", textClassName: "text-health-warning" },
  stale: { label: "Stale", dotClassName: "bg-health-stale", textClassName: "text-health-stale" },
  "not-indexed": { label: "Not yet scanned", dotClassName: "bg-ink-500", textClassName: "text-ink-500" },
};

export function HealthBadge({ status, reason, className }: { status: HealthStatus; reason?: string | null; className?: string }) {
  const config = HEALTH_CONFIG[status];
  const badge = (
    <span className={cn("inline-flex items-center gap-1.5 text-[13.5px] font-semibold", config.textClassName, className)}>
      <span className={cn("size-[7px] shrink-0 rounded-full", config.dotClassName)} />
      {config.label}
    </span>
  );
  if (!reason) return badge;
  return (
    <Tooltip>
      <TooltipTrigger asChild>{badge}</TooltipTrigger>
      <TooltipContent>{reason}</TooltipContent>
    </Tooltip>
  );
}

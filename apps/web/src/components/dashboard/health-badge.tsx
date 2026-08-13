import { CircleCheck, Clock, TriangleAlert } from "lucide-react";
import type { HealthStatus } from "@/lib/repo-health";
import { cn } from "@/lib/utils";

// Only the 4 states repoHealth() actually produces -- not main's full
// 6-state union, since scanning/failed were always connection-registry
// states (out of scope this pass; extend when/if that gets joined in).
const HEALTH_CONFIG: Record<HealthStatus, { label: string; icon: typeof CircleCheck; className: string }> = {
  healthy: { label: "Healthy", icon: CircleCheck, className: "text-health-healthy" },
  warning: { label: "Warning", icon: TriangleAlert, className: "text-health-warning" },
  stale: { label: "Stale", icon: Clock, className: "text-health-stale" },
  "not-indexed": { label: "Not yet scanned", icon: Clock, className: "text-ink-500" },
};

export function HealthBadge({ status, className }: { status: HealthStatus; className?: string }) {
  const config = HEALTH_CONFIG[status];
  const Icon = config.icon;
  return (
    <span className={cn("inline-flex items-center gap-1.5 text-section font-medium", config.className, className)}>
      <Icon className="size-3.5" />
      {config.label}
    </span>
  );
}

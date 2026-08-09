import { CircleCheck, CircleX, Clock, Loader2, TriangleAlert } from "lucide-react";
import { cn } from "@/lib/utils";

// Not a server-provided field anywhere -- no backend endpoint returns a
// "health" status for a repo or connection. This is a client-computed
// classification (see lib/repo-health.ts for repos, and the direct mapping
// from ConnectionStatus for connections) rendered through one shared
// component, consolidating what used to be three separate, non-identical
// ad hoc badges (repos/page.tsx's unconditional green dot had no real
// logic at all; repos/connect/page.tsx had a real 3-way status ternary).
export type HealthStatus = "healthy" | "scanning" | "warning" | "stale" | "failed" | "not-indexed";

const HEALTH_CONFIG: Record<HealthStatus, { label: string; icon: typeof CircleCheck; className: string; spin?: boolean }> = {
  healthy: { label: "Healthy", icon: CircleCheck, className: "text-health-healthy" },
  scanning: { label: "Scanning", icon: Loader2, className: "text-health-scanning", spin: true },
  warning: { label: "Warning", icon: TriangleAlert, className: "text-health-warning" },
  stale: { label: "Stale", icon: Clock, className: "text-health-stale" },
  failed: { label: "Failed", icon: CircleX, className: "text-health-failed" },
  "not-indexed": { label: "Not indexed", icon: Clock, className: "text-ink-500" },
};

export function HealthBadge({ status, className }: { status: HealthStatus; className?: string }) {
  const config = HEALTH_CONFIG[status];
  const Icon = config.icon;
  return (
    <span className={cn("inline-flex items-center gap-1.5 text-section font-medium", config.className, className)}>
      <Icon className={cn("size-3.5", config.spin && "animate-spin")} />
      {config.label}
    </span>
  );
}

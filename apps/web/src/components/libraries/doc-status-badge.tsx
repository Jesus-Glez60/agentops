import { CircleCheck, TriangleAlert } from "lucide-react";
import { cn } from "@/lib/utils";

/** Healthy/mismatch doesn't fit `HealthStatus` (that union is repo-scan-recency semantics) -- a library's docs are either "no known version mismatch" or "at least one repo declares a version other than the latest indexed one." */
export function DocStatusBadge({ hasMismatch, className }: { hasMismatch: boolean; className?: string }) {
  return hasMismatch ? (
    <span className={cn("inline-flex items-center gap-1.5 text-section font-medium text-health-warning", className)}>
      <TriangleAlert className="size-3.5" />
      Version mismatch
    </span>
  ) : (
    <span className={cn("inline-flex items-center gap-1.5 text-section font-medium text-health-healthy", className)}>
      <CircleCheck className="size-3.5" />
      Healthy
    </span>
  );
}

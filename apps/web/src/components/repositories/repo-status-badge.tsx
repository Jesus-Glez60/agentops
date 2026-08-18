import { CircleCheck, Clock, TriangleAlert } from "lucide-react";
import type { ParsedRepoStatus } from "@/lib/api/repos-api";
import { cn } from "@/lib/utils";

export function RepoStatusBadge({ status, className }: { status: ParsedRepoStatus; className?: string }) {
  if (status.kind === "active") {
    return (
      <span className={cn("inline-flex items-center gap-1.5 text-section font-medium text-health-healthy", className)}>
        <CircleCheck className="size-3.5" />
        Active
      </span>
    );
  }
  if (status.kind === "pending") {
    return (
      <span className={cn("inline-flex items-center gap-1.5 text-section font-medium text-health-scanning", className)}>
        <Clock className="size-3.5" />
        Pending verification
      </span>
    );
  }
  return (
    <span className={cn("inline-flex items-center gap-1.5 text-section font-medium text-health-failed", className)} title={status.reason}>
      <TriangleAlert className="size-3.5" />
      Failed
    </span>
  );
}

import { cn } from "@/lib/utils";

/** Colored dot + pill background, not an icon -- matches the prototype's
 * own status pill (`<span>` with a 7px dot, `border-radius:999px`,
 * translucent background), not a lucide icon. Healthy/mismatch doesn't fit
 * `HealthStatus` (that union is repo-scan-recency semantics) -- a
 * library's docs are either "no known version mismatch" or "at least one
 * repo declares a version other than the latest indexed one." */
export function DocStatusBadge({ hasMismatch, className }: { hasMismatch: boolean; className?: string }) {
  const color = hasMismatch ? "var(--health-warning)" : "var(--health-healthy)";
  return (
    <span className={cn("inline-flex items-center gap-1.5 rounded-full bg-ink-100/[0.06] px-2.5 py-1 text-[13px] font-semibold", className)} style={{ color }}>
      <span className="size-[7px] shrink-0 rounded-full" style={{ background: color }} />
      {hasMismatch ? "Version mismatch" : "Healthy"}
    </span>
  );
}

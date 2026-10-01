import type { Library } from "@/lib/api/libraries-api";
import { cn } from "@/lib/utils";

/** Bordered-box stat grid -- matches the design's own `libStats` pattern,
 * which is the same grid Overview/Gotchas already use, not a divided
 * horizontal strip (redesign plan Phase 6: this screen's stats were the
 * one place that diverged from the pattern already correct everywhere
 * else). No public/private tile: that field doesn't exist in this
 * single-tenant build. */
export function LibraryStatsStrip({ libraries }: { libraries: Library[] }) {
  const totalVersions = libraries.reduce((sum, lib) => sum + lib.versions.length, 0);
  const mismatchCount = libraries.filter((lib) => lib.has_mismatch).length;

  return (
    <div className="grid grid-cols-3 gap-4 border-b border-border-strong bg-canvas px-6 py-4">
      <StatBox label="Libraries indexed" value={libraries.length} />
      <StatBox label="Total versions" value={totalVersions} />
      <StatBox label="Version mismatches" value={mismatchCount} valueClassName={mismatchCount > 0 ? "text-health-warning" : undefined} />
    </div>
  );
}

function StatBox({ label, value, valueClassName }: { label: string; value: number; valueClassName?: string }) {
  return (
    <div className="rounded-lg border border-border-strong bg-panel px-4 py-3">
      <p className="text-label uppercase tracking-wide text-ink-500">{label}</p>
      <p className={cn("text-stat-value font-bold text-ink-100", valueClassName)}>{value}</p>
    </div>
  );
}

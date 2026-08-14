import type { Library } from "@/lib/api/libraries-api";

/** Thin horizontal summary row, distinct from the dashboard's `StatCard` grid -- matches the mockup's compact strip rather than forcing this into big cards. No public/private tile: that field doesn't exist in this single-tenant build (see the plan's prior-art audit). */
export function LibraryStatsStrip({ libraries }: { libraries: Library[] }) {
  const totalVersions = libraries.reduce((sum, lib) => sum + lib.versions.length, 0);
  const mismatchCount = libraries.filter((lib) => lib.has_mismatch).length;

  return (
    <div className="flex items-center gap-6 border-b border-border-strong bg-panel px-6 py-3 text-section">
      <Stat label="Libraries indexed" value={libraries.length} />
      <Divider />
      <Stat label="Total versions" value={totalVersions} />
      <Divider />
      <Stat label="Version mismatches" value={mismatchCount} valueClassName={mismatchCount > 0 ? "text-health-warning" : undefined} />
    </div>
  );
}

function Stat({ label, value, valueClassName }: { label: string; value: number; valueClassName?: string }) {
  return (
    <div className="flex items-center gap-2">
      <span className="text-ink-500">{label}</span>
      <span className={valueClassName ?? "font-semibold text-ink-100"}>{value}</span>
    </div>
  );
}

function Divider() {
  return <div className="h-3 w-px bg-border-strong" />;
}

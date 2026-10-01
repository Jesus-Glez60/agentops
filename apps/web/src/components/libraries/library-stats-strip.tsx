import type { Library } from "@/lib/api/libraries-api";
import { StatStrip } from "@/components/shared/stat-strip";

/** Thin bordered strip with internal dividers -- the prototype's own
 * `libStats` pattern (confirmed by re-reading its markup directly), the
 * same one Overview's stat row uses via the shared `StatStrip`. A prior
 * pass mistakenly rebuilt this as a grid of separate bordered cards,
 * believing that was the shared pattern -- it wasn't; both Overview's and
 * Libraries' own prototype markup are single divided containers. No
 * public/private tile: that field doesn't exist in this single-tenant
 * build (see the plan's prior-art audit). */
export function LibraryStatsStrip({ libraries }: { libraries: Library[] }) {
  const totalVersions = libraries.reduce((sum, lib) => sum + lib.versions.length, 0);
  const mismatchCount = libraries.filter((lib) => lib.has_mismatch).length;

  return (
    <StatStrip
      size="md"
      items={[
        { label: "Libraries indexed", value: libraries.length },
        { label: "Total versions", value: totalVersions },
        { label: "Version mismatches", value: mismatchCount, valueClassName: mismatchCount > 0 ? "text-health-warning" : undefined },
      ]}
    />
  );
}

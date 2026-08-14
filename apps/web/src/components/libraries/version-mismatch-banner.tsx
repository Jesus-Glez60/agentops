import { TriangleAlert } from "lucide-react";
import type { RepoLibraryUsage } from "@/lib/api/libraries-api";

/** Shown when at least one repo's declared version differs from the version currently being viewed -- not necessarily the same set as `library.has_mismatch` (that's always against the latest indexed version; this compares against whatever version the detail page's selector is showing). */
export function VersionMismatchBanner({ viewingVersion, mismatched }: { viewingVersion: string; mismatched: RepoLibraryUsage[] }) {
  if (mismatched.length === 0) return null;

  return (
    <div className="flex items-start gap-3 border-b border-health-warning/20 bg-health-warning/5 px-6 py-2.5 text-section">
      <TriangleAlert className="mt-0.5 size-4 shrink-0 text-health-warning" />
      <div>
        <span className="font-medium text-health-warning">Version mismatch: </span>
        <span className="text-health-warning/80">
          You are viewing docs for <span className="text-mono-code">{viewingVersion}</span>, but{" "}
          {mismatched.map((u, i) => (
            <span key={u.repo_identifier}>
              {i > 0 && ", "}
              <span className="text-mono-code text-ink-200">{u.repo_identifier}</span> has <span className="text-mono-code">{u.declared_version}</span> declared
            </span>
          ))}
          .
        </span>
      </div>
    </div>
  );
}

import { FolderSearch } from "lucide-react";
import type { RepoLibraryUsage } from "@/lib/api/libraries-api";
import { EmptyState } from "@/components/shared/empty-state";
import { cn } from "@/lib/utils";

export function ReposUsingThisList({ usedIn }: { usedIn: RepoLibraryUsage[] }) {
  if (usedIn.length === 0) {
    // `min-h-0` override -- this renders inside a detail-page tab panel,
    // not a full-viewport section, so EmptyState's default `min-h-[60vh]`
    // would blow out the layout here.
    return <EmptyState icon={FolderSearch} title="No repos yet" description="No repos have declared this dependency yet — run `agentops sync-docs` in a repo that uses it." className="min-h-0 py-6" />;
  }

  return (
    <div className="flex flex-col gap-1">
      {usedIn.map((u) => (
        <div key={u.repo_identifier} className="flex items-center justify-between rounded border border-border-strong px-2 py-1.5 text-section">
          <span className="truncate text-mono-path text-ink-300">{u.repo_identifier}</span>
          <span className={cn("text-mono-code", u.mismatch ? "text-health-warning" : "text-ink-400")}>{u.declared_version}</span>
        </div>
      ))}
    </div>
  );
}

"use client";

import useSWR from "swr";
import { GitBranch } from "lucide-react";
import { getRepos, REPOS_SWR_KEY } from "@/lib/api/repos-api";

/** A grid of scanned repos to pick as the graph's data source. Used both
 * as the graph page's initial empty state and inside the header's
 * repo-switcher popover. */
export function RepoPicker({ onSelect }: { onSelect: (repo: string) => void }) {
  const { data, isLoading } = useSWR(REPOS_SWR_KEY, getRepos);
  const repos = data?.connections;

  if (isLoading) return <p className="text-body text-ink-500">Loading repositories…</p>;
  if (!repos || repos.length === 0) return <p className="text-body text-ink-500">No repositories scanned yet.</p>;

  return (
    <div className="flex flex-col gap-1.5">
      {repos.map((repo) => (
        <button
          key={repo.id}
          type="button"
          onClick={() => onSelect(repo.id)}
          className="flex items-center gap-2 rounded-md border border-border-strong bg-panel px-3 py-2 text-left text-body text-ink-300 transition-colors hover:border-primary/50 hover:text-ink-100"
        >
          <GitBranch className="size-3.5 shrink-0 text-ink-500" />
          <span className="min-w-0 flex-1 truncate">{repo.id}</span>
          {repo.counts && <span className="shrink-0 text-mono-code text-ink-500">{repo.counts.symbols + repo.counts.files + repo.counts.gotchas + repo.counts.decisions} nodes</span>}
        </button>
      ))}
    </div>
  );
}

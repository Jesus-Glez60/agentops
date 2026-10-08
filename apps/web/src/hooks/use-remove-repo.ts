"use client";

import { useState } from "react";
import { useSWRConfig } from "swr";
import { toast } from "sonner";
import { deleteRepo, REPOS_SWR_KEY, type RepoConnection } from "@/lib/api/repos-api";

// Shared by both repo-table.tsx (dashboard) and repositories-table.tsx
// (Repositories screen) -- was duplicated character-for-character across
// both before this extraction. Backstop for a `discovered`/local-only stub
// the connect flow's auto-suggested-merge toast didn't catch (e.g. it was
// registered under a name that doesn't match any repo being connected, or
// the suggestion was dismissed). Only offered for `discovered` rows (never
// an active Ssh/GitHubApp connection) -- consistent with "Finish
// connecting" only showing there too.
export function useRemoveRepo() {
  const { mutate } = useSWRConfig();
  const [removingIds, setRemovingIds] = useState<Set<string>>(new Set());

  async function removeRepo(repo: RepoConnection) {
    if (!window.confirm(`Remove "${repo.repo_url}"? This only removes the connection -- it doesn't wipe any already-recorded graph/notes/doc data.`)) return;
    setRemovingIds((prev) => new Set(prev).add(repo.id));
    try {
      await deleteRepo(repo.id);
      await mutate(REPOS_SWR_KEY);
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Couldn't remove this connection. Please try again.");
    } finally {
      setRemovingIds((prev) => {
        const next = new Set(prev);
        next.delete(repo.id);
        return next;
      });
    }
  }

  return { removeRepo, removingIds };
}

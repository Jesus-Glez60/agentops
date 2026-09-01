"use client";

import { useState } from "react";
import { toast } from "sonner";
import { listBranches, setBranch, type RepoConnection } from "@/lib/api/repos-api";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";

/** Sentinel for "no override" -- Radix disallows an empty-string item value, and `null` isn't a valid DOM value either. */
const AUTO_BRANCH_VALUE = "__auto__";

/**
 * Branch picker + reindex-on-change, shared by the dashboard's repo table
 * row and the repo detail page -- factored out of `repo-table.tsx` so the
 * lazy-branch-fetch/loading/toast logic exists once, not as two copies
 * that can drift. Manages its own pending/loading state locally (disables
 * itself while a branch switch is in flight) rather than threading a
 * shared `reindexingIds` set in from the caller -- callers should still
 * pass `onChanged` to revalidate whatever list/detail data they own once a
 * switch succeeds.
 */
export function BranchSelect({ repo, onChanged, className }: { repo: RepoConnection; onChanged: () => void; className?: string }) {
  const [pending, setPending] = useState(false);
  // Lazily-fetched, cached for the lifetime of this component instance --
  // the branch list rarely changes mid-session, and re-fetching on every
  // dropdown open would mean an extra round trip (SSH: `git ls-remote`;
  // GitHub App: a GitHub API call) each time a user just wants to glance
  // at the current selection.
  const [branchOptions, setBranchOptions] = useState<string[] | null>(null);

  const currentBranch = repo.tracked_branch ?? repo.branch ?? undefined;
  // Always include the current branch as a renderable option, even before
  // `listBranches` has loaded -- otherwise Radix's `SelectValue` has
  // nothing to match `value` against and silently falls back to the
  // placeholder, which would look like the branch column just went blank
  // on every page load.
  const branchList = branchOptions ?? (currentBranch ? [currentBranch] : []);

  async function handleOpenChange(open: boolean) {
    if (!open || branchOptions) return;
    try {
      const branches = await listBranches(repo.id);
      setBranchOptions(branches);
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Failed to load branches.");
    }
  }

  async function handleChange(value: string) {
    const branch = value === AUTO_BRANCH_VALUE ? null : value;
    setPending(true);
    try {
      await setBranch(repo.id, branch);
      toast.success(branch ? `Switched to ${branch} — reindexing.` : "Reset to default branch — reindexing.");
      onChanged();
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Failed to switch branch. Please try again.");
    } finally {
      setPending(false);
    }
  }

  return (
    <Select value={currentBranch} onValueChange={handleChange} onOpenChange={handleOpenChange} disabled={pending}>
      <SelectTrigger size="sm" className={className ?? "w-40"}>
        <SelectValue placeholder="—" />
      </SelectTrigger>
      <SelectContent>
        <SelectItem value={AUTO_BRANCH_VALUE}>Auto (default)</SelectItem>
        {branchList.map((b) => (
          <SelectItem key={b} value={b}>
            {b}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}

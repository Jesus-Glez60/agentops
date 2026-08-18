"use client";

import { useMemo, useState } from "react";
import useSWR from "swr";
import { toast } from "sonner";
import { TEAM_REPO_ACCESS_SWR_KEY, getRepoAccess, saveRepoAccess } from "@/lib/api/team-api";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";

function cellKey(userId: number, repoId: string) {
  return `${userId}:${repoId}`;
}

export function RepoAccessTab() {
  const { data, mutate, isLoading } = useSWR(TEAM_REPO_ACCESS_SWR_KEY, getRepoAccess);
  const [pending, setPending] = useState<Map<string, boolean>>(new Map());
  const [saving, setSaving] = useState(false);

  const serverAllowed = useMemo(() => {
    const map = new Map<string, boolean>();
    for (const cell of data?.access ?? []) map.set(cellKey(cell.user_id, cell.repo_id), cell.allowed);
    return map;
  }, [data]);

  function isChecked(userId: number, repoId: string) {
    const key = cellKey(userId, repoId);
    return pending.has(key) ? pending.get(key)! : (serverAllowed.get(key) ?? false);
  }

  function toggle(userId: number, repoId: string) {
    const key = cellKey(userId, repoId);
    const next = new Map(pending);
    next.set(key, !isChecked(userId, repoId));
    setPending(next);
  }

  async function handleSave() {
    if (pending.size === 0) return;
    setSaving(true);
    try {
      const changes = [...pending.entries()].map(([key, allowed]) => {
        const [userId, repoId] = key.split(":");
        return { user_id: Number(userId), repo_id: repoId, allowed };
      });
      await saveRepoAccess(changes);
      setPending(new Map());
      await mutate();
      toast.success("Repository access updated");
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Couldn't save changes. Please try again.");
    } finally {
      setSaving(false);
    }
  }

  if (isLoading) return <p className="text-body text-ink-500">Loading…</p>;
  if (!data || data.repos.length === 0) return <p className="text-body text-ink-500">No repositories connected yet.</p>;

  const editableByUser = new Map(data.members.map((m) => [m.user_id, data.access.some((c) => c.user_id === m.user_id && c.editable)]));

  return (
    <div>
      <p className="mb-3 text-body text-ink-400">
        Control which members can access each repository. By default, all active members can access all repos. Override per-user below.
      </p>
      <div className="overflow-x-auto rounded-lg border border-border-strong">
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Member</TableHead>
              {data.repos.map((repo) => (
                <TableHead key={repo.id} className="text-center">
                  {repo.repo_url.replace(/^git@github\.com:|^https?:\/\/github\.com\//, "").replace(/\.git$/, "")}
                </TableHead>
              ))}
            </TableRow>
          </TableHeader>
          <TableBody>
            {data.members.map((member) => (
              <TableRow key={member.user_id}>
                <TableCell className="text-body text-ink-100">
                  {member.first_name} {member.last_name}
                  <span className="ml-1.5 text-mono-code text-ink-500">({member.role})</span>
                </TableCell>
                {data.repos.map((repo) => (
                  <TableCell key={repo.id} className="text-center">
                    <Checkbox
                      checked={isChecked(member.user_id, repo.id)}
                      disabled={!editableByUser.get(member.user_id)}
                      onCheckedChange={() => toggle(member.user_id, repo.id)}
                    />
                  </TableCell>
                ))}
              </TableRow>
            ))}
          </TableBody>
        </Table>
        <div className="flex items-center justify-between border-t border-border-strong px-4 py-3">
          <span className="text-section text-ink-500">Disabled checkboxes are controlled by role.</span>
          <Button size="sm" disabled={pending.size === 0 || saving} onClick={handleSave}>
            {saving ? "Saving…" : "Save changes"}
          </Button>
        </div>
      </div>
    </div>
  );
}

"use client";

import { useState } from "react";
import Link from "next/link";
import useSWR, { useSWRConfig } from "swr";
import { toast } from "sonner";
import { GitBranch, RefreshCw } from "lucide-react";
import { getRepos, verifyRepo, parseRepoStatus, REPOS_SWR_KEY, type RepoConnection } from "@/lib/api/repos-api";
import { relativeTimeFromIsoString } from "@/lib/relative-time";
import { RepoStatusBadge } from "@/components/repositories/repo-status-badge";
import { ViewDeployKeyDialog } from "@/components/repositories/view-deploy-key-dialog";
import { EmptyState } from "@/components/shared/empty-state";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";

export function RepositoriesTable() {
  const { data, isLoading } = useSWR(REPOS_SWR_KEY, getRepos);
  const { mutate } = useSWRConfig();
  const [verifyingIds, setVerifyingIds] = useState<Set<string>>(new Set());
  const [keyDialogRepo, setKeyDialogRepo] = useState<RepoConnection | null>(null);

  async function handleVerify(repo: RepoConnection) {
    setVerifyingIds((prev) => new Set(prev).add(repo.id));
    try {
      await verifyRepo(repo.id);
      await mutate(REPOS_SWR_KEY);
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Verification failed. Please try again.");
    } finally {
      setVerifyingIds((prev) => {
        const next = new Set(prev);
        next.delete(repo.id);
        return next;
      });
    }
  }

  const connections = data?.connections ?? [];

  if (!isLoading && connections.length === 0) {
    return <EmptyState icon={GitBranch} title="No repositories connected" description="Connect a repository to start pulling its code, docs, and knowledge graph into AgentOps." />;
  }

  return (
    <>
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Repository</TableHead>
            <TableHead>Status</TableHead>
            <TableHead>Connected</TableHead>
            <TableHead className="text-right">Actions</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {isLoading && (
            <TableRow>
              <TableCell colSpan={4} className="text-center text-ink-500">
                Loading…
              </TableCell>
            </TableRow>
          )}
          {connections.map((repo) => {
            const status = parseRepoStatus(repo.status);
            const verifying = verifyingIds.has(repo.id);
            return (
              <TableRow key={repo.id}>
                <TableCell>
                  <div className="flex items-center gap-3">
                    <div className="flex size-8 shrink-0 items-center justify-center rounded border border-border-strong bg-panel text-ink-300">
                      <GitBranch className="size-4" />
                    </div>
                    <div className="min-w-0">
                      <div className="truncate font-medium text-ink-100">{repo.repo_url}</div>
                      <div className="truncate text-mono-path text-ink-500">{repo.id}</div>
                    </div>
                  </div>
                </TableCell>
                <TableCell>
                  <RepoStatusBadge status={status} />
                </TableCell>
                <TableCell className="text-mono-code text-ink-400">{relativeTimeFromIsoString(repo.created_at)}</TableCell>
                <TableCell>
                  <div className="flex justify-end gap-1">
                    {repo.method === "discovered" ? (
                      // A discovered connection has no keypair/App install yet
                      // -- "Verify" assumes real auth material already exists,
                      // so it needs its own CTA into the same wizard a human
                      // would otherwise start from scratch, pre-filled with
                      // the URL an agent already found.
                      <Button variant="outline" size="sm" asChild>
                        <Link href={`/repositories/connect/ssh?repo_url=${encodeURIComponent(repo.repo_url)}`}>Finish connecting</Link>
                      </Button>
                    ) : (
                      <>
                        {repo.public_key_openssh && (
                          <Button variant="outline" size="sm" onClick={() => setKeyDialogRepo(repo)}>
                            View key
                          </Button>
                        )}
                        <Button variant="outline" size="sm" onClick={() => handleVerify(repo)} disabled={verifying}>
                          <RefreshCw className={cn("size-3.5", verifying && "animate-spin")} />
                          Verify
                        </Button>
                      </>
                    )}
                  </div>
                </TableCell>
              </TableRow>
            );
          })}
        </TableBody>
      </Table>
      {keyDialogRepo && (
        <ViewDeployKeyDialog
          repo={keyDialogRepo}
          onOpenChange={(open) => {
            if (!open) setKeyDialogRepo(null);
          }}
        />
      )}
    </>
  );
}

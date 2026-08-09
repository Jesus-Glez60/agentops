"use client";

import { useState } from "react";
import Link from "next/link";
import useSWR from "swr";
import { connectRepo, getConnections, verifyConnection } from "@/lib/api/heavy-api";
import { useTenant } from "@/lib/tenant-context";
import { connectionHealth } from "@/lib/repo-health";
import { isFailedStatus, failureReason } from "@/lib/api/types";
import { HealthBadge } from "@/components/shared/health-badge";
import { CopyButton } from "@/components/shared/copy-button";
import { ErrorState } from "@/components/shared/error-state";
import { EmptyState } from "@/components/shared/empty-state";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { KeyRound } from "lucide-react";
import { toast } from "sonner";

export default function SshConnectPage() {
  const { tenant, hasTenant } = useTenant();
  const [repoId, setRepoId] = useState("");
  const [repoUrl, setRepoUrl] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [formError, setFormError] = useState<string | null>(null);
  const [verifyingId, setVerifyingId] = useState<string | null>(null);

  const { data: connections, error, isLoading, mutate } = useSWR(hasTenant ? ["connections", tenant] : null, () => getConnections(tenant!));

  if (!hasTenant) {
    return (
      <EmptyState
        icon={KeyRound}
        title="Select an organization first"
        description="Pick or set an organization id in the sidebar before connecting a repository -- every connection is scoped to one."
      />
    );
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setFormError(null);
    setSubmitting(true);
    try {
      await connectRepo(tenant!, repoId.trim(), repoUrl.trim());
      setRepoId("");
      setRepoUrl("");
      await mutate();
    } catch (e) {
      setFormError(e instanceof Error ? e.message : String(e));
    } finally {
      setSubmitting(false);
    }
  }

  async function handleVerify(id: string) {
    setVerifyingId(id);
    try {
      const result = await verifyConnection(id, tenant!);
      if (result.status === "active") toast.success("Deploy key verified -- connection is active.");
      else toast.error(`Verification failed: ${result.reason ?? "unknown reason"}`);
      await mutate();
    } finally {
      setVerifyingId(null);
    }
  }

  return (
    <div className="flex max-w-2xl flex-col gap-6">
      <div>
        <h1 className="text-page-title font-bold">SSH deploy key</h1>
        <p className="mt-1 text-body text-ink-500">
          Organization: <span className="text-mono-path text-ink-300">{tenant}</span>
        </p>
      </div>

      <form onSubmit={handleSubmit} className="flex flex-col gap-3 rounded-md border border-border-strong bg-panel p-4">
        <Input value={repoId} onChange={(e) => setRepoId(e.target.value)} placeholder="repo id (e.g. widgets)" required />
        <Input value={repoUrl} onChange={(e) => setRepoUrl(e.target.value)} placeholder="git@github.com:org/repo.git" required className="font-mono text-mono-path" />
        <Button type="submit" disabled={submitting} className="self-start">
          {submitting ? "Generating…" : "Generate deploy key"}
        </Button>
        {formError && <ErrorState message={formError} />}
      </form>

      {error && <ErrorState message={error instanceof Error ? error.message : String(error)} />}
      {isLoading && <Skeleton className="h-24 w-full" />}

      {!isLoading && connections && connections.length === 0 && (
        <EmptyState icon={KeyRound} title="No connections yet" description="Generate a deploy key above to connect your first repo." />
      )}

      {connections && connections.length > 0 && (
        <div className="flex flex-col gap-3">
          {connections.map((c) => (
            <div key={c.id} className="rounded-md border border-border-strong bg-panel p-4">
              <div className="flex items-center justify-between gap-2">
                <span className="truncate font-mono text-mono-path text-ink-100">{c.repo_url}</span>
                <HealthBadge status={connectionHealth(c.status)} />
              </div>
              {isFailedStatus(c.status) && failureReason(c.status) && (
                <p className="mt-1 text-mono-code text-health-failed">{failureReason(c.status)}</p>
              )}
              {c.public_key_openssh && (
                <div className="mt-3 flex flex-col gap-1.5">
                  <p className="text-body text-ink-500">Add this as a read-only Deploy Key on the repo (Settings → Deploy Keys):</p>
                  <div className="flex items-center gap-2">
                    <code className="flex-1 overflow-x-auto rounded bg-raised p-2 text-mono-code text-ink-100">{c.public_key_openssh}</code>
                    <CopyButton value={c.public_key_openssh} label="" />
                  </div>
                </div>
              )}
              <div className="mt-3 flex gap-2">
                <Button size="sm" variant="outline" disabled={verifyingId === c.id} onClick={() => handleVerify(c.id)}>
                  {verifyingId === c.id ? "Verifying…" : "Verify access"}
                </Button>
                <Button asChild size="sm" variant="ghost">
                  <Link href={`/repos/connect/${encodeURIComponent(c.id)}/progress`}>View status</Link>
                </Button>
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

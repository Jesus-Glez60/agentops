"use client";

import { use } from "react";
import Link from "next/link";
import useSWR from "swr";
import { CheckCircle2, CircleX, Loader2 } from "lucide-react";
import { getConnections } from "@/lib/api/heavy-api";
import { useTenant } from "@/lib/tenant-context";
import { isFailedStatus, failureReason } from "@/lib/api/types";
import { EmptyState } from "@/components/shared/empty-state";
import { Skeleton } from "@/components/ui/skeleton";
import { Button } from "@/components/ui/button";

/**
 * Honest 3-state connection status, not the mockup's 9-stage live-log
 * indexing progress screen -- that implies a clone+index pipeline with
 * per-stage job tracking that doesn't exist server-side today (heavy-api's
 * connect flow only does credential custody + a verification clone, see
 * SECURITY.md). This is the real, reachable version: pending -> active, or
 * pending -> failed with a reason. A staged/live-log version is Phase 3
 * backend work, not something to fake here.
 */
export default function ConnectionProgressPage({ params }: { params: Promise<{ id: string }> }) {
  const { id } = use(params);
  const { tenant, hasTenant } = useTenant();

  const { data: connections, isLoading } = useSWR(hasTenant ? ["connections", tenant] : null, () => getConnections(tenant!), {
    refreshInterval: 3000,
  });

  const connection = connections?.find((c) => c.id === id);

  if (!hasTenant) return <EmptyState icon={Loader2} title="No organization selected" />;
  if (isLoading) return <Skeleton className="h-48 w-full" />;
  if (!connection) return <EmptyState icon={CircleX} title="Connection not found" description={`No connection '${id}' for this organization.`} />;

  const isActive = connection.status === "active";
  const isFailed = isFailedStatus(connection.status);
  const isPending = !isActive && !isFailed;

  return (
    <div className="flex max-w-xl flex-col gap-6">
      <div>
        <h1 className="text-page-title font-bold">Connection status</h1>
        <p className="mt-1 truncate font-mono text-mono-path text-ink-500">{connection.repo_url}</p>
      </div>

      <div className="flex flex-col gap-3 rounded-md border border-border-strong bg-panel p-4">
        <div className="flex items-center gap-2">
          {isPending && <Loader2 className="size-5 animate-spin text-health-scanning" />}
          {isActive && <CheckCircle2 className="size-5 text-health-healthy" />}
          {isFailed && <CircleX className="size-5 text-health-failed" />}
          <span className="text-section font-medium text-ink-100">
            {isPending && "Waiting for verification"}
            {isActive && "Connection active"}
            {isFailed && "Connection failed"}
          </span>
        </div>
        {isFailed && failureReason(connection.status) && <p className="text-body text-health-failed">{failureReason(connection.status)}</p>}
        {isPending && <p className="text-body text-ink-500">Add the deploy key to GitHub, then verify access from the connect page.</p>}
      </div>

      <Button asChild variant="outline" className="w-fit">
        <Link href="/repos/connect/ssh">Back to connections</Link>
      </Button>
    </div>
  );
}

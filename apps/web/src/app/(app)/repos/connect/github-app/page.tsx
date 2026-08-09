"use client";

import useSWR from "swr";
import Link from "next/link";
import { GitBranch } from "lucide-react";
import { getGithubAppInstallUrl } from "@/lib/api/heavy-api";
import { EmptyState } from "@/components/shared/empty-state";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";

export default function GithubAppConnectPage() {
  const { data, isLoading } = useSWR("github-app-install-url", () => getGithubAppInstallUrl());

  return (
    <div className="flex max-w-2xl flex-col gap-6">
      <div>
        <h1 className="text-page-title font-bold">GitHub App</h1>
        <p className="mt-1 text-body text-ink-500">
          AgentOps requests <strong>read-only</strong> access to repository contents. No write operations or secrets are accessed. You
          can revoke access at any time from GitHub App settings.
        </p>
      </div>

      {isLoading && <Skeleton className="h-10 w-48" />}

      {data && (
        <Button asChild className="w-fit gap-1.5">
          <a href={data.install_url} target="_blank" rel="noreferrer">
            <GitBranch className="size-4" />
            Install GitHub App
          </a>
        </Button>
      )}

      {/* Honest gap: agentops-github-app has no "list repositories after
          install" endpoint yet, and no install callback records which
          installation id belongs to which tenant (see SECURITY.md) --
          there is nothing real to show here beyond the install link. */}
      <EmptyState
        icon={GitBranch}
        title="Repository selection isn't available yet"
        description="After installing the GitHub App, there's currently no way for this dashboard to list which repositories were granted access or connect them automatically -- that requires backend work (a repository-listing endpoint and an install callback) that hasn't been built. Use the SSH deploy key flow to connect a repo today."
      />

      <Button asChild variant="outline" className="w-fit">
        <Link href="/repos/connect/ssh">Use SSH deploy key instead</Link>
      </Button>
    </div>
  );
}

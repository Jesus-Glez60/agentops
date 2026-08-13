"use client";

import useSWR from "swr";
import { Clock, Database, Share2, TriangleAlert } from "lucide-react";
import { getRepos, REPOS_SWR_KEY } from "@/lib/api/agentops-api";
import { repoHealth } from "@/lib/repo-health";
import { StatCard } from "@/components/dashboard/stat-card";
import { ActivityTicker } from "@/components/dashboard/activity-ticker";
import { RepoTable } from "@/components/dashboard/repo-table";

export default function OverviewPage() {
  const { data: repos } = useSWR(REPOS_SWR_KEY, getRepos);

  const repoCount = repos?.length ?? 0;
  const nodeCount = repos?.reduce((sum, r) => sum + (r.counts ? r.counts.symbols + r.counts.files + r.counts.gotchas + r.counts.decisions : 0), 0) ?? 0;
  const gotchaCount = repos?.reduce((sum, r) => sum + (r.counts?.gotchas_needing_curation ?? 0), 0) ?? 0;
  const staleCount = repos?.filter((r) => repoHealth(r) === "stale").length ?? 0;

  return (
    <div className="flex flex-col gap-4 p-6">
      <div className="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-4">
        <StatCard label="Repositories" value={repoCount} icon={Database} />
        <StatCard label="Knowledge nodes" value={nodeCount.toLocaleString()} icon={Share2} />
        <StatCard label="Gotchas needing curation" value={gotchaCount} icon={TriangleAlert} valueClassName="text-health-warning" href="/gotchas" />
        <StatCard label="Stale index" value={staleCount} icon={Clock} valueClassName="text-health-stale" />
      </div>

      <ActivityTicker />

      <RepoTable />
    </div>
  );
}

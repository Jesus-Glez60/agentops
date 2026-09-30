"use client";

import useSWR from "swr";
import { Database, Share2, Sparkles, TriangleAlert } from "lucide-react";
import { getRepos, getRepoUsage, REPOS_SWR_KEY } from "@/lib/api/repos-api";
import { getMyMemberships, resolveOrgDisplayName, MY_MEMBERSHIPS_SWR_KEY } from "@/lib/api/team-api";
import { repoHealth } from "@/lib/repo-health";
import type { SessionUser } from "@/lib/auth/types";
import { StatCard } from "@/components/dashboard/stat-card";
import { ActivityTicker } from "@/components/dashboard/activity-ticker";
import { RepoTable } from "@/components/dashboard/repo-table";
import { OverviewHero } from "@/components/dashboard/overview-hero";
import { NextUp, buildNextUpCards } from "@/components/dashboard/next-up";

export function OverviewPageClient({ user }: { user: SessionUser }) {
  const { data } = useSWR(REPOS_SWR_KEY, getRepos);
  const repos = data?.connections;
  const { data: membershipsData } = useSWR(MY_MEMBERSHIPS_SWR_KEY, getMyMemberships);
  const orgName = resolveOrgDisplayName(membershipsData?.memberships ?? [], user);

  // Sums each connected repo's real, exact knowledge-reuse hit count
  // (agentops_api::usage::usage_summary) -- an honest all-time total, not
  // "· 30d" as the prototype's mockup copy shows, since the backend query
  // has no date-range filtering yet. See redesign plan's Session 3 update.
  const { data: totalHits } = useSWR(repos ? ["knowledge-hits", repos.map((r) => r.id).join(",")] : null, () => Promise.all(repos!.map((r) => getRepoUsage(r.id))).then((summaries) => summaries.reduce((sum, s) => sum + s.hit_count, 0)));

  const repoCount = repos?.length ?? 0;
  const nodeCount = repos?.reduce((sum, r) => sum + (r.counts ? r.counts.symbols + r.counts.files + r.counts.gotchas + r.counts.decisions : 0), 0) ?? 0;
  const gotchaCount = repos?.reduce((sum, r) => sum + (r.counts?.gotchas_needing_curation ?? 0), 0) ?? 0;
  const anyRepoUnhealthy = repos?.some((r) => repoHealth(r) !== "healthy") ?? false;
  const nextUpCards = repos ? buildNextUpCards(repos, gotchaCount) : [];

  return (
    <div className="flex flex-col gap-6 p-6">
      <OverviewHero orgName={orgName} firstName={user.first_name} needsCurationCount={gotchaCount} anyRepoUnhealthy={anyRepoUnhealthy} />

      <NextUp cards={nextUpCards} />

      <div className="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-4">
        <StatCard label="Repositories" value={repoCount} icon={Database} />
        <StatCard label="Knowledge nodes" value={nodeCount.toLocaleString()} icon={Share2} />
        <StatCard label="Gotchas needing curation" value={gotchaCount} icon={TriangleAlert} valueClassName="text-health-warning" href="/gotchas" />
        <StatCard label="Knowledge hits" value={totalHits === undefined ? "—" : totalHits.toLocaleString()} icon={Sparkles} valueClassName="text-health-healthy" />
      </div>

      <div className="grid grid-cols-1 gap-4 lg:grid-cols-[2fr_1fr]">
        <RepoTable />
        <ActivityTicker />
      </div>
    </div>
  );
}

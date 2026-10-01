"use client";

import useSWR from "swr";
import { getRepos, getRepoUsage, REPOS_SWR_KEY } from "@/lib/api/repos-api";
import { getMyMemberships, resolveOrgDisplayName, MY_MEMBERSHIPS_SWR_KEY } from "@/lib/api/team-api";
import { repoHealth } from "@/lib/repo-health";
import type { SessionUser } from "@/lib/auth/types";
import { StatStrip } from "@/components/shared/stat-strip";
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
  const indexedCount = repos?.filter((r) => r.counts).length ?? 0;
  const waitingCount = repoCount - indexedCount;
  const nodeCount = repos?.reduce((sum, r) => sum + (r.counts ? r.counts.symbols + r.counts.files + r.counts.gotchas + r.counts.decisions : 0), 0) ?? 0;
  const gotchaCount = repos?.reduce((sum, r) => sum + (r.counts?.gotchas_needing_curation ?? 0), 0) ?? 0;
  const gotchaRepoCount = repos?.filter((r) => (r.counts?.gotchas_needing_curation ?? 0) > 0).length ?? 0;
  const anyRepoUnhealthy = repos?.some((r) => repoHealth(r) !== "healthy") ?? false;
  const nextUpCards = repos ? buildNextUpCards(repos, gotchaCount) : [];

  return (
    <div className="flex h-full flex-col">
      {/* Hero band -- bordered off from the rest of the page, with the same
          per-screen radial-gradient corner bleed every other hero has
          (exact prototype values for this screen: mauve top-left, peach
          bottom-right). Previously this sat flush with "Next up" below it
          with no visual separation at all. */}
      <div className="relative overflow-hidden border-b bg-canvas px-6 py-9">
        <div
          aria-hidden
          className="pointer-events-none absolute inset-0"
          style={{
            background: "radial-gradient(ellipse 80% 50% at 20% -5%, rgba(203,166,247,0.32), transparent 70%), radial-gradient(ellipse 50% 40% at 90% 100%, rgba(250,179,135,0.14), transparent 70%)",
          }}
        />
        <div className="relative">
          <OverviewHero orgName={orgName} firstName={user.first_name} needsCurationCount={gotchaCount} anyRepoUnhealthy={anyRepoUnhealthy} />
        </div>
      </div>

      <div className="flex flex-col gap-6 p-6">
        <NextUp cards={nextUpCards} />

        <StatStrip
          items={[
            { label: "Repositories", value: repoCount, note: repoCount > 0 ? `${indexedCount} indexed${waitingCount > 0 ? ` · ${waitingCount} waiting on credentials` : ""}` : undefined },
            { label: "Knowledge nodes", value: nodeCount.toLocaleString(), note: "symbols, files, gotchas, decisions" },
            { label: "Gotchas needing curation", value: gotchaCount, valueClassName: "text-health-warning", href: "/gotchas", note: gotchaRepoCount > 0 ? `across ${gotchaRepoCount} repositor${gotchaRepoCount === 1 ? "y" : "ies"}` : undefined },
            { label: "Knowledge hits", value: totalHits === undefined ? "—" : totalHits.toLocaleString(), valueClassName: "text-health-healthy", note: "answers served from the graph" },
          ]}
        />

        <div className="grid grid-cols-1 gap-4 lg:grid-cols-[2fr_1fr]">
          <RepoTable />
          <ActivityTicker />
        </div>
      </div>
    </div>
  );
}

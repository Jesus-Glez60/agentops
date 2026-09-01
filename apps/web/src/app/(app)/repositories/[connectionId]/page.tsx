"use client";

// Distinct from the narrower `[connectionId]/index/page.tsx` (indexing-
// progress-only, job stages/retry/regenerate-key) -- this is the general
// "click a row, zoom into detail" page the dashboard's "View details"
// button links to, modeled on `libraries/[slug]/page.tsx`'s layout.
//
// Explicitly out of scope for this pass: a multi-job history list and raw
// job-log line viewer -- no backend endpoint exists for either yet
// (`indexing_status` only returns the latest/specified single job). Job
// progress detail is a link out to the existing `.../index` page instead
// of duplicating its polling logic here.

import { Suspense } from "react";
import Link from "next/link";
import { useParams } from "next/navigation";
import useSWR, { useSWRConfig } from "swr";
import { ArrowLeft, ArrowRight, GitBranch } from "lucide-react";
import { getRepos, parseRepoStatus, REPOS_SWR_KEY } from "@/lib/api/repos-api";
import { repoHealthWithReason } from "@/lib/repo-health";
import { HealthBadge } from "@/components/dashboard/health-badge";
import { NodeCountBar } from "@/components/dashboard/node-count-bar";
import { BranchSelect } from "@/components/repositories/branch-select";
import { Button } from "@/components/ui/button";

const METHOD_LABELS: Record<string, string> = {
  ssh: "SSH deploy key",
  github_app: "GitHub App",
};

export default function RepoDetailPage() {
  // useParams doesn't strictly need Suspense, but this page also reads no
  // search params today and may grow to (e.g. a tab query param, matching
  // the library detail page's pattern) -- wrapping now avoids a later
  // build-time surprise.
  return (
    <Suspense fallback={null}>
      <RepoDetailPageInner />
    </Suspense>
  );
}

function RepoDetailPageInner() {
  const { connectionId } = useParams<{ connectionId: string }>();
  const { data, isLoading } = useSWR(REPOS_SWR_KEY, getRepos);
  const { mutate } = useSWRConfig();

  const repo = data?.connections.find((c) => c.id === connectionId);

  if (isLoading) {
    return <p className="p-8 text-body text-ink-500">Loading…</p>;
  }
  if (!repo) {
    return <p className="p-8 text-body text-ink-500">No repository named &quot;{connectionId}&quot; is connected.</p>;
  }

  const status = parseRepoStatus(repo.status);
  const { status: health, reason } = repoHealthWithReason(repo);
  const totalNodes = repo.counts ? repo.counts.symbols + repo.counts.files + repo.counts.gotchas + repo.counts.decisions : null;

  return (
    <div className="flex h-full flex-col">
      <div className="flex h-[52px] shrink-0 items-center gap-2 border-b border-border-strong px-5 text-section">
        <Link href="/repositories" className="flex items-center gap-1.5 text-ink-400 transition-colors hover:text-ink-100">
          <ArrowLeft className="size-3.5" />
          Repositories
        </Link>
        <span className="text-ink-600">/</span>
        <span className="font-medium text-ink-100">{repo.id}</span>
      </div>

      <div className="flex-1 overflow-y-auto px-8 py-6">
        <div className="mb-1 flex items-center gap-2">
          <GitBranch className="size-4 text-ink-500" />
          <h1 className="text-lg font-semibold text-ink-100">{repo.id}</h1>
        </div>
        <p className="truncate text-mono-path text-ink-500">{repo.repo_url}</p>

        <div className="mt-6 grid max-w-[640px] grid-cols-2 gap-x-8 gap-y-5">
          <Field label="Connection method">
            <span className="text-body text-ink-200">{METHOD_LABELS[repo.method] ?? repo.method}</span>
          </Field>
          <Field label="Branch">
            <BranchSelect repo={repo} onChanged={() => mutate(REPOS_SWR_KEY)} className="w-full" />
          </Field>
          <Field label="Health">
            <HealthBadge status={health} reason={reason} />
          </Field>
          <Field label="Status">
            <span className="text-mono-code text-ink-300">{status.kind === "failed" ? status.reason : status.kind}</span>
          </Field>
          <Field label="Nodes">
            {repo.counts ? (
              <div className="flex flex-col gap-1">
                <NodeCountBar counts={repo.counts} className="w-48" />
                <span className="text-mono-code text-ink-500">{totalNodes} total</span>
              </div>
            ) : (
              <span className="text-mono-code text-ink-500">not yet scanned</span>
            )}
          </Field>
        </div>

        <div className="mt-8">
          <Button variant="outline" size="sm" asChild>
            <Link href={`/repositories/${encodeURIComponent(repo.id)}/index`}>
              Indexing progress
              <ArrowRight className="size-3.5" />
            </Link>
          </Button>
        </div>
      </div>
    </div>
  );
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div>
      <p className="mb-1.5 text-mono-code uppercase text-ink-500">{label}</p>
      {children}
    </div>
  );
}

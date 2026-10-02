"use client";

// Distinct from the narrower `[connectionId]/index/page.tsx` (indexing-
// progress-only, job stages/retry/regenerate-key) -- this is the general
// "click a row, zoom into detail" page the dashboard's "View details"
// button links to, modeled on `libraries/[slug]/page.tsx`'s two-column
// layout (content left, meta/actions sidebar right) -- that pattern
// already existed there before this pass touched it; this page adopts it
// for consistency between the app's two sibling detail screens, not
// because the prototype depicts a "Repository detail" screen (it doesn't
// have one at all, confirmed via grep for every `data-screen-label=`).
//
// Explicitly out of scope for this pass: a multi-job history list and raw
// job-log line viewer -- no backend endpoint exists for either yet
// (`indexing_status` only returns the latest/specified single job). Job
// progress detail is a link out to the existing `.../index` page instead
// of duplicating its polling logic here.
//
// Split out of `page.tsx` (a server component) so this client component can
// receive `apiUrl` as a prop -- `publicApiUrl()` reads request headers via
// `next/headers`, a server-only API -- needed for the Usage card's
// `usage sync --remote` command (see `UsageCard`'s doc comment).

import { Suspense } from "react";
import Link from "next/link";
import { useParams } from "next/navigation";
import useSWR, { useSWRConfig } from "swr";
import { ArrowLeft, ArrowRight } from "lucide-react";
import { getRepos, getRepoUsage, parseRepoStatus, REPOS_SWR_KEY } from "@/lib/api/repos-api";
import { repoHealthWithReason } from "@/lib/repo-health";
import { HealthBadge } from "@/components/dashboard/health-badge";
import { NodeCountBar } from "@/components/dashboard/node-count-bar";
import { UsageCard } from "@/components/dashboard/usage-card";
import { BranchSelect } from "@/components/repositories/branch-select";
import { Button } from "@/components/ui/button";

const METHOD_LABELS: Record<string, string> = {
  ssh: "SSH deploy key",
  github_app: "GitHub App",
};

export function RepoDetailPageClient({ apiUrl }: { apiUrl: string }) {
  // useParams doesn't strictly need Suspense, but this page also reads no
  // search params today and may grow to (e.g. a tab query param, matching
  // the library detail page's pattern) -- wrapping now avoids a later
  // build-time surprise.
  return (
    <Suspense fallback={null}>
      <RepoDetailPageInner apiUrl={apiUrl} />
    </Suspense>
  );
}

function RepoDetailPageInner({ apiUrl }: { apiUrl: string }) {
  const { connectionId } = useParams<{ connectionId: string }>();
  const { data, isLoading } = useSWR(REPOS_SWR_KEY, getRepos);
  const { mutate } = useSWRConfig();

  const repo = data?.connections.find((c) => c.id === connectionId);
  const { data: usage } = useSWR(repo ? `/repos/${repo.id}/usage` : null, () => getRepoUsage(repo!.id));

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

      <div className="flex min-h-0 flex-1 overflow-hidden">
        <div className="flex-1 overflow-y-auto px-8 py-6">
          <h1 className="text-display-card font-bold text-ink-100">{repo.id}</h1>
          <p className="mt-1 truncate text-mono-path text-ink-500">{repo.repo_url}</p>

          <div className="mt-6 max-w-[640px]">
            <UsageCard usage={usage ?? null} apiUrl={apiUrl} />
          </div>
        </div>

        <div className="w-[260px] shrink-0 space-y-4 overflow-y-auto border-l border-border-strong bg-panel p-4">
          <div>
            <p className="mb-2 text-mono-code uppercase text-ink-500">Connection</p>
            <dl className="space-y-3">
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
                    <NodeCountBar counts={repo.counts} />
                    <span className="text-mono-code text-ink-500">{totalNodes} total</span>
                  </div>
                ) : (
                  <span className="text-mono-code text-ink-500">not yet scanned</span>
                )}
              </Field>
            </dl>
          </div>
          <div className="space-y-1.5 border-t border-border-strong pt-3">
            <p className="mb-2 text-mono-code uppercase text-ink-500">Actions</p>
            <Button size="cta" variant="outline" className="w-full justify-center" asChild>
              <Link href={`/repositories/${encodeURIComponent(repo.id)}/index`}>
                Indexing progress
                <ArrowRight className="size-3.5" />
              </Link>
            </Button>
          </div>
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

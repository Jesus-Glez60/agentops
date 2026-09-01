"use client";

import Link from "next/link";
import useSWR from "swr";
import { ExternalLink } from "lucide-react";
import { getGithubAppInstallations, getInstallationRepos, GITHUB_APP_INSTALLATIONS_SWR_KEY, type GithubAppInstallation } from "@/lib/api/repos-api";
import { relativeTimeFromIsoString } from "@/lib/relative-time";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";

/**
 * Not a `KNOWN_PROVIDERS`/`ProviderRow` entry alongside Linear/Anthropic in
 * `org-integrations-tab.tsx` -- a GitHub App installation isn't a
 * `/integrations/*`-vault `IntegrationSummary` (pasted API key), it's a
 * separate `/repos/github-app/*` resource with its own connect flow (an
 * install redirect through github.com, not a form submit). Kept as its own
 * card so `ProviderRow`'s shape doesn't need bending to fit a second kind
 * of "connected" state.
 */
export function GithubAppIntegrationCard() {
  const { data, isLoading } = useSWR(GITHUB_APP_INSTALLATIONS_SWR_KEY, getGithubAppInstallations);
  const installations = data?.installations ?? [];

  return (
    <Card className="mt-4">
      <CardHeader className="border-b border-border-strong pb-4">
        <CardTitle>GitHub App</CardTitle>
      </CardHeader>
      <CardContent className="divide-y divide-border-strong p-0">
        {isLoading && <p className="px-6 py-4 text-body text-ink-500">Loading…</p>}
        {!isLoading && installations.length === 0 && (
          <div className="flex items-center justify-between gap-4 px-6 py-4">
            <div className="min-w-0">
              <p className="text-body font-medium text-ink-100">Not connected</p>
              <p className="truncate text-mono-code text-ink-500">Install the AgentOps GitHub App to connect repositories without managing per-repo SSH keys.</p>
            </div>
            <Button asChild size="sm" className="shrink-0">
              <Link href="/repositories/connect">Connect</Link>
            </Button>
          </div>
        )}
        {installations.map((installation) => (
          <InstallationRow key={installation.id} installation={installation} />
        ))}
      </CardContent>
    </Card>
  );
}

function InstallationRow({ installation }: { installation: GithubAppInstallation }) {
  const { data: reposData, isLoading: reposLoading } = useSWR(["github-app-installation-repos", installation.id], () => getInstallationRepos(installation.id));
  const repos = reposData?.repositories ?? [];

  return (
    <div className="px-6 py-4">
      <div className="flex items-center justify-between gap-4">
        <div className="min-w-0">
          <p className="text-body font-medium text-ink-100">Connected as {installation.account_login}</p>
          <p className="truncate text-mono-code text-ink-500">Installed {relativeTimeFromIsoString(installation.installed_at)}</p>
        </div>
        <div className="flex shrink-0 items-center gap-2">
          <Button asChild size="sm" variant="outline">
            <Link href="/repositories/connect">Add repository</Link>
          </Button>
          <Button asChild size="sm" variant="outline">
            <a href={installation.manage_url} target="_blank" rel="noreferrer">
              Manage on GitHub
              <ExternalLink className="size-3.5" />
            </a>
          </Button>
        </div>
      </div>
      <div className="mt-3">
        <p className="mb-1.5 text-mono-code uppercase text-ink-500">Authorized repositories</p>
        {reposLoading && <p className="text-mono-code text-ink-500">Loading…</p>}
        {!reposLoading && repos.length === 0 && <p className="text-mono-code text-ink-500">None authorized yet.</p>}
        {!reposLoading && repos.length > 0 && (
          <ul className="space-y-0.5">
            {repos.map((repo) => (
              <li key={repo.full_name} className="text-mono-code text-ink-300">
                {repo.full_name}
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}

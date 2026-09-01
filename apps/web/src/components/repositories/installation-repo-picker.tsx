"use client";

import { useEffect, useState } from "react";
import { useRouter } from "next/navigation";
import useSWR, { useSWRConfig } from "swr";
import { toast } from "sonner";
import { GitBranch, Lock } from "lucide-react";
import { connectFromInstallation, getInstallationRepos, getRepos, GITHUB_APP_INSTALLATIONS_SWR_KEY, REPOS_SWR_KEY, type InstallationRepo } from "@/lib/api/repos-api";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";

/** Normalizes either connection method's `repo_url` shape down to `owner/repo`, matching GitHub API's `full_name` format. */
function repoFullNameFromUrl(url: string): string {
  return url
    .replace(/^git@github\.com:/, "")
    .replace(/^https?:\/\/github\.com\//, "")
    .replace(/\.git$/, "");
}

/**
 * The GitHub App installation's authorized-repos checklist + connect
 * action -- shared by the fresh-install flow (`select/page.tsx`, reached
 * after a genuinely new GitHub install redirects back to us) and the
 * already-connected "Connect repository" flow (`connect/page.tsx`, which
 * skips the GitHub round trip entirely once an installation exists, since
 * GitHub only redirects back for a brand-new install, never an update).
 * One implementation instead of two copies that could drift.
 */
export function InstallationRepoPicker({ installationId }: { installationId: string }) {
  const router = useRouter();
  const { mutate } = useSWRConfig();

  const [repos, setRepos] = useState<InstallationRepo[] | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [loadError, setLoadError] = useState<string | null>(null);
  const [connecting, setConnecting] = useState(false);

  // Repos already connected (any method) get excluded below -- reselecting
  // one here would just 409 per-repo against the backend's existing-id
  // check, a confusing dead end now that this picker is front-and-center
  // rather than a one-time post-install step. `repo_url` differs by
  // connection method (SSH: `git@github.com:owner/repo.git`, GitHub App:
  // `https://github.com/owner/repo.git`) -- both need normalizing to
  // `owner/repo` before comparing against GitHub's `full_name`, or an
  // SSH-connected repo silently isn't recognized as already-connected.
  const { data: reposData } = useSWR(REPOS_SWR_KEY, getRepos);
  const connectedFullNames = new Set((reposData?.connections ?? []).map((c) => repoFullNameFromUrl(c.repo_url)));

  useEffect(() => {
    getInstallationRepos(installationId)
      .then((res) => {
        setRepos(res.repositories);
        // Set by `/repositories/connect/local` before redirecting into
        // GitHub's OAuth install flow -- a URL query param wouldn't survive
        // that external round-trip without new backend plumbing to forward
        // it through the callback, but sessionStorage does (same tab, same
        // origin). Read once and clear, so it doesn't linger for an
        // unrelated future visit to this page.
        const target = sessionStorage.getItem("agentops:local-connect:target-repo");
        if (target) {
          sessionStorage.removeItem("agentops:local-connect:target-repo");
          if (res.repositories.some((r) => r.full_name === target)) {
            setSelected(new Set([target]));
          }
        }
      })
      .catch((err) => setLoadError(err instanceof Error ? err.message : "Couldn't load repositories for this installation."));
  }, [installationId]);

  function toggle(fullName: string) {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(fullName)) next.delete(fullName);
      else next.add(fullName);
      return next;
    });
  }

  async function handleConnect() {
    if (selected.size === 0) return;
    setConnecting(true);
    try {
      const res = await connectFromInstallation(installationId, Array.from(selected));
      toast.success(res.connections.length === 1 ? "Repository connected — indexing started." : `${res.connections.length} repositories connected — indexing started.`);
      mutate(GITHUB_APP_INSTALLATIONS_SWR_KEY);
      mutate(REPOS_SWR_KEY);
      const first = res.connections[0];
      if (first) {
        router.push(`/repositories/${encodeURIComponent(first.connection.id)}/index`);
      } else {
        router.push("/repositories");
      }
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Couldn't connect the selected repositories. Please try again.");
    } finally {
      setConnecting(false);
    }
  }

  const availableRepos = repos?.filter((r) => !connectedFullNames.has(r.full_name)) ?? null;

  return (
    <div>
      <div className="overflow-hidden rounded-lg border border-border-strong bg-panel">
        <div className="flex items-center gap-3 border-b border-border-strong px-4 py-3">
          <span className="text-section font-medium text-ink-100">Authorized repositories</span>
          <span className="ml-auto text-section text-ink-400">
            <span className="text-ink-100">{selected.size}</span> of {availableRepos?.length ?? 0} selected
          </span>
        </div>
        <div className="divide-y divide-border-strong">
          {loadError && <div className="px-4 py-6 text-center text-section text-health-failed">{loadError}</div>}
          {!loadError && availableRepos === null && <div className="px-4 py-6 text-center text-section text-ink-500">Loading…</div>}
          {!loadError && availableRepos?.length === 0 && (
            <div className="px-4 py-6 text-center text-section text-ink-500">
              Every authorized repository is already connected. Use &quot;Manage on GitHub&quot; to authorize more.
            </div>
          )}
          {availableRepos?.map((repo) => (
            <label key={repo.full_name} className="flex cursor-pointer items-center gap-3 px-4 py-2.5 hover:bg-white/5">
              <Checkbox checked={selected.has(repo.full_name)} onCheckedChange={() => toggle(repo.full_name)} />
              <GitBranch className="size-4 text-ink-400" />
              <div className="flex-1">
                <span className="text-section font-medium text-ink-100">{repo.full_name}</span>
                <span className="ml-2 text-mono-code text-ink-500">
                  {repo.default_branch}
                  {repo.language ? ` · ${repo.language}` : ""}
                </span>
              </div>
            </label>
          ))}
        </div>
      </div>

      <div className="mt-5 flex items-start gap-3 rounded-md border border-border-strong bg-panel px-3.5 py-3 text-section text-ink-400">
        <Lock className="mt-0.5 size-4 shrink-0 text-ink-500" />
        <p>
          AgentOps requests <strong className="text-ink-300">read-only</strong> access to repository contents. No write operations or secrets are accessed. You can revoke access at any time from GitHub App
          settings.
        </p>
      </div>

      <div className="mt-6 flex items-center gap-3">
        <Button onClick={handleConnect} disabled={connecting || selected.size === 0}>
          {connecting ? "Connecting…" : `Connect ${selected.size} ${selected.size === 1 ? "repository" : "repositories"} →`}
        </Button>
      </div>
    </div>
  );
}

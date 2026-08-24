"use client";

import { useEffect, useState } from "react";
import { useRouter, useSearchParams } from "next/navigation";
import Link from "next/link";
import { toast } from "sonner";
import { ArrowLeft, GitBranch, Lock } from "lucide-react";
import { connectFromInstallation, getInstallationRepos, type InstallationRepo } from "@/lib/api/repos-api";
import { StepIndicator } from "@/components/repositories/connect-wizard/step-indicator";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";

export default function SelectGithubAppReposPage() {
  const router = useRouter();
  const searchParams = useSearchParams();
  const installationId = searchParams.get("installation_id");

  const [repos, setRepos] = useState<InstallationRepo[] | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [loadError, setLoadError] = useState<string | null>(null);
  const [connecting, setConnecting] = useState(false);

  useEffect(() => {
    if (!installationId) return;
    getInstallationRepos(installationId)
      .then((res) => setRepos(res.repositories))
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
    if (!installationId || selected.size === 0) return;
    setConnecting(true);
    try {
      const res = await connectFromInstallation(installationId, Array.from(selected));
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

  if (!installationId) {
    return (
      <div className="mx-auto w-full max-w-[680px] px-6 py-10 text-section text-ink-400">
        Missing installation id. <Link href="/repositories/connect" className="text-primary underline">Start over</Link>.
      </div>
    );
  }

  return (
    <div className="mx-auto w-full max-w-[680px] px-6 py-10">
      <Link href="/repositories/connect" className="mb-6 inline-flex items-center gap-1.5 text-section text-ink-400 hover:text-ink-100">
        <ArrowLeft className="size-3.5" />
        Connect
      </Link>

      <StepIndicator
        steps={[
          { label: "Method", status: "done" },
          { label: "Install app", status: "done" },
          { label: "Select repositories", status: "active" },
          { label: "Verify & index", status: "pending" },
        ]}
      />

      <h1 className="text-page-title font-semibold text-ink-100">Select repositories to index</h1>
      <p className="mt-1 text-section text-ink-400">Choose which repositories from this installation to connect.</p>

      <div className="mt-6 overflow-hidden rounded-lg border border-border-strong bg-panel">
        <div className="flex items-center gap-3 border-b border-border-strong px-4 py-3">
          <span className="text-section font-medium text-ink-100">Installed repositories</span>
          <span className="ml-auto text-section text-ink-400">
            <span className="text-ink-100">{selected.size}</span> of {repos?.length ?? 0} selected
          </span>
        </div>
        <div className="divide-y divide-border-strong">
          {loadError && <div className="px-4 py-6 text-center text-section text-health-failed">{loadError}</div>}
          {!loadError && repos === null && <div className="px-4 py-6 text-center text-section text-ink-500">Loading…</div>}
          {!loadError && repos?.length === 0 && <div className="px-4 py-6 text-center text-section text-ink-500">No repositories found for this installation.</div>}
          {repos?.map((repo) => (
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
        <Button variant="outline" asChild>
          <Link href="/repositories/connect">Back</Link>
        </Button>
      </div>
    </div>
  );
}

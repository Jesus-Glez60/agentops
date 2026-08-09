"use client";

import Link from "next/link";
import useSWR from "swr";
import { GitBranch, KeyRound, Check, X } from "lucide-react";
import { getGithubAppInstallUrl } from "@/lib/api/heavy-api";
import { ApiError } from "@/lib/api/fetcher";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";

const GITHUB_APP_FEATURES = [
  { ok: true, label: "Automatic webhook updates" },
  { ok: true, label: "Multi-repository install" },
  { ok: true, label: "Granular permission control" },
  { ok: true, label: "No per-repo key management" },
];

const SSH_FEATURES = [
  { ok: true, label: "Per-repository access only" },
  { ok: true, label: "Read-only key by default" },
  { ok: false, label: "No automatic webhook updates" },
  { ok: false, label: "Requires manual key rotation" },
];

export default function ConnectMethodPage() {
  const { data, error } = useSWR("github-app-install-url", () =>
    getGithubAppInstallUrl().then(
      (r) => r.install_url,
      (e) => {
        if (e instanceof ApiError && e.status === 404) return null;
        throw e;
      },
    ),
  );
  const installUrl = data ?? null;
  const githubAppConfigured = installUrl !== null && !error;

  return (
    <div className="flex max-w-4xl flex-col gap-6">
      <div>
        <h1 className="text-page-title font-bold">Connect repository</h1>
        <p className="mt-1 text-body text-ink-500">Select how AgentOps should access your repository.</p>
      </div>

      <div className="grid gap-4 md:grid-cols-2">
        <Card className="border-border-strong bg-panel">
          <CardHeader>
            <div className="flex items-center justify-between">
              <CardTitle className="flex items-center gap-2 text-subheading">
                <GitBranch className="size-5" />
                GitHub App
              </CardTitle>
              <Badge className="bg-primary/15 text-primary">Recommended</Badge>
            </div>
          </CardHeader>
          <CardContent className="flex flex-col gap-4">
            <ul className="space-y-1.5 text-body text-ink-300">
              {GITHUB_APP_FEATURES.map((f) => (
                <li key={f.label} className="flex items-center gap-2">
                  <Check className="size-3.5 text-health-healthy" />
                  {f.label}
                </li>
              ))}
            </ul>
            {githubAppConfigured ? (
              <Button asChild className="gap-1.5">
                <Link href="/repos/connect/github-app">
                  <GitBranch className="size-4" />
                  Continue with GitHub App
                </Link>
              </Button>
            ) : (
              <p className="text-body text-ink-500">
                {data === undefined && !error ? "Checking availability…" : "No GitHub App is configured for this deployment yet."}
              </p>
            )}
          </CardContent>
        </Card>

        <Card className="border-border-strong bg-panel">
          <CardHeader>
            <CardTitle className="flex items-center gap-2 text-subheading">
              <KeyRound className="size-5" />
              SSH Deploy Key
            </CardTitle>
          </CardHeader>
          <CardContent className="flex flex-col gap-4">
            <ul className="space-y-1.5 text-body text-ink-300">
              {SSH_FEATURES.map((f) => (
                <li key={f.label} className="flex items-center gap-2">
                  {f.ok ? <Check className="size-3.5 text-health-healthy" /> : <X className="size-3.5 text-ink-500" />}
                  {f.label}
                </li>
              ))}
            </ul>
            <Button asChild variant="outline" className="gap-1.5">
              <Link href="/repos/connect/ssh">
                <KeyRound className="size-4" />
                Use SSH deploy key instead
              </Link>
            </Button>
          </CardContent>
        </Card>
      </div>
    </div>
  );
}

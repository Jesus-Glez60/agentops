"use client";

import { Suspense, useEffect, useState } from "react";
import Link from "next/link";
import { useRouter, useSearchParams } from "next/navigation";
import useSWR from "swr";
import { toast } from "sonner";
import { ArrowLeft, Cloud, FolderSearch, GitBranch, Key, ShieldCheck } from "lucide-react";
import { getGithubAppInstallUrl, getGithubAppInstallations, GITHUB_APP_INSTALLATIONS_SWR_KEY } from "@/lib/api/repos-api";
import { StepIndicator } from "@/components/repositories/connect-wizard/step-indicator";
import { InstallationRepoPicker } from "@/components/repositories/installation-repo-picker";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

/** Short stable codes the backend redirects with on `github_app_callback` failure -- see `github_app_routes.rs`'s `redirect_to_frontend` call sites. */
const GITHUB_APP_ERROR_MESSAGES: Record<string, string> = {
  no_session: "The GitHub install link expired or your session changed before it completed — try connecting again.",
  not_configured: "GitHub App isn't fully configured for this deployment yet.",
  token_exchange_failed: "Couldn't confirm the installation with GitHub — try again in a moment.",
  list_repos_failed: "Installed, but couldn't list the installation's repositories — try again in a moment.",
  save_failed: "Installed, but saving the connection failed — try again.",
};

export default function ChooseConnectionMethodPage() {
  // useSearchParams requires a Suspense boundary during static generation.
  return (
    <Suspense fallback={null}>
      <ChooseConnectionMethodPageInner />
    </Suspense>
  );
}

function ChooseConnectionMethodPageInner() {
  const router = useRouter();
  const searchParams = useSearchParams();

  // GitHub only redirects back to our callback for a brand-new install --
  // once a tenant is already connected, re-running the install flow just
  // shows GitHub's own "already installed" screen and goes nowhere useful.
  // So once we know an installation exists, this page stops being a
  // "choose a method" screen and becomes a "pick which authorized repo to
  // connect" screen instead, with SSH/local kept as secondary manual paths
  // below rather than equal-weight top-level options.
  const { data: installationsData, isLoading: installationsLoading } = useSWR(GITHUB_APP_INSTALLATIONS_SWR_KEY, getGithubAppInstallations);
  const installations = installationsData?.installations ?? [];

  useEffect(() => {
    const errorCode = searchParams.get("github_app_error");
    if (!errorCode) return;
    toast.error(GITHUB_APP_ERROR_MESSAGES[errorCode] ?? "GitHub App connection failed. Please try again.");
    router.replace("/repositories/connect");
    // Intentionally omitting `router` from deps -- Next's router identity
    // isn't stable across renders and including it would re-fire this
    // effect (and re-toast) on every navigation, not just the one that
    // carried the error param.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [searchParams]);

  if (installationsLoading) {
    return <p className="p-8 text-body text-ink-500">Loading…</p>;
  }

  if (installations.length > 0) {
    return <AlreadyConnectedView installationId={installations[0].id} accountLogin={installations[0].account_login} manageUrl={installations[0].manage_url} />;
  }

  return <ChooseMethodView />;
}

function AlreadyConnectedView({ installationId, accountLogin, manageUrl }: { installationId: string; accountLogin: string; manageUrl: string }) {
  return (
    <div className="mx-auto w-full max-w-[680px] px-6 py-12">
      <Link href="/repositories" className="mb-6 inline-flex items-center gap-1.5 text-section text-ink-400 hover:text-ink-100">
        <ArrowLeft className="size-3.5" />
        Repositories
      </Link>

      <h1 className="text-page-title font-semibold text-ink-100">Add a repository</h1>
      <p className="mt-1.5 text-section text-ink-400">
        Connected via GitHub App as <span className="text-ink-200">{accountLogin}</span>. Select a repository below, or{" "}
        <a href={manageUrl} target="_blank" rel="noreferrer" className="text-primary underline">
          authorize more on GitHub
        </a>
        .
      </p>

      <div className="mt-6">
        <InstallationRepoPicker installationId={installationId} />
      </div>

      <div className="mt-8 border-t border-border-strong pt-6">
        <p className="mb-3 text-section text-ink-500">Or connect a repository another way</p>
        <div className="flex flex-col gap-2">
          <Link href="/repositories/connect/ssh" className="flex items-center gap-3 rounded-md border border-border-strong px-4 py-3 text-section hover:border-ink-500">
            <Key className="size-4 shrink-0 text-ink-400" />
            <span className="text-ink-200">SSH deploy key</span>
          </Link>
          <Link href="/repositories/connect/local" className="flex items-center gap-3 rounded-md border border-border-strong px-4 py-3 text-section hover:border-ink-500">
            <FolderSearch className="size-4 shrink-0 text-ink-400" />
            <span className="text-ink-200">Repo on your own machine</span>
          </Link>
        </div>
      </div>
    </div>
  );
}

const METHODS = [
  {
    id: "github-app" as const,
    icon: Cloud,
    title: "GitHub App",
    badge: "RECOMMENDED",
    description: "Install the AgentOps GitHub App on your organization. Works with private repositories. Receives webhook updates automatically.",
    pros: ["Automatic webhook updates", "Multi-repository install", "No per-repo key management"],
  },
  {
    id: "ssh" as const,
    icon: Key,
    title: "SSH Deploy Key",
    badge: "MANUAL",
    description: "Generate a read-only SSH deploy key and add it to a specific repository. Suited for environments where GitHub App installation is restricted.",
    pros: ["Per-repository access only", "Read-only key by default"],
  },
];

function ChooseMethodView() {
  const [selected, setSelected] = useState<"github-app" | "ssh">("github-app");
  const [continuing, setContinuing] = useState(false);

  async function handleContinue() {
    if (selected === "ssh") {
      window.location.href = "/repositories/connect/ssh";
      return;
    }
    setContinuing(true);
    try {
      const { install_url } = await getGithubAppInstallUrl();
      window.location.href = install_url;
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "GitHub App isn't configured for this deployment yet -- try SSH deploy key instead.");
    } finally {
      setContinuing(false);
    }
  }

  return (
    <div className="mx-auto w-full max-w-[720px] px-6 py-12">
      <Link href="/repositories" className="mb-6 inline-flex items-center gap-1.5 text-section text-ink-400 hover:text-ink-100">
        <ArrowLeft className="size-3.5" />
        Repositories
      </Link>

      <StepIndicator
        steps={[
          { label: "Choose connection method", status: "active" },
          { label: "Configure", status: "pending" },
          { label: "Index", status: "pending" },
        ]}
      />

      <h1 className="text-page-title font-semibold text-ink-100">Choose a connection method</h1>
      <p className="mt-1.5 text-section text-ink-400">Select how AgentOps should access your repository. We recommend the GitHub App for most teams.</p>

      <div className="mt-8 grid grid-cols-2 gap-4">
        {METHODS.map((method) => {
          const Icon = method.icon;
          const isSelected = selected === method.id;
          return (
            <button
              key={method.id}
              type="button"
              onClick={() => setSelected(method.id)}
              className={cn(
                "rounded-lg border p-5 text-left transition-colors",
                isSelected ? "border-primary bg-primary/5" : "border-border-strong hover:border-ink-500",
              )}
            >
              <div className="mb-4 flex items-start justify-between">
                <div className="flex size-10 items-center justify-center rounded-lg border border-border-strong bg-panel">
                  <Icon className="size-5 text-ink-200" />
                </div>
                <span className={cn("rounded px-2 py-0.5 text-[10px] font-semibold", isSelected ? "bg-primary text-primary-foreground" : "border border-border-strong text-ink-400")}>
                  {method.badge}
                </span>
              </div>
              <h3 className="mb-1.5 text-[15px] font-semibold text-ink-100">{method.title}</h3>
              <p className="mb-4 text-section leading-relaxed text-ink-400">{method.description}</p>
              <div className="space-y-1.5 text-section">
                {method.pros.map((pro) => (
                  <div key={pro} className="flex items-center gap-2 text-ink-300">
                    <ShieldCheck className="size-3.5 text-health-healthy" />
                    {pro}
                  </div>
                ))}
              </div>
            </button>
          );
        })}
      </div>

      <div className="mt-6 flex items-start gap-3 rounded-md border border-border-strong bg-panel px-4 py-3.5 text-section">
        <GitBranch className="mt-0.5 size-4 shrink-0 text-primary" />
        <div>
          <p className="mb-1 font-medium text-ink-200">GitHub App permissions requested</p>
          <p className="text-ink-400">Contents: Read &middot; Metadata: Read &middot; Pull requests: Read &middot; Webhooks: Receive push events</p>
        </div>
      </div>

      <div className="mt-6 flex items-center gap-3">
        <Button onClick={handleContinue} disabled={continuing}>
          {selected === "github-app" ? (continuing ? "Redirecting…" : "Continue with GitHub App") : "Continue with SSH deploy key"}
        </Button>
      </div>

      {/* Deliberately not a third card above -- that's a select-then-Continue
          radio group keyed to two actions ("install app" / "generate key"),
          neither of which fits a local-only repo. This goes straight to its
          own page instead. */}
      <Link href="/repositories/connect/local" className="mt-8 flex items-start gap-3 rounded-md border border-border-strong px-4 py-3.5 text-section hover:border-ink-500">
        <FolderSearch className="mt-0.5 size-4 shrink-0 text-ink-400" />
        <div>
          <p className="font-medium text-ink-200">Have a repo on your own machine?</p>
          <p className="text-ink-400">Point us at a local folder and we&apos;ll tell you the right command to run — including repos that aren&apos;t pushed anywhere.</p>
        </div>
      </Link>
    </div>
  );
}

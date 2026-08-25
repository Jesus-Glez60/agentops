"use client";

import { useState } from "react";
import Link from "next/link";
import { toast } from "sonner";
import { ArrowLeft, Cloud, FolderSearch, GitBranch, Key, ShieldCheck } from "lucide-react";
import { getGithubAppInstallUrl } from "@/lib/api/repos-api";
import { StepIndicator } from "@/components/repositories/connect-wizard/step-indicator";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

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

export default function ChooseConnectionMethodPage() {
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

"use client";

import { Suspense } from "react";
import { useSearchParams } from "next/navigation";
import Link from "next/link";
import { ArrowLeft } from "lucide-react";
import { StepIndicator } from "@/components/repositories/connect-wizard/step-indicator";
import { InstallationRepoPicker } from "@/components/repositories/installation-repo-picker";

export default function SelectGithubAppReposPage() {
  // useSearchParams requires a Suspense boundary during static generation.
  return (
    <Suspense fallback={null}>
      <SelectGithubAppReposPageInner />
    </Suspense>
  );
}

function SelectGithubAppReposPageInner() {
  const searchParams = useSearchParams();
  const installationId = searchParams.get("installation_id");

  if (!installationId) {
    return (
      <div className="mx-auto w-full max-w-[680px] px-6 py-10 text-section text-ink-400">
        Missing installation id.{" "}
        <Link href="/repositories/connect" className="text-primary underline">
          Start over
        </Link>
        .
      </div>
    );
  }

  return (
    <div className="relative overflow-hidden">
      <div
        aria-hidden
        className="pointer-events-none absolute inset-0"
        style={{ background: "radial-gradient(ellipse 50% 40% at 50% -10%, rgba(203,166,247,0.2), transparent 70%)" }}
      />
      <div className="relative mx-auto w-full max-w-[680px] px-6 py-10">
        <Link href="/repositories/connect" className="mb-6 inline-flex items-center gap-1.5 text-section text-ink-400 hover:text-ink-100">
          <ArrowLeft className="size-3.5" />
          Connect
        </Link>

        <StepIndicator
          steps={[
            { label: "Method", status: "done" },
            { label: "Configure", status: "active" },
            { label: "Index", status: "pending" },
          ]}
        />

        <h1 className="text-[clamp(30px,3.4vw,42px)] font-extrabold leading-[1.05] tracking-[-0.04em] text-ink-100">Select repositories to index</h1>
        <p className="mt-2.5 text-[16px] text-ink-300">Choose which repositories from this installation to connect.</p>

        <div className="mt-6">
          <InstallationRepoPicker installationId={installationId} />
        </div>
      </div>
    </div>
  );
}

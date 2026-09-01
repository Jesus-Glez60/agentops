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

      <div className="mt-6">
        <InstallationRepoPicker installationId={installationId} />
      </div>
    </div>
  );
}

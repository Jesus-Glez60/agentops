"use client";

import { useState } from "react";
import Link from "next/link";
import { useParams } from "next/navigation";
import useSWR from "swr";
import { toast } from "sonner";
import { CheckCircle2, CircleDashed, GitBranch, Loader2, TriangleAlert, RotateCcw, KeyRound } from "lucide-react";
import { getIndexingStatus, retryIndexing, regenerateDeployKey, INDEXING_STAGE_LABELS, type IndexingStage } from "@/lib/api/repos-api";
import { Progress } from "@/components/ui/progress";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

/** Static, stage-keyed troubleshooting hints for the failure screen -- not server-driven, a small local lookup mirroring the mockup's own static copy per failure stage. */
const TROUBLESHOOTING: Record<string, string[]> = {
  connection_verified: ["Confirm the connection's credentials are still valid.", "For SSH: verify the deploy key hasn't been removed from GitHub."],
  repository_cloned: [
    "Verify the deploy key is still present in GitHub → Settings → Deploy keys.",
    "Confirm the key has not been revoked or regenerated since last use.",
    "Check that the repository URL is correct.",
    "If the key was removed, regenerate it and retry.",
  ],
  files_discovered: ["Confirm the repository has at least one file checked out on its default branch."],
  symbols_extracted: ["This is usually a transient scan error -- retry indexing."],
  dependencies_mapped: ["This is usually a transient scan error -- retry indexing."],
  knowledge_nodes_created: ["This is usually a transient scan error -- retry indexing."],
  embeddings_generated: ["Confirm semantic search (Qdrant) is configured for this deployment, or retry indexing."],
  documentation_generated: ["Retry indexing -- documentation generation failures don't usually recur."],
  index_ready: ["Retry indexing."],
};

function StageIcon({ status }: { status: IndexingStage["status"] }) {
  if (status === "done") return <CheckCircle2 className="size-full text-canvas" />;
  if (status === "active") return <Loader2 className="size-3.5 animate-spin text-primary" />;
  if (status === "failed") return <TriangleAlert className="size-3.5 text-canvas" />;
  return <CircleDashed className="size-3 text-ink-600" />;
}

export default function IndexingStatusPage() {
  const { connectionId } = useParams<{ connectionId: string }>();
  const [retrying, setRetrying] = useState(false);
  const [regenerating, setRegenerating] = useState(false);

  const { data, error } = useSWR(
    ["index-status", connectionId],
    () => getIndexingStatus(connectionId),
    { refreshInterval: (latest) => (latest && latest.job.status !== "running" ? 0 : 2000) },
  );

  async function handleRetry() {
    if (!data) return;
    setRetrying(true);
    try {
      await retryIndexing(connectionId, data.job.id);
      toast.success("Retrying indexing.");
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Couldn't retry indexing. Please try again.");
    } finally {
      setRetrying(false);
    }
  }

  async function handleRegenerateKey() {
    setRegenerating(true);
    try {
      await regenerateDeployKey(connectionId);
      toast.success("A fresh deploy key was generated -- add it to GitHub, then retry indexing.");
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Couldn't regenerate a deploy key for this connection (only SSH-method connections have one to regenerate).");
    } finally {
      setRegenerating(false);
    }
  }

  if (error) {
    return (
      <div className="mx-auto w-full max-w-[680px] px-6 py-10 text-section text-ink-400">
        No indexing job found for this repository yet. <Link href="/repositories" className="text-primary underline">Back to repositories</Link>.
      </div>
    );
  }
  if (!data) {
    return <div className="mx-auto w-full max-w-[680px] px-6 py-10 text-section text-ink-500">Loading…</div>;
  }

  const { job, stages, overall_percent } = data;
  const failedStage = stages.find((s) => s.status === "failed");
  const doneCount = stages.filter((s) => s.status === "done").length;

  return (
    <div className="mx-auto w-full max-w-[680px] px-6 py-10">
      <div className="mb-6 flex items-center gap-3">
        <div className="flex size-10 shrink-0 items-center justify-center rounded-lg border border-border-strong bg-panel">
          <GitBranch className={cn("size-5", job.status === "failed" ? "text-health-failed" : "text-primary")} />
        </div>
        <div>
          <h1 className="font-mono text-[16px] font-semibold text-ink-100">{connectionId}</h1>
          <p className="font-mono text-mono-code text-ink-400">
            {job.kind === "initial" ? "Initial index" : "Reindex"} &middot; {job.status}
          </p>
        </div>
        {job.status === "running" && (
          <div className="ml-auto text-right">
            <div className="text-section font-medium text-primary">
              {doneCount} / {stages.length} stages
            </div>
          </div>
        )}
      </div>

      {job.status === "failed" && failedStage && (
        <div className="mb-6 overflow-hidden rounded-lg border border-health-failed/40 bg-health-failed/5">
          <div className="flex items-start gap-3 px-4 py-3">
            <TriangleAlert className="mt-0.5 size-5 shrink-0 text-health-failed" />
            <div>
              <h2 className="mb-1 text-[14px] font-semibold text-health-failed">Indexing failed at: {INDEXING_STAGE_LABELS[failedStage.stage] ?? failedStage.stage}</h2>
              <p className="text-section leading-relaxed text-health-failed/80">{failedStage.error}</p>
            </div>
          </div>
        </div>
      )}

      {job.status !== "failed" && (
        <div className="mb-6">
          <Progress value={overall_percent} />
          <div className="mt-1.5 flex items-center justify-between text-mono-code text-ink-500">
            <span>{overall_percent}% complete</span>
          </div>
        </div>
      )}

      <div className="mb-6 overflow-hidden rounded-lg border border-border-strong bg-panel">
        <div className="border-b border-border-strong px-4 py-2.5 text-mono-code uppercase tracking-wider text-ink-400">Indexing stages</div>
        <div className="space-y-0 p-4">
          {stages.map((stage, i) => (
            <div key={stage.stage}>
              <div className="flex items-start gap-3 py-2">
                <div
                  className={cn(
                    "flex size-[26px] shrink-0 items-center justify-center rounded-full",
                    stage.status === "done" && "bg-health-healthy",
                    stage.status === "active" && "border-2 border-primary bg-primary/15",
                    stage.status === "failed" && "bg-health-failed",
                    stage.status === "pending" && "border border-border-strong bg-canvas",
                  )}
                >
                  <StageIcon status={stage.status} />
                </div>
                <div className="flex-1 pt-0.5">
                  <div className="flex items-center justify-between">
                    <span className={cn("text-section", stage.status === "pending" ? "text-ink-500" : "text-ink-100 font-medium")}>{INDEXING_STAGE_LABELS[stage.stage] ?? stage.stage}</span>
                    <span
                      className={cn(
                        "font-mono text-mono-code",
                        stage.status === "done" && "text-health-healthy",
                        stage.status === "active" && "text-primary",
                        stage.status === "failed" && "text-health-failed",
                        stage.status === "pending" && "text-ink-500",
                      )}
                    >
                      {stage.status === "done" && stage.progress_total ? `${stage.progress_total}` : stage.status}
                    </span>
                  </div>
                  {stage.status === "active" && stage.progress_total ? (
                    <div className="mt-1.5">
                      <Progress value={((stage.progress_current ?? 0) / stage.progress_total) * 100} />
                    </div>
                  ) : null}
                </div>
              </div>
              {i < stages.length - 1 && <div className="ml-[13px] h-[18px] w-px bg-border-strong" />}
            </div>
          ))}
        </div>
      </div>

      {job.status === "failed" && failedStage && (
        <>
          <div className="mb-6 rounded-md border border-border-strong bg-panel px-4 py-3.5">
            <h3 className="mb-3 text-section font-semibold text-ink-200">Troubleshooting steps</h3>
            <ol className="list-decimal space-y-2 pl-5 text-section text-ink-400">
              {(TROUBLESHOOTING[failedStage.stage] ?? ["Retry indexing."]).map((step) => (
                <li key={step}>{step}</li>
              ))}
            </ol>
          </div>

          <div className="flex items-center gap-3">
            {failedStage.stage === "repository_cloned" && (
              <Button onClick={handleRegenerateKey} disabled={regenerating} variant="outline">
                <KeyRound className="size-3.5" />
                {regenerating ? "Regenerating…" : "Regenerate deploy key"}
              </Button>
            )}
            <Button onClick={handleRetry} disabled={retrying} variant="outline">
              <RotateCcw className="size-3.5" />
              {retrying ? "Retrying…" : "Retry indexing"}
            </Button>
            <Button variant="outline" asChild>
              <Link href="/repositories">Manage connection</Link>
            </Button>
          </div>
        </>
      )}

      {job.status === "succeeded" && (
        <div className="flex items-center gap-3">
          <Button asChild>
            <Link href="/repositories">Back to repositories</Link>
          </Button>
        </div>
      )}
    </div>
  );
}

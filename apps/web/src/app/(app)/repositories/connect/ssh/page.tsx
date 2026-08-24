"use client";

import { useState } from "react";
import { useRouter } from "next/navigation";
import Link from "next/link";
import { toast } from "sonner";
import { ArrowLeft, Clock, Copy, Check, ShieldCheck } from "lucide-react";
import { connectRepo, verifyRepo, type ConnectRepoResponse } from "@/lib/api/repos-api";
import { StepIndicator } from "@/components/repositories/connect-wizard/step-indicator";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";

/** Best-effort slug from the last path segment of a git URL (SSH or HTTPS), stripped of `.git` -- mirrors `connect-repository-dialog.tsx`'s own helper, the backend just needs a stable identifier for this (tenant, repo) pair. */
function repoIdFromUrl(url: string): string {
  const last = url.replace(/\/+$/, "").split(/[/:]/).pop() ?? url;
  return last.replace(/\.git$/, "") || "repo";
}

export default function SshDeployKeyPage() {
  const router = useRouter();
  const [repoUrl, setRepoUrl] = useState("");
  const [connecting, setConnecting] = useState(false);
  const [verifying, setVerifying] = useState(false);
  const [result, setResult] = useState<ConnectRepoResponse | null>(null);
  const [copied, setCopied] = useState(false);

  async function handleConnect(e: React.FormEvent) {
    e.preventDefault();
    const trimmed = repoUrl.trim();
    if (!trimmed) return;
    setConnecting(true);
    try {
      const connected = await connectRepo({ repo_id: repoIdFromUrl(trimmed), repo_url: trimmed });
      setResult(connected);
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Couldn't generate a deploy key for that repository. Please try again.");
    } finally {
      setConnecting(false);
    }
  }

  async function handleCopy() {
    const key = result?.connection.public_key_openssh;
    if (!key) return;
    await navigator.clipboard.writeText(key);
    setCopied(true);
  }

  async function handleVerify() {
    if (!result) return;
    setVerifying(true);
    try {
      const outcome = await verifyRepo(result.connection.id);
      if (outcome.status === "failed") {
        toast.error(outcome.reason ?? "Verification failed. Confirm the deploy key was added, then try again.");
        return;
      }
      router.push(`/repositories/${encodeURIComponent(result.connection.id)}/index`);
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Verification failed. Please try again.");
    } finally {
      setVerifying(false);
    }
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
          { label: "Deploy key", status: result ? "done" : "active" },
          { label: "Repository URL", status: result ? "active" : "pending" },
          { label: "Verify & index", status: "pending" },
        ]}
      />

      <h1 className="text-page-title font-semibold text-ink-100">SSH deploy key</h1>
      <p className="mt-1 text-section text-ink-400">
        {result ? "A unique read-only SSH deploy key has been generated for this repository. Add it to GitHub to grant AgentOps access." : "Enter the repository's SSH URL to generate a dedicated, read-only deploy key."}
      </p>

      {!result ? (
        <form onSubmit={handleConnect} className="mt-6">
          <label className="mb-1.5 block text-mono-code uppercase text-ink-500">Repository SSH URL</label>
          <Input value={repoUrl} onChange={(e) => setRepoUrl(e.target.value)} placeholder="git@github.com:acme/widgets.git" required autoFocus />
          <p className="mt-1.5 text-mono-code text-ink-500">Use the SSH URL format: git@github.com:owner/repo.git</p>
          <div className="mt-6">
            <Button type="submit" disabled={connecting || !repoUrl.trim()}>
              {connecting ? "Generating key…" : "Generate deploy key"}
            </Button>
          </div>
        </form>
      ) : (
        <>
          <div className="mt-6">
            <label className="mb-2 block text-section font-medium text-ink-200">1. Copy the public SSH key</label>
            <div className="overflow-hidden rounded-md border border-border-strong">
              <div className="flex items-center justify-between border-b border-border-strong bg-canvas px-3 py-1.5">
                <span className="text-mono-code text-ink-500">id_ed25519.pub &middot; read-only</span>
                <button type="button" onClick={handleCopy} className="flex items-center gap-1.5 text-mono-code text-ink-400 hover:text-ink-100">
                  {copied ? <Check className="size-3.5" /> : <Copy className="size-3.5" />}
                  {copied ? "Copied" : "Copy"}
                </button>
              </div>
              <pre className="whitespace-pre-wrap break-all bg-canvas px-4 py-3 text-mono-code leading-relaxed text-ink-300">{result.connection.public_key_openssh}</pre>
            </div>
            <p className="mt-1.5 flex items-center gap-1.5 text-mono-code text-ink-500">
              <ShieldCheck className="size-3.5" />
              This key is restricted to read-only access and is unique to this repository.
            </p>
          </div>

          <div className="mt-5 rounded-md border border-border-strong bg-panel px-4 py-3.5">
            <label className="mb-3 block text-section font-medium text-ink-200">2. Add the key to your GitHub repository</label>
            <ol className="list-decimal space-y-1.5 pl-5 text-section text-ink-400">
              <li>Go to your repository on GitHub</li>
              <li>
                Navigate to <span className="text-mono-code text-ink-200">Settings → Deploy keys</span>
              </li>
              <li>
                Click <span className="text-mono-code text-ink-200">Add deploy key</span>
              </li>
              <li>
                Paste the key above. Set <span className="text-mono-code text-ink-200">Allow write access</span> to <strong className="text-ink-100">off</strong>
              </li>
              <li>
                Click <span className="text-mono-code text-ink-200">Add key</span>
              </li>
            </ol>
          </div>

          <div className="mt-5 flex items-center gap-3 rounded-md border border-health-scanning/35 bg-health-scanning/5 px-3.5 py-3 text-section">
            <Clock className="size-4 shrink-0 text-health-scanning" />
            <div>
              <p className="font-medium text-health-scanning">Waiting for key to be added to GitHub</p>
              <p className="text-health-scanning/70">Once you&apos;ve added the key, click Verify access below.</p>
            </div>
          </div>

          <div className="mt-6 flex items-center gap-3">
            <Button onClick={handleVerify} disabled={verifying}>
              <ShieldCheck className="size-4" />
              {verifying ? "Verifying…" : "Verify access"}
            </Button>
            <Button variant="outline" onClick={() => setResult(null)}>
              Back
            </Button>
          </div>
        </>
      )}
    </div>
  );
}

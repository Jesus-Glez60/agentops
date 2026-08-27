"use client";

// Picks a local folder, detects (client-side, without uploading anything)
// whether it has a git remote, and shows the exact right next step for
// each of three cases -- the server can't reach the user's own machine, so
// this component's whole job is routing, not reimplementing any
// connection flow:
//
//   1. No remote (or the picker isn't usable here at all) -- this repo can
//      never be server-indexed or team-shared; show the plain local
//      `agentops install` command.
//   2. Remote found, already connected -- show the `agentops connect
//      --remote` command with a freshly generated personal API key, same
//      pattern the onboarding checklist uses.
//   3. Remote found, not connected yet -- route to the existing SSH/GitHub
//      App flow (host-aware: GitHub App only offered for github.com), with
//      the detected URL/repo carried along so the user doesn't retype it.
import { useState } from "react";
import { useRouter } from "next/navigation";
import Link from "next/link";
import { toast } from "sonner";
import { ArrowLeft, FolderOpen } from "lucide-react";
import { getRepos } from "@/lib/api/repos-api";
import { createApiKey } from "@/lib/api/profile-api";
import { StepIndicator } from "@/components/repositories/connect-wizard/step-indicator";
import { CopyButton } from "@/components/shared/copy-button";
import { Button } from "@/components/ui/button";

const INSTALL_COMMAND = "agentops install";

/** `git@host:org/repo.git` or `https://host/org/repo(.git)` -- both formats
 * are already in active use elsewhere in this app (SSH page placeholder,
 * GitHub App's `clone_url`). A substring check like `url.includes("github.com")`
 * is a real bypass risk (false-positives on a lookalike host or an org/repo
 * path segment) so this actually extracts the host. */
function parseGitUrl(url: string): { host: string; orgRepo: string | null } | null {
  const trimmed = url.trim();
  const sshMatch = trimmed.match(/^[\w-]+@([^:]+):(.+?)(\.git)?$/);
  if (sshMatch) {
    return { host: sshMatch[1], orgRepo: sshMatch[2].replace(/\.git$/, "") };
  }
  try {
    const u = new URL(trimmed);
    const orgRepo = u.pathname.replace(/^\//, "").replace(/\.git$/, "");
    return { host: u.host, orgRepo: orgRepo || null };
  } catch {
    return null;
  }
}

/** Reads `.git/config` inside a picked folder and pulls the `[remote
 * "origin"]` url, if any -- format has been stable for git's entire
 * history, a small regex beats pulling in a full in-browser git
 * implementation (e.g. isomorphic-git) for one value. Any failure (no
 * `.git`, no origin remote, permission denied) just means "no remote",
 * not an error -- that's a real, expected case (a local-only repo), not
 * a bug. */
async function detectRemoteUrl(dirHandle: FileSystemDirectoryHandle): Promise<string | null> {
  try {
    const gitDir = await dirHandle.getDirectoryHandle(".git");
    const configFile = await gitDir.getFileHandle("config");
    const file = await configFile.getFile();
    const content = await file.text();
    const match = content.match(/\[remote "origin"\][^[]*?url\s*=\s*(\S+)/);
    return match ? match[1] : null;
  } catch {
    return null;
  }
}

type Outcome = { kind: "no-remote" } | { kind: "connected" } | { kind: "not-connected"; host: string; url: string; orgRepo: string | null };

export function LocalRepoClient({ apiUrl, apiUrlIsGuessed }: { apiUrl: string; apiUrlIsGuessed: boolean }) {
  const router = useRouter();
  const [picking, setPicking] = useState(false);
  const [outcome, setOutcome] = useState<Outcome | null>(null);
  const [apiKey, setApiKey] = useState<string | null>(null);
  const [generatingKey, setGeneratingKey] = useState(false);

  // Deliberately not computed during render -- `showDirectoryPicker`/secure
  // context are facts about the browser this code is actually running in,
  // not derivable from anything the server could know (unlike the API
  // URL, which is resolved server-side above). Rendering a neutral state
  // until this resolves avoids a server/client mismatch; picked once on
  // mount, doesn't change during the page's life.
  const [supported, setSupported] = useState<boolean | null>(null);
  if (supported === null && typeof window !== "undefined") {
    // Safe here specifically because the check is synchronous, has no
    // side effects beyond this component's own state, and only ever
    // narrows null -> a fixed boolean once -- not an effect-worthy
    // external subscription.
    setSupported("showDirectoryPicker" in window && window.isSecureContext);
  }

  async function handlePick() {
    setPicking(true);
    try {
      // showDirectoryPicker isn't in TS's default DOM lib yet.
      const dirHandle: FileSystemDirectoryHandle = await (window as unknown as { showDirectoryPicker: () => Promise<FileSystemDirectoryHandle> }).showDirectoryPicker();
      const remoteUrl = await detectRemoteUrl(dirHandle);
      if (!remoteUrl) {
        setOutcome({ kind: "no-remote" });
        return;
      }
      const parsed = parseGitUrl(remoteUrl);
      if (!parsed) {
        setOutcome({ kind: "no-remote" });
        return;
      }
      const { connections } = await getRepos();
      const existing = connections.find((c) => c.repo_url === remoteUrl);
      setOutcome(existing ? { kind: "connected" } : { kind: "not-connected", host: parsed.host, url: remoteUrl, orgRepo: parsed.orgRepo });
    } catch (err) {
      // AbortError = user closed the picker without choosing -- not a real error.
      if (err instanceof Error && err.name !== "AbortError") {
        toast.error("Couldn't read that folder. Please try again.");
      }
    } finally {
      setPicking(false);
    }
  }

  async function generateKey() {
    setGeneratingKey(true);
    try {
      const created = await createApiKey("Coding tool");
      setApiKey(created.key);
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Couldn't generate an API key. Please try again.");
    } finally {
      setGeneratingKey(false);
    }
  }

  function goToGithubApp() {
    if (outcome?.kind !== "not-connected" || !outcome.orgRepo) return;
    // See github-app/select/page.tsx's doc comment: a query param can't
    // survive the external GitHub OAuth redirect without new backend
    // plumbing, sessionStorage does (same tab, same origin).
    sessionStorage.setItem("agentops:local-connect:target-repo", outcome.orgRepo);
    router.push("/repositories/connect");
  }

  return (
    <div className="mx-auto w-full max-w-[680px] px-6 py-10">
      <Link href="/repositories/connect" className="mb-6 inline-flex items-center gap-1.5 text-section text-ink-400 hover:text-ink-100">
        <ArrowLeft className="size-3.5" />
        Connect
      </Link>

      <StepIndicator steps={[{ label: "Method", status: "done" }, { label: "Local folder", status: "active" }]} />

      <h1 className="text-page-title font-semibold text-ink-100">Connect a local repo</h1>
      <p className="mt-1 text-section text-ink-400">Point us at a folder on this machine and we&apos;ll tell you exactly what to run — including repos that aren&apos;t pushed anywhere.</p>

      {supported === false && (
        <div className="mt-6 rounded-md border border-border-strong bg-panel px-4 py-3.5 text-section text-ink-400">
          {typeof window !== "undefined" && !window.isSecureContext
            ? "Folder picking needs a secure connection (HTTPS, or localhost) -- this page is loaded over plain HTTP. You can still run the command below directly."
            : "Your browser doesn't support picking a local folder here (this needs Chrome, Edge, or another Chromium-based browser) -- you can still run the command below directly."}
        </div>
      )}

      {supported !== false && !outcome && (
        <div className="mt-6">
          <Button onClick={handlePick} disabled={picking}>
            <FolderOpen className="size-4" />
            {picking ? "Reading folder…" : "Choose a folder"}
          </Button>
          <p className="mt-1.5 text-mono-code text-ink-500">We only read .git/config, locally in your browser -- nothing is uploaded.</p>
        </div>
      )}

      {(outcome?.kind === "no-remote" || supported === false) && (
        <div className="mt-6 rounded-md border border-border-strong bg-panel px-4 py-3.5">
          <p className="mb-3 text-section text-ink-300">
            {outcome?.kind === "no-remote" ? "That folder has no git remote — " : ""}
            This repo can&apos;t be indexed by the server or shared with your team (there&apos;s no remote for it to reach). Run this on that machine instead:
          </p>
          <div className="flex items-center gap-2">
            <code className="flex-1 truncate rounded-md border border-border-strong bg-canvas px-3 py-2 text-mono-code text-ink-200">{INSTALL_COMMAND}</code>
            <CopyButton value={INSTALL_COMMAND} />
          </div>
        </div>
      )}

      {outcome?.kind === "connected" && (
        <div className="mt-6 rounded-md border border-border-strong bg-panel px-4 py-3.5">
          <p className="mb-3 text-section text-ink-300">This repo is already connected. Connect your coding tool to it:</p>
          {apiUrlIsGuessed && (
            <p className="mb-3 rounded-md border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-body text-amber-500">
              Couldn&apos;t confirm this server&apos;s public API address — guessed <code className="text-mono-code">{apiUrl}</code>. If wrong, set <code className="text-mono-code">AGENTOPS_PUBLIC_API_URL</code> and reload.
            </p>
          )}
          {apiKey === null ? (
            <Button size="sm" disabled={generatingKey} onClick={generateKey}>
              {generatingKey ? "Generating…" : "Generate API key"}
            </Button>
          ) : (
            <>
              <p className="mb-2 text-mono-code text-ink-500">Copy this now — it won&apos;t be shown again. Installs the CLI if it isn&apos;t already there.</p>
              <div className="flex items-center gap-2">
                <code className="flex-1 truncate rounded-md border border-border-strong bg-canvas px-3 py-2 text-mono-code text-ink-200">{`export AGENTOPS_API_KEY=${apiKey} && curl -fsSL ${apiUrl}/connect.sh | sh`}</code>
                <CopyButton value={`export AGENTOPS_API_KEY=${apiKey} && curl -fsSL ${apiUrl}/connect.sh | sh`} />
              </div>
              <a href={`${apiUrl}/connect.sh`} target="_blank" rel="noreferrer" className="mt-2 inline-block text-body text-ink-500 underline underline-offset-2 hover:text-ink-300">
                Preview the script before running it
              </a>
            </>
          )}
        </div>
      )}

      {outcome?.kind === "not-connected" && (
        <div className="mt-6 rounded-md border border-border-strong bg-panel px-4 py-3.5">
          <p className="mb-3 text-section text-ink-300">
            Found a remote (<span className="text-mono-code text-ink-200">{outcome.url}</span>) but it isn&apos;t connected yet — let&apos;s set that up so it can be indexed and shared with your team.
          </p>
          <div className="flex items-center gap-3">
            {outcome.host === "github.com" ? (
              <>
                <Button onClick={goToGithubApp}>Continue with GitHub App</Button>
                <Button variant="outline" onClick={() => router.push(`/repositories/connect/ssh?repo_url=${encodeURIComponent(outcome.url)}`)}>
                  Use SSH deploy key instead
                </Button>
              </>
            ) : (
              <Button onClick={() => router.push(`/repositories/connect/ssh?repo_url=${encodeURIComponent(outcome.url)}`)}>Continue with SSH deploy key</Button>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

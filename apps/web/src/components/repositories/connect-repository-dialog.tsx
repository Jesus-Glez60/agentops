"use client";

import { useState } from "react";
import useSWR, { useSWRConfig } from "swr";
import { toast } from "sonner";
import { Plus, Copy, Check } from "lucide-react";
import { connectRepo, getRepos, REPOS_SWR_KEY, type ConnectRepoResponse } from "@/lib/api/repos-api";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle, DialogTrigger } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";

/** Best-effort slug from the last path segment of a git URL (SSH or HTTPS), stripped of `.git` -- the backend just needs a stable identifier for this (tenant, repo) pair, not a real parse of every possible git URL shape. */
function repoIdFromUrl(url: string): string {
  const last = url.replace(/\/+$/, "").split(/[/:]/).pop() ?? url;
  return last.replace(/\.git$/, "") || "repo";
}

export function ConnectRepositoryDialog() {
  // Reads the same cached response `RepositoriesTable` fetches -- no extra
  // request, just needs `can_connect` to decide whether to render at all.
  const { data } = useSWR(REPOS_SWR_KEY, getRepos);
  const { mutate } = useSWRConfig();
  const [open, setOpen] = useState(false);
  const [repoUrl, setRepoUrl] = useState("");
  const [connecting, setConnecting] = useState(false);
  const [result, setResult] = useState<ConnectRepoResponse | null>(null);
  const [copied, setCopied] = useState(false);

  function handleOpenChange(next: boolean) {
    setOpen(next);
    if (!next) {
      setRepoUrl("");
      setResult(null);
      setCopied(false);
    }
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    const trimmed = repoUrl.trim();
    if (!trimmed) return;
    setConnecting(true);
    try {
      const connected = await connectRepo({ repo_id: repoIdFromUrl(trimmed), repo_url: trimmed });
      setResult(connected);
      await mutate(REPOS_SWR_KEY);
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Couldn't connect that repository. Please try again.");
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

  // `can_connect` is server-computed from the caller's own capabilities --
  // this component never re-derives that logic, it just hides the trigger
  // once the response says so. While `data` is still loading, render
  // nothing rather than flash the button and yank it away a moment later.
  if (data && !data.can_connect) return null;

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogTrigger asChild>
        <Button size="sm">
          <Plus className="size-3.5" />
          Connect repository
        </Button>
      </DialogTrigger>
      <DialogContent>
        {result ? (
          <>
            <DialogHeader>
              <DialogTitle>Deploy key generated</DialogTitle>
              <DialogDescription>Add this as a read-only Deploy Key on {result.connection.repo_url}, then verify the connection from the repository list.</DialogDescription>
            </DialogHeader>
            <div className="flex items-center gap-2">
              <code className="flex-1 truncate rounded-md border border-border-strong bg-panel px-3 py-2 text-mono-code text-ink-200">{result.connection.public_key_openssh}</code>
              <Button size="sm" variant="outline" onClick={handleCopy}>
                {copied ? <Check className="size-3.5" /> : <Copy className="size-3.5" />}
                {copied ? "Copied" : "Copy"}
              </Button>
            </div>
            <DialogFooter>
              <Button size="sm" onClick={() => handleOpenChange(false)}>
                Done
              </Button>
            </DialogFooter>
          </>
        ) : (
          <form onSubmit={handleSubmit}>
            <DialogHeader>
              <DialogTitle>Connect repository</DialogTitle>
              <DialogDescription>Generates a read-only SSH deploy key you&apos;ll add to the repo on GitHub, then verify the connection.</DialogDescription>
            </DialogHeader>
            <div className="py-4">
              <label className="mb-1.5 block text-mono-code uppercase text-ink-500">Repository URL</label>
              <Input value={repoUrl} onChange={(e) => setRepoUrl(e.target.value)} placeholder="git@github.com:acme/widgets.git" required autoFocus />
            </div>
            <DialogFooter>
              <Button type="submit" size="sm" disabled={connecting || !repoUrl.trim()}>
                {connecting ? "Connecting…" : "Connect"}
              </Button>
            </DialogFooter>
          </form>
        )}
      </DialogContent>
    </Dialog>
  );
}

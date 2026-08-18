"use client";

import { useState } from "react";
import { Copy, Check } from "lucide-react";
import type { RepoConnection } from "@/lib/api/repos-api";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";

/** Unlike an API key, `public_key_openssh` is a *public* key -- safe to keep showing on demand rather than a reveal-once flow, so this is just a plain view+copy dialog, opened from a repo row's "View key" action. */
export function ViewDeployKeyDialog({ repo, onOpenChange }: { repo: RepoConnection; onOpenChange: (open: boolean) => void }) {
  const [copied, setCopied] = useState(false);

  async function handleCopy() {
    if (!repo.public_key_openssh) return;
    await navigator.clipboard.writeText(repo.public_key_openssh);
    setCopied(true);
  }

  return (
    <Dialog open onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Deploy key</DialogTitle>
          <DialogDescription>Add this as a read-only Deploy Key on {repo.repo_url}.</DialogDescription>
        </DialogHeader>
        <div className="flex items-center gap-2">
          <code className="flex-1 truncate rounded-md border border-border-strong bg-panel px-3 py-2 text-mono-code text-ink-200">{repo.public_key_openssh}</code>
          <Button size="sm" variant="outline" onClick={handleCopy}>
            {copied ? <Check className="size-3.5" /> : <Copy className="size-3.5" />}
            {copied ? "Copied" : "Copy"}
          </Button>
        </div>
        <DialogFooter>
          <Button size="sm" onClick={() => onOpenChange(false)}>
            Done
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

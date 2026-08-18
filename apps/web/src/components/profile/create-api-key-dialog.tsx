"use client";

import { useState } from "react";
import { useSWRConfig } from "swr";
import { toast } from "sonner";
import { Plus, Copy, Check } from "lucide-react";
import { createApiKey, API_KEYS_SWR_KEY } from "@/lib/api/profile-api";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";

export function CreateApiKeyDialog() {
  const { mutate } = useSWRConfig();
  const [open, setOpen] = useState(false);
  const [name, setName] = useState("");
  const [creating, setCreating] = useState(false);
  const [rawKey, setRawKey] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  function handleOpenChange(next: boolean) {
    setOpen(next);
    if (!next) {
      // Reset only on close -- while open, closing must not lose an unsaved raw key mid-copy.
      setName("");
      setRawKey(null);
      setCopied(false);
    }
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!name.trim()) return;
    setCreating(true);
    try {
      const created = await createApiKey(name.trim());
      setRawKey(created.key);
      await mutate(API_KEYS_SWR_KEY);
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Couldn't generate a key. Please try again.");
    } finally {
      setCreating(false);
    }
  }

  async function handleCopy() {
    if (!rawKey) return;
    await navigator.clipboard.writeText(rawKey);
    setCopied(true);
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <Button size="sm" variant="outline" onClick={() => setOpen(true)}>
        <Plus className="size-3.5" />
        Generate new key
      </Button>
      <DialogContent>
        {rawKey ? (
          <>
            <DialogHeader>
              <DialogTitle>Key generated</DialogTitle>
              <DialogDescription>Copy it now — it won&apos;t be shown again.</DialogDescription>
            </DialogHeader>
            <div className="flex items-center gap-2">
              <code className="flex-1 truncate rounded-md border border-border-strong bg-panel px-3 py-2 text-mono-code text-ink-200">{rawKey}</code>
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
              <DialogTitle>Generate new API key</DialogTitle>
              <DialogDescription>API keys grant full read access to your indexed repositories. Never share them publicly.</DialogDescription>
            </DialogHeader>
            <div className="py-4">
              <label className="mb-1.5 block text-mono-code uppercase text-ink-500">Name</label>
              <Input value={name} onChange={(e) => setName(e.target.value)} placeholder="CI / CD Pipeline" required autoFocus />
            </div>
            <DialogFooter>
              <Button type="submit" size="sm" disabled={creating || !name.trim()}>
                {creating ? "Generating…" : "Generate key"}
              </Button>
            </DialogFooter>
          </form>
        )}
      </DialogContent>
    </Dialog>
  );
}

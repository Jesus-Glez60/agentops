"use client";

import { useState } from "react";
import useSWR, { useSWRConfig } from "swr";
import { toast } from "sonner";
import { UserPlus, Copy, Check } from "lucide-react";
import { createTeamInvite, inviteUrl, TEAM_INVITES_SWR_KEY, TEAM_ROLES_SWR_KEY, getTeamRoles } from "@/lib/api/team-api";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";

const ROLE_OPTIONS: { value: string; label: string }[] = [
  { value: "member", label: "Member" },
  { value: "admin", label: "Admin" },
  { value: "viewer", label: "Viewer" },
  { value: "billing", label: "Billing" },
];

export function InviteMemberDialog() {
  const { mutate } = useSWRConfig();
  // Custom roles are assignable at invite time too, not just via an
  // existing member's role dropdown -- same data `RolesTab` already fetches.
  const { data: rolesData } = useSWR(TEAM_ROLES_SWR_KEY, getTeamRoles);
  const roleOptions = [...ROLE_OPTIONS, ...(rolesData?.custom_roles.map((r) => ({ value: r.role_key, label: r.label })) ?? [])];
  const [open, setOpen] = useState(false);
  const [email, setEmail] = useState("");
  const [role, setRole] = useState<string>("member");
  const [note, setNote] = useState("");
  const [creating, setCreating] = useState(false);
  const [link, setLink] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  function handleOpenChange(next: boolean) {
    setOpen(next);
    if (!next) {
      setEmail("");
      setRole("member");
      setNote("");
      setLink(null);
      setCopied(false);
    }
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!email.trim()) return;
    setCreating(true);
    try {
      const invite = await createTeamInvite({ email: email.trim(), role, note: note.trim() || undefined });
      setLink(inviteUrl(invite.token));
      await mutate(TEAM_INVITES_SWR_KEY);
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Couldn't send that invite. Please try again.");
    } finally {
      setCreating(false);
    }
  }

  async function handleCopy() {
    if (!link) return;
    await navigator.clipboard.writeText(link);
    setCopied(true);
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <Button size="sm" onClick={() => setOpen(true)}>
        <UserPlus className="size-3.5" />
        Invite member
      </Button>
      <DialogContent>
        {link ? (
          <>
            <DialogHeader>
              <DialogTitle>Invite created</DialogTitle>
              <DialogDescription>Email delivery isn&apos;t set up yet — share this link with them directly.</DialogDescription>
            </DialogHeader>
            <div className="flex items-center gap-2">
              <code className="flex-1 truncate rounded-md border border-border-strong bg-panel px-3 py-2 text-mono-code text-ink-200">{link}</code>
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
              <DialogTitle>Invite team member</DialogTitle>
            </DialogHeader>
            <div className="space-y-3 py-4">
              <div>
                <label className="mb-1.5 block text-mono-code uppercase text-ink-500">Email address</label>
                <Input type="email" value={email} onChange={(e) => setEmail(e.target.value)} placeholder="name@company.com" autoFocus required />
              </div>
              <div>
                <label className="mb-1.5 block text-mono-code uppercase text-ink-500">Role</label>
                <Select value={role} onValueChange={setRole}>
                  <SelectTrigger className="w-full">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {roleOptions.map((r) => (
                      <SelectItem key={r.value} value={r.value}>
                        {r.label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
              <div>
                <label className="mb-1.5 block text-mono-code uppercase text-ink-500">
                  Personal note <span className="normal-case text-ink-500">(optional)</span>
                </label>
                <Input value={note} onChange={(e) => setNote(e.target.value)} placeholder="Add a welcome message…" />
              </div>
            </div>
            <DialogFooter>
              <Button type="submit" size="sm" disabled={creating || !email.trim()}>
                {creating ? "Creating…" : "Create invite"}
              </Button>
            </DialogFooter>
          </form>
        )}
      </DialogContent>
    </Dialog>
  );
}

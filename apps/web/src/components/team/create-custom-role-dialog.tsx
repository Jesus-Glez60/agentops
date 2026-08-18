"use client";

import { useState } from "react";
import { useSWRConfig } from "swr";
import { toast } from "sonner";
import { Plus } from "lucide-react";
import { createCustomRole, TEAM_ROLES_SWR_KEY, type Capability, type MemberRole } from "@/lib/api/team-api";
import { CapabilityChecklist } from "@/components/team/capability-checklist";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle, DialogTrigger } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";

const BASE_ROLES: MemberRole[] = ["admin", "member", "viewer", "billing"];
const BASE_ROLE_LABELS: Record<MemberRole, string> = { admin: "Admin", member: "Member", viewer: "Viewer", billing: "Billing" };

export function CreateCustomRoleDialog({ matrix }: { matrix: Capability[] }) {
  const { mutate } = useSWRConfig();
  const [open, setOpen] = useState(false);
  const [label, setLabel] = useState("");
  const [clonedFrom, setClonedFrom] = useState<MemberRole>("member");
  const [checked, setChecked] = useState<Set<string>>(() => new Set(matrix.filter((c) => c.allowed_roles.includes("member")).map((c) => c.key)));
  const [creating, setCreating] = useState(false);

  function handleOpenChange(next: boolean) {
    setOpen(next);
    if (next) {
      // Re-seed from the current "clone from" selection every time the
      // dialog opens, so a previous session's edits don't linger.
      setChecked(new Set(matrix.filter((c) => c.allowed_roles.includes(clonedFrom)).map((c) => c.key)));
    } else {
      setLabel("");
    }
  }

  function handleCloneFromChange(role: MemberRole) {
    setClonedFrom(role);
    setChecked(new Set(matrix.filter((c) => c.allowed_roles.includes(role)).map((c) => c.key)));
  }

  function toggleCapability(key: string) {
    setChecked((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    const trimmed = label.trim();
    if (!trimmed) return;
    setCreating(true);
    try {
      await createCustomRole({ label: trimmed, cloned_from: clonedFrom, capabilities: [...checked] });
      await mutate(TEAM_ROLES_SWR_KEY);
      toast.success(`Custom role "${trimmed}" created`);
      handleOpenChange(false);
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Couldn't create that role. Please try again.");
    } finally {
      setCreating(false);
    }
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogTrigger asChild>
        <Button size="sm">
          <Plus className="size-3.5" />
          Create custom role
        </Button>
      </DialogTrigger>
      <DialogContent>
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>Create custom role</DialogTitle>
            <DialogDescription>Clone a base role, then toggle individual capabilities.</DialogDescription>
          </DialogHeader>
          <div className="space-y-3 py-4">
            <div>
              <label className="mb-1.5 block text-mono-code uppercase text-ink-500">Name</label>
              <Input value={label} onChange={(e) => setLabel(e.target.value)} placeholder="e.g. Junior Developer" autoFocus />
            </div>
            <div>
              <label className="mb-1.5 block text-mono-code uppercase text-ink-500">Clone from</label>
              <Select value={clonedFrom} onValueChange={(v) => handleCloneFromChange(v as MemberRole)}>
                <SelectTrigger className="w-full">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {BASE_ROLES.map((role) => (
                    <SelectItem key={role} value={role}>
                      {BASE_ROLE_LABELS[role]}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
            <div>
              <label className="mb-1.5 block text-mono-code uppercase text-ink-500">Capabilities</label>
              <CapabilityChecklist matrix={matrix} checked={checked} onToggle={toggleCapability} />
            </div>
          </div>
          <DialogFooter>
            <Button type="submit" size="sm" disabled={creating || !label.trim()}>
              {creating ? "Creating…" : "Create role"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

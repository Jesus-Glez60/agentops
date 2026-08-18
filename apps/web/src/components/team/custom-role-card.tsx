"use client";

import { useState } from "react";
import { useSWRConfig } from "swr";
import { toast } from "sonner";
import { Shield } from "lucide-react";
import { updateCustomRoleCapabilities, deleteCustomRole, TEAM_ROLES_SWR_KEY, type Capability, type CustomRole } from "@/lib/api/team-api";
import { CapabilityChecklist } from "@/components/team/capability-checklist";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle, DialogTrigger } from "@/components/ui/dialog";

export function CustomRoleCard({ role, matrix, canManage }: { role: CustomRole; matrix: Capability[]; canManage: boolean }) {
  const { mutate } = useSWRConfig();
  const [editOpen, setEditOpen] = useState(false);
  const [checked, setChecked] = useState<Set<string>>(() => new Set(role.capabilities));
  const [saving, setSaving] = useState(false);
  const [deleting, setDeleting] = useState(false);

  function handleEditOpenChange(next: boolean) {
    setEditOpen(next);
    if (next) setChecked(new Set(role.capabilities));
  }

  function toggleCapability(key: string) {
    setChecked((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  }

  async function handleSave() {
    setSaving(true);
    try {
      await updateCustomRoleCapabilities(role.role_key, [...checked]);
      await mutate(TEAM_ROLES_SWR_KEY);
      toast.success("Capabilities updated");
      setEditOpen(false);
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Couldn't update capabilities. Please try again.");
    } finally {
      setSaving(false);
    }
  }

  async function handleDelete() {
    setDeleting(true);
    try {
      await deleteCustomRole(role.role_key);
      await mutate(TEAM_ROLES_SWR_KEY);
      toast.success(`"${role.label}" deleted`);
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Couldn't delete that role — it may still be assigned to a member.");
    } finally {
      setDeleting(false);
    }
  }

  return (
    <Card size="sm">
      <CardContent className="space-y-2">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-2">
            <div className="flex size-6 items-center justify-center rounded-md border border-border-strong bg-panel">
              <Shield className="size-3.5 text-ink-400" />
            </div>
            <span className="text-body font-semibold text-ink-100">{role.label}</span>
          </div>
          <span className="text-mono-code text-ink-500">{role.capabilities.length} capabilities</span>
        </div>
        <p className="text-section text-ink-400">Cloned from {role.cloned_from}</p>
        {canManage && (
          <div className="flex gap-1.5 pt-1">
            <Dialog open={editOpen} onOpenChange={handleEditOpenChange}>
              <DialogTrigger asChild>
                <Button variant="outline" size="sm">
                  Edit capabilities
                </Button>
              </DialogTrigger>
              <DialogContent>
                <DialogHeader>
                  <DialogTitle>Edit &quot;{role.label}&quot;</DialogTitle>
                  <DialogDescription>Toggle which capabilities this custom role grants.</DialogDescription>
                </DialogHeader>
                <div className="py-4">
                  <CapabilityChecklist matrix={matrix} checked={checked} onToggle={toggleCapability} />
                </div>
                <DialogFooter>
                  <Button size="sm" onClick={handleSave} disabled={saving}>
                    {saving ? "Saving…" : "Save"}
                  </Button>
                </DialogFooter>
              </DialogContent>
            </Dialog>
            <Button variant="outline" size="sm" onClick={handleDelete} disabled={deleting}>
              {deleting ? "Deleting…" : "Delete"}
            </Button>
          </div>
        )}
      </CardContent>
    </Card>
  );
}

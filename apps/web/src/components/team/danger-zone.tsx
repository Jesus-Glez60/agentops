"use client";

import { useState } from "react";
import useSWR from "swr";
import { toast } from "sonner";
import { TEAM_SWR_KEY, TEAM_MEMBERS_SWR_KEY, getTeam, getTeamMembers, transferOwnership, deleteOrganization, type TeamMember } from "@/lib/api/team-api";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle, DialogTrigger } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";

/** Owner-only section at the bottom of the Members tab -- `team.is_owner`
 * is the same server-computed field the Team Management "Integrations" tab
 * gates on, so this component never re-derives that check either. */
export function DangerZone() {
  const { data: team } = useSWR(TEAM_SWR_KEY, getTeam);
  const { data: members } = useSWR(TEAM_MEMBERS_SWR_KEY, getTeamMembers);

  if (!team?.is_owner) return null;

  const otherActiveAdmins = (members ?? []).filter((m) => m.role === "admin" && m.status === "active" && !m.is_you);

  return (
    <Card className="mt-6 border-destructive/30">
      <CardHeader className="border-b border-border-strong pb-4">
        <CardTitle className="text-destructive">Danger Zone</CardTitle>
      </CardHeader>
      <CardContent className="divide-y divide-border-strong p-0">
        <TransferOwnershipRow admins={otherActiveAdmins} />
        <DeleteOrganizationRow tenant={team.tenant} />
      </CardContent>
    </Card>
  );
}

function TransferOwnershipRow({ admins }: { admins: TeamMember[] }) {
  const [open, setOpen] = useState(false);
  const [toUserId, setToUserId] = useState<string>("");
  const [transferring, setTransferring] = useState(false);

  async function handleTransfer() {
    if (!toUserId) return;
    setTransferring(true);
    try {
      await transferOwnership(Number(toUserId));
      toast.success("Ownership transferred");
      // Nearly everything on this page is Owner-gated -- reload rather
      // than patch a dozen SWR caches individually.
      window.location.reload();
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Couldn't transfer ownership. Please try again.");
      setTransferring(false);
    }
  }

  return (
    <div className="flex items-center justify-between gap-4 px-6 py-4">
      <div className="min-w-0">
        <p className="text-body font-medium text-ink-100">Transfer Ownership</p>
        <p className="text-mono-code text-ink-500">Hand off Owner-only controls (billing, org deletion, admin management) to another admin.</p>
      </div>
      <Dialog
        open={open}
        onOpenChange={(next) => {
          setOpen(next);
          if (!next) setToUserId("");
        }}
      >
        <DialogTrigger asChild>
          <Button variant="outline" size="sm" disabled={admins.length === 0} className="shrink-0">
            Transfer
          </Button>
        </DialogTrigger>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Transfer Ownership</DialogTitle>
            <DialogDescription>Pick an active admin to become the new Owner. You&apos;ll keep your admin access, but lose Owner-only controls.</DialogDescription>
          </DialogHeader>
          <div className="py-4">
            <Select value={toUserId} onValueChange={setToUserId}>
              <SelectTrigger className="w-full">
                <SelectValue placeholder="Choose an admin" />
              </SelectTrigger>
              <SelectContent>
                {admins.map((admin) => (
                  <SelectItem key={admin.user_id} value={String(admin.user_id)}>
                    {admin.first_name} {admin.last_name} ({admin.email})
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          <DialogFooter>
            <Button size="sm" onClick={handleTransfer} disabled={!toUserId || transferring}>
              {transferring ? "Transferring…" : "Transfer ownership"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}

function DeleteOrganizationRow({ tenant }: { tenant: string }) {
  const [open, setOpen] = useState(false);
  const [confirmText, setConfirmText] = useState("");
  const [deleting, setDeleting] = useState(false);

  async function handleDelete() {
    if (confirmText !== tenant) return;
    setDeleting(true);
    try {
      await deleteOrganization(confirmText);
      toast.success("Organization deleted");
      // The session's tenant has changed server-side -- every tenant-scoped
      // cache in the app is now stale, a full navigation is the only
      // correct way to land in the fresh org.
      window.location.href = "/";
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Couldn't delete the organization. Please try again.");
      setDeleting(false);
    }
  }

  return (
    <div className="flex items-center justify-between gap-4 px-6 py-4">
      <div className="min-w-0">
        <p className="text-body font-medium text-ink-100">Delete Organization</p>
        <p className="text-mono-code text-ink-500">Permanently deletes every repository connection, integration credential, and team member. This cannot be undone.</p>
      </div>
      <Dialog
        open={open}
        onOpenChange={(next) => {
          setOpen(next);
          if (!next) setConfirmText("");
        }}
      >
        <DialogTrigger asChild>
          <Button variant="destructive" size="sm" className="shrink-0">
            Delete
          </Button>
        </DialogTrigger>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Delete Organization</DialogTitle>
            <DialogDescription>
              This permanently deletes every repository connection, org-wide and personal integration credential, member, invite, and custom role for this organization. This cannot be undone.
            </DialogDescription>
          </DialogHeader>
          <div className="py-4">
            <label className="mb-1.5 block text-mono-code uppercase text-ink-500">
              Type <span className="text-ink-200">{tenant}</span> to confirm
            </label>
            <Input value={confirmText} onChange={(e) => setConfirmText(e.target.value)} placeholder={tenant} autoFocus />
          </div>
          <DialogFooter>
            <Button variant="destructive" size="sm" onClick={handleDelete} disabled={confirmText !== tenant || deleting}>
              {deleting ? "Deleting…" : "Permanently delete"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}

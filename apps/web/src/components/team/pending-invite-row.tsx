"use client";

import { toast } from "sonner";
import { Mail } from "lucide-react";
import type { TeamInvite, CustomRole } from "@/lib/api/team-api";
import { resendTeamInvite, cancelTeamInvite, inviteUrl, roleLabel } from "@/lib/api/team-api";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { TableCell, TableRow } from "@/components/ui/table";
import { relativeTimeFromIsoString } from "@/lib/relative-time";

export function PendingInviteRow({ invite, canManage, customRoles, onChanged }: { invite: TeamInvite; canManage: boolean; customRoles: CustomRole[]; onChanged: () => void }) {
  async function handleResend() {
    try {
      const { token } = await resendTeamInvite(invite.id);
      await navigator.clipboard.writeText(inviteUrl(token));
      onChanged();
      toast.success("New invite link copied to clipboard");
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Couldn't resend that invite. Please try again.");
    }
  }

  async function handleCancel() {
    try {
      await cancelTeamInvite(invite.id);
      onChanged();
      toast.success("Invite cancelled");
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Couldn't cancel that invite. Please try again.");
    }
  }

  return (
    <TableRow className="opacity-70">
      <TableCell>
        <div className="flex items-center gap-2.5">
          <div className="flex size-7 shrink-0 items-center justify-center rounded-full border border-border-strong bg-panel">
            <Mail className="size-3.5 text-ink-400" />
          </div>
          <div className="min-w-0">
            <div className="truncate text-body font-medium italic text-ink-400">Invited user</div>
            <div className="truncate text-mono-code text-ink-500">{invite.email}</div>
          </div>
        </div>
      </TableCell>
      <TableCell>
        <Badge variant="outline">{roleLabel(invite.role, customRoles)}</Badge>
      </TableCell>
      <TableCell>
        <Badge variant="outline" className="border-health-warning/40 text-health-warning">
          Pending
        </Badge>
      </TableCell>
      <TableCell className="text-mono-code text-ink-500">Sent {relativeTimeFromIsoString(invite.created_at)}</TableCell>
      {canManage && (
        <TableCell className="text-right">
          <div className="flex justify-end gap-1.5">
            <Button variant="outline" size="sm" onClick={handleResend}>
              Resend
            </Button>
            <Button variant="outline" size="sm" onClick={handleCancel}>
              Cancel
            </Button>
          </div>
        </TableCell>
      )}
    </TableRow>
  );
}

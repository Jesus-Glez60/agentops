"use client";

import { toast } from "sonner";
import type { TeamMember, MemberRole, MemberStatus, CustomRole } from "@/lib/api/team-api";
import { updateTeamMember, removeTeamMember, FIXED_ROLE_LABELS, roleLabel } from "@/lib/api/team-api";
import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { TableCell, TableRow } from "@/components/ui/table";
import { relativeTimeFromIsoString } from "@/lib/relative-time";

export function MemberRow({ member, canManage, customRoles, onChanged }: { member: TeamMember; canManage: boolean; customRoles: CustomRole[]; onChanged: () => void }) {
  async function handleRoleChange(role: string) {
    try {
      await updateTeamMember(member.user_id, { role });
      onChanged();
      toast.success("Role updated");
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Couldn't update role. Please try again.");
    }
  }

  async function handleStatusToggle() {
    const nextStatus: MemberStatus = member.status === "active" ? "suspended" : "active";
    try {
      await updateTeamMember(member.user_id, { status: nextStatus });
      onChanged();
      toast.success(nextStatus === "active" ? "Member reactivated" : "Member suspended");
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Couldn't update status. Please try again.");
    }
  }

  async function handleRemove() {
    try {
      await removeTeamMember(member.user_id);
      onChanged();
      toast.success("Member removed");
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Couldn't remove that member. Please try again.");
    }
  }

  return (
    <TableRow>
      <TableCell>
        <div className="flex items-center gap-2.5">
          <Avatar className="size-7 shrink-0">
            {member.avatar_url && <AvatarImage src={member.avatar_url} alt="" />}
            <AvatarFallback className="text-label">{member.first_name.charAt(0).toUpperCase()}</AvatarFallback>
          </Avatar>
          <div className="min-w-0">
            <div className="flex items-center gap-1.5 truncate text-body font-medium text-ink-100">
              {member.first_name} {member.last_name}
              {member.is_you && <span className="text-mono-code text-ink-500">(you)</span>}
            </div>
            <div className="truncate text-mono-code text-ink-500">{member.email}</div>
          </div>
        </div>
      </TableCell>
      <TableCell>
        {canManage ? (
          <Select defaultValue={member.role} onValueChange={handleRoleChange}>
            <SelectTrigger size="sm" className="w-36">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {(Object.keys(FIXED_ROLE_LABELS) as MemberRole[]).map((r) => (
                <SelectItem key={r} value={r}>
                  {FIXED_ROLE_LABELS[r]}
                </SelectItem>
              ))}
              {customRoles.map((r) => (
                <SelectItem key={r.role_key} value={r.role_key}>
                  {r.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        ) : (
          <Badge variant="outline">{roleLabel(member.role, customRoles)}</Badge>
        )}
      </TableCell>
      <TableCell>
        <Badge variant={member.status === "active" ? "outline" : "destructive"} className={member.status === "active" ? "border-health-healthy/40 text-health-healthy" : ""}>
          {member.status === "active" ? "Active" : "Suspended"}
        </Badge>
      </TableCell>
      <TableCell className="text-mono-code text-ink-500">{relativeTimeFromIsoString(member.joined_at)}</TableCell>
      {canManage && (
        <TableCell className="text-right">
          <div className="flex justify-end gap-1.5">
            <Button variant="outline" size="sm" onClick={handleStatusToggle}>
              {member.status === "active" ? "Suspend" : "Reactivate"}
            </Button>
            <Button variant="outline" size="sm" onClick={handleRemove}>
              Remove
            </Button>
          </div>
        </TableCell>
      )}
    </TableRow>
  );
}

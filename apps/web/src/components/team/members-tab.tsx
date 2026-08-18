"use client";

import useSWR from "swr";
import { TEAM_SWR_KEY, TEAM_MEMBERS_SWR_KEY, TEAM_INVITES_SWR_KEY, TEAM_ROLES_SWR_KEY, getTeam, getTeamMembers, getTeamInvites, getTeamRoles } from "@/lib/api/team-api";
import { MemberRow } from "@/components/team/member-row";
import { PendingInviteRow } from "@/components/team/pending-invite-row";
import { InviteMemberDialog } from "@/components/team/invite-member-dialog";
import { DangerZone } from "@/components/team/danger-zone";
import { Table, TableBody, TableHead, TableHeader, TableRow } from "@/components/ui/table";

export function MembersTab() {
  const { data: team } = useSWR(TEAM_SWR_KEY, getTeam);
  const { data: members, mutate: mutateMembers, isLoading } = useSWR(TEAM_MEMBERS_SWR_KEY, getTeamMembers);
  const canManage = team?.role === "admin";
  const { data: invites, mutate: mutateInvites } = useSWR(canManage ? TEAM_INVITES_SWR_KEY : null, getTeamInvites);
  const { data: rolesData } = useSWR(TEAM_ROLES_SWR_KEY, getTeamRoles);
  const customRoles = rolesData?.custom_roles ?? [];

  return (
    <div className="max-w-[900px]">
      <div className="mb-3 flex items-center justify-between">
        <p className="text-body text-ink-500">
          {members?.length ?? 0} member{members?.length === 1 ? "" : "s"}
        </p>
        {canManage && <InviteMemberDialog />}
      </div>
      <div className="rounded-lg border border-border-strong">
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Member</TableHead>
              <TableHead>Role</TableHead>
              <TableHead>Status</TableHead>
              <TableHead>Joined</TableHead>
              {canManage && <TableHead className="text-right">Actions</TableHead>}
            </TableRow>
          </TableHeader>
          <TableBody>
            {isLoading && (
              <TableRow>
                <td colSpan={5} className="px-4 py-6 text-center text-body text-ink-500">
                  Loading…
                </td>
              </TableRow>
            )}
            {(members ?? []).map((member) => (
              <MemberRow key={member.user_id} member={member} canManage={canManage} customRoles={customRoles} onChanged={() => mutateMembers()} />
            ))}
            {(invites ?? []).map((invite) => (
              <PendingInviteRow key={invite.id} invite={invite} canManage={canManage} customRoles={customRoles} onChanged={() => mutateInvites()} />
            ))}
          </TableBody>
        </Table>
      </div>
      <DangerZone />
    </div>
  );
}

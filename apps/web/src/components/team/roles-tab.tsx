"use client";

import useSWR from "swr";
import { TEAM_SWR_KEY, TEAM_ROLES_SWR_KEY, getTeam, getTeamRoles } from "@/lib/api/team-api";
import { RoleCard } from "@/components/team/role-card";
import { CustomRoleCard } from "@/components/team/custom-role-card";
import { CreateCustomRoleDialog } from "@/components/team/create-custom-role-dialog";
import { PermissionsMatrixTable } from "@/components/team/permissions-matrix-table";

export function RolesTab() {
  const { data: team } = useSWR(TEAM_SWR_KEY, getTeam);
  const { data } = useSWR(TEAM_ROLES_SWR_KEY, getTeamRoles);
  if (!data) return null;

  // `team.manage_roles` is admin-only on the backend -- custom-role
  // creation/edit/delete actions are hidden entirely for a non-admin
  // rather than shown and left to 403 on click.
  const canManageRoles = team?.role === "admin";

  return (
    <div className="space-y-5">
      <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-4">
        {data.roles.map((role) => (
          <RoleCard key={role.role} role={role} />
        ))}
        {data.custom_roles.map((role) => (
          <CustomRoleCard key={role.role_key} role={role} matrix={data.matrix} canManage={canManageRoles} />
        ))}
      </div>
      {canManageRoles && (
        <div>
          <div className="mb-2 flex items-center justify-between">
            <h2 className="text-body font-semibold text-ink-100">Custom Roles</h2>
            <CreateCustomRoleDialog matrix={data.matrix} />
          </div>
        </div>
      )}
      <div>
        <div className="mb-2 flex items-center justify-between">
          <h2 className="text-body font-semibold text-ink-100">Permissions Matrix</h2>
          <span className="text-section text-ink-500">{canManageRoles ? "Clone a role above to customize capabilities" : "Read-only"}</span>
        </div>
        <PermissionsMatrixTable matrix={data.matrix} />
      </div>
    </div>
  );
}

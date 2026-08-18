"use client";

import { useState } from "react";
import useSWR from "swr";
import { TEAM_SWR_KEY, getTeam } from "@/lib/api/team-api";
import { TeamHeader } from "@/components/team/team-header";
import { MembersTab } from "@/components/team/members-tab";
import { RolesTab } from "@/components/team/roles-tab";
import { RepoAccessTab } from "@/components/team/repo-access-tab";
import { AuditLogTab } from "@/components/team/audit-log-tab";
import { OrgIntegrationsTab } from "@/components/team/org-integrations-tab";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";

export function TeamPageClient() {
  const [tab, setTab] = useState("members");
  const { data: team } = useSWR(TEAM_SWR_KEY, getTeam);

  if (!team) return null;

  return (
    <div className="flex h-full flex-col">
      <TeamHeader team={team} />
      <Tabs value={tab} onValueChange={setTab} className="min-h-0 flex-1">
        <TabsList variant="line" className="shrink-0 border-b border-border-strong px-8">
          <TabsTrigger value="members">Members</TabsTrigger>
          <TabsTrigger value="roles">Roles &amp; Permissions</TabsTrigger>
          <TabsTrigger value="repos">Repository Access</TabsTrigger>
          {(team.role === "admin" || team.is_owner) && <TabsTrigger value="integrations">Integrations</TabsTrigger>}
          {team.role === "admin" && <TabsTrigger value="audit">Audit Log</TabsTrigger>}
        </TabsList>
        <div className="flex-1 overflow-y-auto px-8 py-6">
          <TabsContent value="members" className="mt-0">
            <MembersTab />
          </TabsContent>
          <TabsContent value="roles" className="mt-0">
            <RolesTab />
          </TabsContent>
          <TabsContent value="repos" className="mt-0">
            <RepoAccessTab />
          </TabsContent>
          <TabsContent value="integrations" className="mt-0">
            <OrgIntegrationsTab />
          </TabsContent>
          <TabsContent value="audit" className="mt-0">
            <AuditLogTab />
          </TabsContent>
        </div>
      </Tabs>
    </div>
  );
}

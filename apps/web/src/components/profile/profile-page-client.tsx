"use client";

import { useState } from "react";
import useSWR from "swr";
import { Bell, Users } from "lucide-react";
import type { SessionUser } from "@/lib/auth/types";
import { PROFILE_SWR_KEY, getProfile } from "@/lib/api/profile-api";
import { ProfileHero } from "@/components/profile/profile-hero";
import { AccountTab } from "@/components/profile/account-tab";
import { SecurityTab } from "@/components/profile/security-tab";
import { ApiKeysTab } from "@/components/profile/api-keys-tab";
import { PersonalIntegrationsTab } from "@/components/profile/personal-integrations-tab";
import { EmptyState } from "@/components/shared/empty-state";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";

export function ProfilePageClient({ initialUser, apiUrl, apiUrlIsGuessed }: { initialUser: SessionUser; apiUrl: string; apiUrlIsGuessed: boolean }) {
  const [tab, setTab] = useState("account");
  const { data: user } = useSWR(PROFILE_SWR_KEY, getProfile, { fallbackData: initialUser });

  return (
    <div className="flex h-full flex-col">
      <ProfileHero user={user ?? initialUser} />
      <Tabs value={tab} onValueChange={setTab} className="min-h-0 flex-1">
        <TabsList variant="line" className="shrink-0 border-b border-border-strong px-8">
          <TabsTrigger value="account">Account</TabsTrigger>
          <TabsTrigger value="security">Security</TabsTrigger>
          <TabsTrigger value="notifications">Notifications</TabsTrigger>
          <TabsTrigger value="team">Team &amp; Access</TabsTrigger>
          <TabsTrigger value="api-keys">API Keys</TabsTrigger>
          <TabsTrigger value="integrations">Integrations</TabsTrigger>
        </TabsList>
        <div className="flex-1 overflow-y-auto px-8 py-6">
          <TabsContent value="account" className="mt-0 max-w-[900px]">
            <AccountTab user={user ?? initialUser} />
          </TabsContent>
          <TabsContent value="security" className="mt-0">
            <SecurityTab user={user ?? initialUser} />
          </TabsContent>
          <TabsContent value="notifications" className="mt-0">
            <EmptyState icon={Bell} title="Coming soon" description="Notification preferences will land here." />
          </TabsContent>
          <TabsContent value="team" className="mt-0">
            <EmptyState icon={Users} title="Coming soon" description="Team membership and role management will land here." />
          </TabsContent>
          <TabsContent value="api-keys" className="mt-0">
            <ApiKeysTab apiUrl={apiUrl} apiUrlIsGuessed={apiUrlIsGuessed} />
          </TabsContent>
          <TabsContent value="integrations" className="mt-0">
            <PersonalIntegrationsTab />
          </TabsContent>
        </div>
      </Tabs>
    </div>
  );
}

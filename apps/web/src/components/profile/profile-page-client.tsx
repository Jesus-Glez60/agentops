"use client";

import { useState } from "react";
import useSWR from "swr";
import type { SessionUser } from "@/lib/auth/types";
import { PROFILE_SWR_KEY, getProfile } from "@/lib/api/profile-api";
import { ProfileHero } from "@/components/profile/profile-hero";
import { AccountTab } from "@/components/profile/account-tab";
import { SecurityTab } from "@/components/profile/security-tab";
import { ApiKeysTab } from "@/components/profile/api-keys-tab";
import { ConnectToolSection } from "@/components/profile/connect-tool-section";
import { PersonalIntegrationsTab } from "@/components/profile/personal-integrations-tab";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";

// Notifications and Team & Access tabs retired here (redesign plan Phase
// 8) -- both were pure `EmptyState` "Coming soon" stubs with zero backing
// logic, not real features, and the design doesn't show them either.
// Integrations stays even though the design's own mockup doesn't show it:
// it's a real, working feature, and "the design is the true UI" is never
// grounds to remove a shipped feature the design merely didn't depict --
// only stub placeholders get hidden to match it.
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
          <TabsTrigger value="api-keys">API Keys</TabsTrigger>
          <TabsTrigger value="coding-tools">Coding Tools</TabsTrigger>
          <TabsTrigger value="integrations">Integrations</TabsTrigger>
        </TabsList>
        <div className="flex-1 overflow-y-auto px-8 py-6">
          <TabsContent value="account" className="mt-0 max-w-[900px]">
            <AccountTab user={user ?? initialUser} />
          </TabsContent>
          <TabsContent value="security" className="mt-0">
            <SecurityTab user={user ?? initialUser} />
          </TabsContent>
          <TabsContent value="api-keys" className="mt-0">
            <ApiKeysTab />
          </TabsContent>
          <TabsContent value="coding-tools" className="mt-0">
            <ConnectToolSection apiUrl={apiUrl} apiUrlIsGuessed={apiUrlIsGuessed} />
          </TabsContent>
          <TabsContent value="integrations" className="mt-0">
            <PersonalIntegrationsTab />
          </TabsContent>
        </div>
      </Tabs>
    </div>
  );
}

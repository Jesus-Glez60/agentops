"use client";

import { useSWRConfig } from "swr";
import { toast } from "sonner";
import type { SessionUser } from "@/lib/auth/types";
import { PROFILE_SWR_KEY, updatePreferences } from "@/lib/api/profile-api";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Checkbox } from "@/components/ui/checkbox";

export function PreferencesCard({ user }: { user: SessionUser }) {
  const { mutate } = useSWRConfig();

  async function save(update: Parameters<typeof updatePreferences>[0]) {
    try {
      await updatePreferences(update);
      await mutate(PROFILE_SWR_KEY);
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Couldn't update preferences. Please try again.");
    }
  }

  return (
    <Card>
      <CardHeader className="border-b border-border-strong pb-4">
        <CardTitle>Preferences</CardTitle>
      </CardHeader>
      <CardContent className="divide-y divide-border-strong p-0">
        <Row title="Interface theme" description="Applies to the entire AgentOps interface">
          <Select defaultValue={user.theme_pref} onValueChange={(v) => save({ theme_pref: v })}>
            <SelectTrigger size="sm" className="w-44">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="dark">Dark</SelectItem>
              <SelectItem value="light">Light</SelectItem>
              <SelectItem value="system">System</SelectItem>
            </SelectContent>
          </Select>
        </Row>
        <Row title="Default search scope" description="Starting context for semantic search queries">
          <Select defaultValue={user.default_search_scope} onValueChange={(v) => save({ default_search_scope: v })}>
            <SelectTrigger size="sm" className="w-44">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="all">All repositories</SelectItem>
            </SelectContent>
          </Select>
        </Row>
        <Row title="Show gotcha callouts inline" description="Surface gotcha nodes within the documentation viewer">
          <Checkbox checked={user.show_gotcha_callouts} onCheckedChange={(checked) => save({ show_gotcha_callouts: checked === true })} />
        </Row>
        <Row title="Graph layout algorithm" description="Default layout used in Knowledge Graph views">
          <Select defaultValue={user.graph_layout_algorithm} onValueChange={(v) => save({ graph_layout_algorithm: v })}>
            <SelectTrigger size="sm" className="w-44">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="force">Force-directed</SelectItem>
              <SelectItem value="hierarchical">Hierarchical</SelectItem>
              <SelectItem value="radial">Radial</SelectItem>
            </SelectContent>
          </Select>
        </Row>
      </CardContent>
    </Card>
  );
}

function Row({ title, description, children }: { title: string; description: string; children: React.ReactNode }) {
  return (
    <div className="flex items-center justify-between px-6 py-4">
      <div>
        <p className="text-body font-medium text-ink-100">{title}</p>
        <p className="text-section text-ink-500">{description}</p>
      </div>
      {children}
    </div>
  );
}

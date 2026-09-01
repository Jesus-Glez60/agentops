"use client";

import { useState } from "react";
import useSWR from "swr";
import { toast } from "sonner";
import { getOrgIntegrations, storeOrgIntegration, deleteOrgIntegration, ORG_INTEGRATIONS_SWR_KEY, type IntegrationSummary } from "@/lib/api/integrations-api";
import { relativeTimeFromIsoString } from "@/lib/relative-time";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { GithubAppIntegrationCard } from "@/components/team/github-app-integration-card";

/** The two providers this deployment actually resolves credentials for
 * today (`resolve_linear_config`/`resolve_anthropic_config` in
 * agentops-heavy-api) -- the backend vault itself is provider-agnostic, so
 * this list is a frontend-only display choice, not a backend constraint. */
const KNOWN_PROVIDERS: { key: string; label: string; placeholder: string; description: string }[] = [
  { key: "linear", label: "Linear", placeholder: "lin_api_…", description: "Powers auto-kickoff and issue sync for the whole org." },
  { key: "anthropic", label: "Anthropic", placeholder: "sk-ant-…", description: "Used for LLM-assisted features (BYOK) across the org." },
];

function ProviderRow({ provider, connected }: { provider: (typeof KNOWN_PROVIDERS)[number]; connected: IntegrationSummary | undefined }) {
  const { mutate } = useSWR(ORG_INTEGRATIONS_SWR_KEY, getOrgIntegrations);
  const [apiKey, setApiKey] = useState("");
  const [saving, setSaving] = useState(false);
  const [disconnecting, setDisconnecting] = useState(false);

  async function handleConnect(e: React.FormEvent) {
    e.preventDefault();
    const trimmed = apiKey.trim();
    if (!trimmed) return;
    setSaving(true);
    try {
      await storeOrgIntegration(provider.key, { auth_type: "api_key", secret: trimmed });
      setApiKey("");
      await mutate();
      toast.success(`${provider.label} connected`);
    } catch (err) {
      toast.error(err instanceof Error ? err.message : `Couldn't connect ${provider.label}. Please try again.`);
    } finally {
      setSaving(false);
    }
  }

  async function handleDisconnect() {
    setDisconnecting(true);
    try {
      await deleteOrgIntegration(provider.key);
      await mutate();
      toast.success(`${provider.label} disconnected`);
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Couldn't disconnect. Please try again.");
    } finally {
      setDisconnecting(false);
    }
  }

  return (
    <div className="flex items-center justify-between gap-4 px-6 py-4">
      <div className="min-w-0">
        <p className="text-body font-medium text-ink-100">{provider.label}</p>
        <p className="truncate text-mono-code text-ink-500">{connected ? `Connected — last updated ${relativeTimeFromIsoString(connected.updated_at)}` : provider.description}</p>
      </div>
      {connected ? (
        <Button variant="outline" size="sm" onClick={handleDisconnect} disabled={disconnecting} className="shrink-0">
          {disconnecting ? "Disconnecting…" : "Disconnect"}
        </Button>
      ) : (
        <form onSubmit={handleConnect} className="flex shrink-0 items-center gap-2">
          <Input value={apiKey} onChange={(e) => setApiKey(e.target.value)} placeholder={provider.placeholder} className="w-56" />
          <Button type="submit" size="sm" disabled={saving || !apiKey.trim()}>
            {saving ? "Connecting…" : "Connect"}
          </Button>
        </form>
      )}
    </div>
  );
}

/** Owner/Admin only -- gated the same way the Team Management page hides
 * this tab entirely for non-admins (see `team-page-client.tsx`), matching
 * the backend's `integrations.manage` capability requirement. */
export function OrgIntegrationsTab() {
  const { data: integrations, isLoading } = useSWR(ORG_INTEGRATIONS_SWR_KEY, getOrgIntegrations);

  return (
    <div className="max-w-[900px]">
      <Card>
        <CardHeader className="border-b border-border-strong pb-4">
          <CardTitle>Org-wide Integrations</CardTitle>
        </CardHeader>
        <CardContent className="divide-y divide-border-strong p-0">
          {isLoading && <p className="px-6 py-4 text-body text-ink-500">Loading…</p>}
          {!isLoading && KNOWN_PROVIDERS.map((provider) => <ProviderRow key={provider.key} provider={provider} connected={integrations?.find((i) => i.provider === provider.key)} />)}
        </CardContent>
      </Card>
      <p className="mt-3 text-section text-ink-500">These credentials are shared by the whole organization and managed by Owners and Admins only.</p>

      <GithubAppIntegrationCard />
    </div>
  );
}

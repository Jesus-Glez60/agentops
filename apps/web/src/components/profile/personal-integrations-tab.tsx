"use client";

import { useState } from "react";
import useSWR from "swr";
import { toast } from "sonner";
import { getMyIntegrations, storeMyIntegration, deleteMyIntegration, MY_INTEGRATIONS_SWR_KEY } from "@/lib/api/integrations-api";
import { getGithubAppInstallations, GITHUB_APP_INSTALLATIONS_SWR_KEY } from "@/lib/api/repos-api";
import { relativeTimeFromIsoString } from "@/lib/relative-time";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";

/** Read-only -- the connect/manage action lives on Team & Access >
 * Integrations (`GithubAppIntegrationCard`, admin/owner-gated, since a
 * GitHub App installation is tenant-wide, not per-user like Linear below).
 * This just lets a solo/individual user see connection status without
 * hunting through Team settings. */
function GithubAppStatusRow() {
  const { data, isLoading } = useSWR(GITHUB_APP_INSTALLATIONS_SWR_KEY, getGithubAppInstallations);
  const installations = data?.installations ?? [];

  if (isLoading) return null;
  return (
    <div className="flex items-center justify-between gap-4 px-6 py-4">
      <div className="min-w-0">
        <p className="text-body font-medium text-ink-100">GitHub App</p>
        <p className="truncate text-mono-code text-ink-500">
          {installations.length > 0 ? `Connected as ${installations[0].account_login}` : "Not connected — manage this from Team & Access > Integrations."}
        </p>
      </div>
    </div>
  );
}

const LINEAR_PROVIDER = "linear";

/** Linear-only UI for this pass (confirmed scope) -- the backend
 * (`/integrations/me*`) is provider-agnostic, so adding a second provider
 * later is a frontend-only change, not a backend one. */
export function PersonalIntegrationsTab() {
  const { data: integrations, mutate, isLoading } = useSWR(MY_INTEGRATIONS_SWR_KEY, getMyIntegrations);
  const [apiKey, setApiKey] = useState("");
  const [saving, setSaving] = useState(false);
  const [disconnecting, setDisconnecting] = useState(false);

  const linear = integrations?.find((i) => i.provider === LINEAR_PROVIDER);

  async function handleConnect(e: React.FormEvent) {
    e.preventDefault();
    const trimmed = apiKey.trim();
    if (!trimmed) return;
    setSaving(true);
    try {
      await storeMyIntegration(LINEAR_PROVIDER, { auth_type: "api_key", secret: trimmed });
      setApiKey("");
      await mutate();
      toast.success("Linear connected");
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Couldn't connect Linear. Please try again.");
    } finally {
      setSaving(false);
    }
  }

  async function handleDisconnect() {
    setDisconnecting(true);
    try {
      await deleteMyIntegration(LINEAR_PROVIDER);
      await mutate();
      toast.success("Linear disconnected");
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Couldn't disconnect. Please try again.");
    } finally {
      setDisconnecting(false);
    }
  }

  return (
    <div className="max-w-[900px]">
      <Card>
        <CardHeader className="border-b border-border-strong pb-4">
          <CardTitle>Personal Integrations</CardTitle>
        </CardHeader>
        <CardContent className="divide-y divide-border-strong p-0">
          {isLoading && <p className="px-6 py-4 text-body text-ink-500">Loading…</p>}
          {!isLoading && (
            <div className="flex items-center justify-between gap-4 px-6 py-4">
              <div className="min-w-0">
                <p className="text-body font-medium text-ink-100">Linear</p>
                <p className="truncate text-mono-code text-ink-500">
                  {linear ? `Connected — last updated ${relativeTimeFromIsoString(linear.updated_at)}` : "Connect your own Linear account to pull issues assigned to you."}
                </p>
              </div>
              {linear ? (
                <Button variant="outline" size="sm" onClick={handleDisconnect} disabled={disconnecting} className="shrink-0">
                  {disconnecting ? "Disconnecting…" : "Disconnect"}
                </Button>
              ) : (
                <form onSubmit={handleConnect} className="flex shrink-0 items-center gap-2">
                  <Input value={apiKey} onChange={(e) => setApiKey(e.target.value)} placeholder="lin_api_…" className="w-56" />
                  <Button type="submit" size="sm" disabled={saving || !apiKey.trim()}>
                    {saving ? "Connecting…" : "Connect"}
                  </Button>
                </form>
              )}
            </div>
          )}
          <GithubAppStatusRow />
        </CardContent>
      </Card>
      <p className="mt-3 text-section text-ink-500">Personal connections are yours alone — they pull your own assigned issues and are never visible to other members, even admins.</p>
    </div>
  );
}

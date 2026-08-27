"use client";

import useSWR from "swr";
import { toast } from "sonner";
import { API_KEYS_SWR_KEY, getApiKeys, revokeApiKey } from "@/lib/api/profile-api";
import { relativeTimeFromIsoString } from "@/lib/relative-time";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { CreateApiKeyDialog } from "@/components/profile/create-api-key-dialog";

export function ApiKeysTab() {
  const { data: keys, mutate, isLoading } = useSWR(API_KEYS_SWR_KEY, getApiKeys);

  async function handleRevoke(id: number) {
    try {
      await revokeApiKey(id);
      await mutate();
      toast.success("Key revoked");
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Couldn't revoke that key. Please try again.");
    }
  }

  return (
    <div className="max-w-[900px]">
      <Card>
        <CardHeader className="flex-row items-center justify-between border-b border-border-strong pb-4">
          <CardTitle>API Keys</CardTitle>
          <CreateApiKeyDialog />
        </CardHeader>
        <CardContent className="divide-y divide-border-strong p-0">
          {isLoading && <p className="px-6 py-4 text-body text-ink-500">Loading…</p>}
          {!isLoading && (keys ?? []).length === 0 && <p className="px-6 py-4 text-body text-ink-500">No API keys yet.</p>}
          {(keys ?? []).map((key) => (
            <div key={key.id} className="flex items-center justify-between px-6 py-3">
              <div>
                <p className="text-body font-medium text-ink-100">{key.name}</p>
                <p className="text-mono-code text-ink-500">
                  {key.key_prefix}
                  {"••••••••••••"} &middot; {key.last_used_at ? `Last used ${relativeTimeFromIsoString(key.last_used_at)}` : "Never used"}
                </p>
              </div>
              <Button variant="outline" size="sm" onClick={() => handleRevoke(key.id)}>
                Revoke
              </Button>
            </div>
          ))}
        </CardContent>
      </Card>
      <p className="mt-3 text-section text-ink-500">API keys grant full read access to your indexed repositories. Never share them publicly.</p>
    </div>
  );
}

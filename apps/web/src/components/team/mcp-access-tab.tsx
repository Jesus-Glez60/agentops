"use client";

import useSWR from "swr";
import { toast } from "sonner";
import { TEAM_MCP_ACCESS_MODE_SWR_KEY, getMcpAccessMode, setMcpAccessMode, type McpAccessMode } from "@/lib/api/team-api";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";

/**
 * Owner/Admin only -- gated the same way `OrgIntegrationsTab` is (see
 * `team-page-client.tsx`), matching the backend's `mcp.manage_access_mode`
 * capability requirement. Controls whether `/mcp` write tools (`scan_repo`,
 * `explain_symbol`, task tools, ...) are enabled for the whole org, not
 * just the caller -- defaults to Advisor (read-only) until an admin opts
 * in, same as the deployment-level `AGENTOPS_ACCESS_MODE` env var this
 * replaces the need to set manually.
 */
export function McpAccessTab() {
  const { data, isLoading, mutate } = useSWR(TEAM_MCP_ACCESS_MODE_SWR_KEY, getMcpAccessMode);

  async function save(mode: McpAccessMode) {
    const previous = data;
    await mutate({ mode }, { revalidate: false });
    try {
      await setMcpAccessMode(mode);
    } catch (err) {
      await mutate(previous, { revalidate: false });
      toast.error(err instanceof Error ? err.message : "Couldn't update MCP access mode. Please try again.");
    }
  }

  return (
    <div className="max-w-[900px]">
      <Card>
        <CardHeader className="border-b border-border-strong pb-4">
          <CardTitle>MCP Access Mode</CardTitle>
        </CardHeader>
        <CardContent className="divide-y divide-border-strong p-0">
          <div className="flex items-center justify-between gap-6 px-6 py-4">
            <div>
              <p className="text-body font-medium text-ink-100">Write access over MCP</p>
              <p className="max-w-[520px] text-section text-ink-500">
                Advisor (read-only) blocks write tools like <code className="text-mono-code">scan_repo</code>, <code className="text-mono-code">explain_symbol</code>, and task tools for every
                agent connected over MCP. <code className="text-mono-code">add_note</code> and <code className="text-mono-code">ingest_notes</code> always work regardless of this setting —
                growing the knowledge base isn&apos;t a destructive action.
              </p>
            </div>
            {isLoading || !data ? (
              <p className="shrink-0 text-mono-code text-ink-500">Loading…</p>
            ) : (
              <Select value={data.mode} onValueChange={(v) => save(v as McpAccessMode)}>
                <SelectTrigger size="sm" className="w-44 shrink-0">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="advisor">Advisor (read-only)</SelectItem>
                  <SelectItem value="full">Full (read + write)</SelectItem>
                </SelectContent>
              </Select>
            )}
          </div>
        </CardContent>
      </Card>
    </div>
  );
}

"use client";

import useSWR from "swr";
import { toast } from "sonner";
import { TEAM_MCP_ACCESS_MODE_SWR_KEY, getMcpAccessMode, setMcpAccessMode, type McpAccessMode } from "@/lib/api/team-api";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { cn } from "@/lib/utils";

const MODES: { value: McpAccessMode; title: string; description: string }[] = [
  {
    value: "advisor",
    title: "Advisor (read-only)",
    description: "Blocks write tools like scan_repo, explain_symbol, and task tools for every agent connected over MCP.",
  },
  {
    value: "full",
    title: "Full (read + write)",
    description: "Every /mcp tool is enabled for every agent connected to this org, including scan_repo, explain_symbol, and task tools.",
  },
];

/**
 * Owner/Admin only -- gated the same way `OrgIntegrationsTab` is (see
 * `team-page-client.tsx`), matching the backend's `mcp.manage_access_mode`
 * capability requirement. Controls whether `/mcp` write tools (`scan_repo`,
 * `explain_symbol`, task tools, ...) are enabled for the whole org, not
 * just the caller -- defaults to Advisor (read-only) until an admin opts
 * in, same as the deployment-level `AGENTOPS_ACCESS_MODE` env var this
 * replaces the need to set manually.
 *
 * Radio-cards, not a `Select` dropdown (redesign plan Phase 7) -- reuses
 * the same whole-card-is-the-button pattern already built for the
 * Repositories Connect wizard's method-selection cards
 * (`app/(app)/repositories/connect/page.tsx`'s `ChooseMethodView`), not a
 * new `RadioGroup` primitive: neither that pattern nor the design's own
 * markup uses a real `<input type="radio">`, just a styled button with a
 * manually-drawn selected/unselected ring.
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
        <CardContent className="pt-5">
          <p className="mb-4 max-w-[640px] text-section text-ink-500">
            Controls write access over MCP for every agent connected to this org. <code className="text-mono-code">add_note</code> and <code className="text-mono-code">ingest_notes</code> always
            work regardless of this setting — growing the knowledge base isn&apos;t a destructive action.
          </p>
          {isLoading || !data ? (
            <p className="text-mono-code text-ink-500">Loading…</p>
          ) : (
            <div className="grid gap-4" style={{ gridTemplateColumns: "repeat(auto-fit, minmax(260px, 1fr))" }}>
              {MODES.map((mode) => {
                const isSelected = data.mode === mode.value;
                return (
                  <button
                    key={mode.value}
                    type="button"
                    onClick={() => save(mode.value)}
                    aria-pressed={isSelected}
                    className={cn("rounded-lg border p-4 text-left transition-colors", isSelected ? "border-primary bg-primary/5" : "border-border-strong hover:border-ink-500")}
                  >
                    <div className="mb-2 flex items-center gap-2.5">
                      <span className={cn("flex size-4 shrink-0 items-center justify-center rounded-full border", isSelected ? "border-primary" : "border-border-strong")}>
                        {isSelected && <span className="size-2 rounded-full bg-primary" />}
                      </span>
                      <span className="text-[17px] font-semibold text-ink-100">{mode.title}</span>
                    </div>
                    <p className="pl-[26px] text-section leading-relaxed text-ink-500">{mode.description}</p>
                  </button>
                );
              })}
            </div>
          )}
        </CardContent>
      </Card>
    </div>
  );
}

"use client";

// Persistent "Connect a coding tool" home (Initiative 2), rendered as its
// own Profile tab -- the same tool-select + connect.sh command generation
// the onboarding checklist's remote-connect step uses, so reconnecting,
// adding another tool, or recovering from something going wrong during
// onboarding never requires going back through /welcome and redoing the
// whole checklist. The user-menu's "Connect a coding tool" link (which
// routes to /welcome) still works too; this is a second, more discoverable
// home for the same action -- caught live: there was no way to retry or add
// tools short of re-onboarding.
import { useState } from "react";
import { ChevronRight } from "lucide-react";
import { toast } from "sonner";
import { createApiKey } from "@/lib/api/profile-api";
import { ToolSelect, DEFAULT_SELECTED_AGENTS } from "@/components/onboarding/tool-select";
import { CopyButton } from "@/components/shared/copy-button";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";

export function ConnectToolSection({ apiUrl, apiUrlIsGuessed }: { apiUrl: string; apiUrlIsGuessed: boolean }) {
  const [selectedAgents, setSelectedAgents] = useState<string[]>(DEFAULT_SELECTED_AGENTS);
  const [apiKey, setApiKey] = useState<string | null>(null);
  const [generating, setGenerating] = useState(false);
  const agentsArg = selectedAgents.length > 0 ? selectedAgents.join(",") : DEFAULT_SELECTED_AGENTS.join(",");
  const npxCommand = apiKey ? `export AGENTOPS_API_KEY=${apiKey} && npx agentops-cli connect --remote ${apiUrl} --api-key "$AGENTOPS_API_KEY" --agents ${agentsArg}` : "";
  const curlCommand = apiKey ? `export AGENTOPS_API_KEY=${apiKey} && curl -fsSL ${apiUrl}/connect.sh?agents=${agentsArg} | sh` : "";

  async function generate() {
    setGenerating(true);
    try {
      const created = await createApiKey("Coding tool");
      setApiKey(created.key);
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Couldn't generate an API key. Please try again.");
    } finally {
      setGenerating(false);
    }
  }

  return (
    <Card className="max-w-[900px]">
      <CardHeader>
        <CardTitle>Connect a coding tool</CardTitle>
      </CardHeader>
      <CardContent className="space-y-3">
        {apiUrlIsGuessed && (
          <p className="rounded-md border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-body text-amber-500">
            Couldn&apos;t confirm this server&apos;s public API address — guessed <code className="text-mono-code">{apiUrl}</code>. If wrong, set <code className="text-mono-code">AGENTOPS_PUBLIC_API_URL</code> and reload.
          </p>
        )}
        <ToolSelect selected={selectedAgents} onChange={setSelectedAgents} />
        {apiKey === null ? (
          <Button size="sm" disabled={generating} onClick={generate}>
            {generating ? "Generating…" : "Generate API key"}
          </Button>
        ) : (
          <>
            <p className="text-body text-ink-400">Copy this now — it won&apos;t be shown again. From your own machine (installs the CLI if it isn&apos;t already there), run:</p>
            <div className="flex items-center gap-2">
              <code className="flex-1 truncate rounded-md border border-border-strong bg-panel px-3 py-2 text-mono-code text-ink-200">{npxCommand}</code>
              <CopyButton value={npxCommand} />
            </div>
            <Collapsible>
              <CollapsibleTrigger className="group flex items-center gap-1 text-body text-ink-500 hover:text-ink-300">
                <ChevronRight className="size-3.5 transition-transform group-data-[state=open]:rotate-90" />
                Advanced: install via curl instead
              </CollapsibleTrigger>
              <CollapsibleContent className="space-y-2 pt-2">
                <div className="flex items-center gap-2">
                  <code className="flex-1 truncate rounded-md border border-border-strong bg-panel px-3 py-2 text-mono-code text-ink-200">{curlCommand}</code>
                  <CopyButton value={curlCommand} />
                </div>
                <a href={`${apiUrl}/connect.sh?agents=${agentsArg}`} target="_blank" rel="noreferrer" className="inline-block text-body text-ink-500 underline underline-offset-2 hover:text-ink-300">
                  Preview the script before running it
                </a>
              </CollapsibleContent>
            </Collapsible>
          </>
        )}
      </CardContent>
    </Card>
  );
}

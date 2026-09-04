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
import { ToolSelect, DEFAULT_SELECTED_AGENTS } from "@/components/onboarding/tool-select";
import { CopyButton } from "@/components/shared/copy-button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";

export function ConnectToolSection({ apiUrl, apiUrlIsGuessed }: { apiUrl: string; apiUrlIsGuessed: boolean }) {
  const [selectedAgents, setSelectedAgents] = useState<string[]>(DEFAULT_SELECTED_AGENTS);
  const agentsArg = selectedAgents.length > 0 ? selectedAgents.join(",") : DEFAULT_SELECTED_AGENTS.join(",");
  // No API key to generate anymore -- `agentops connect` logs in via a
  // browser-based device-authorization flow the first time it runs (see
  // `agentops-cli`'s `device_flow_login`), so this command is identical
  // for every user and needs no per-visit setup step in this UI at all.
  const npxCommand = `npx agentops-cli connect --remote ${apiUrl} --agents ${agentsArg}`;
  const curlCommand = `curl -fsSL ${apiUrl}/connect.sh?agents=${agentsArg} | sh`;
  // Module 8 (usage/knowledge-reuse dashboard): a one-time device-login the
  // first time, same as the connect command above -- the server URL gets
  // persisted into that repo's `.context/agentops-remote.json`, so every
  // later `usage sync` there needs no `--remote` at all.
  const usageSyncCommand = `npx agentops-cli usage sync --remote ${apiUrl}`;

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
        <p className="text-body text-ink-400">From your own machine (installs the CLI if it isn&apos;t already there, then opens a browser to log in), run:</p>
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
        <Collapsible>
          <CollapsibleTrigger className="group flex items-center gap-1 text-body text-ink-500 hover:text-ink-300">
            <ChevronRight className="size-3.5 transition-transform group-data-[state=open]:rotate-90" />
            Track token usage &amp; knowledge reuse
          </CollapsibleTrigger>
          <CollapsibleContent className="space-y-2 pt-2">
            <p className="text-body text-ink-400">From a connected repo, run this to sync local session token/cost data into its Usage card:</p>
            <div className="flex items-center gap-2">
              <code className="flex-1 truncate rounded-md border border-border-strong bg-panel px-3 py-2 text-mono-code text-ink-200">{usageSyncCommand}</code>
              <CopyButton value={usageSyncCommand} />
            </div>
          </CollapsibleContent>
        </Collapsible>
      </CardContent>
    </Card>
  );
}

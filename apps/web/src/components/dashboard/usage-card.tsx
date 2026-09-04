import { Info } from "lucide-react";
import type { UsageSummary } from "@/lib/api/repos-api";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { CopyButton } from "@/components/shared/copy-button";

function formatTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`;
  return String(n);
}

function formatUsd(n: number): string {
  return `$${n.toFixed(2)}`;
}

function Stat({ label, value, className }: { label: string; value: string; className?: string }) {
  return (
    <div className={className}>
      <p className="text-mono-code uppercase text-ink-500">{label}</p>
      <p className="text-body font-medium text-ink-100">{value}</p>
    </div>
  );
}

/**
 * Module 8 (CodeBurn-inspired usage/knowledge-reuse tracking) dashboard
 * card. `usage.hit_count` is a real, exact count; `estimated_tokens_saved`/
 * `estimated_cost_saved_usd` are a heuristic aggregate (see
 * `agentops_api::usage`'s doc comment) -- always rendered with an explicit
 * "estimated" label and an info tooltip explaining the caveat, never as a
 * precise number. Full-width block, not a `Field` grid cell: more visual
 * room than the surrounding health-summary grid needs.
 *
 * `usage: null` (no `session_usage` rows synced yet) renders the sync
 * command instead of a stat grid -- `apiUrl` is what makes `--remote`
 * concrete, same as `ConnectToolSection`'s `connect --remote` command (see
 * that component's doc comment for why no per-visit API key is needed:
 * `usage sync --remote` device-logs in once and persists the result into
 * `.context/agentops-remote.json`, same marker `connect --remote` writes).
 */
export function UsageCard({ usage, apiUrl }: { usage: UsageSummary | null; apiUrl: string }) {
  const usageSyncCommand = `npx agentops-cli usage sync --remote ${apiUrl}`;

  if (!usage || (usage.tokens.input_tokens === 0 && usage.tokens.output_tokens === 0 && usage.hit_count === 0)) {
    return (
      <div className="rounded-lg border border-border-strong p-4">
        <h2 className="mb-2 text-body font-medium text-ink-100">Usage &amp; knowledge reuse</h2>
        <p className="mb-2 text-body text-ink-400">No usage data synced yet. From your local checkout of this repo, run:</p>
        <div className="flex items-center gap-2">
          <code className="flex-1 truncate rounded-md border border-border-strong bg-panel px-3 py-2 text-mono-code text-ink-200">{usageSyncCommand}</code>
          <CopyButton value={usageSyncCommand} />
        </div>
      </div>
    );
  }

  const totalTokens = usage.tokens.input_tokens + usage.tokens.output_tokens;

  return (
    <div className="rounded-lg border border-border-strong p-4">
      <div className="mb-3 flex items-center gap-1.5">
        <h2 className="text-body font-medium text-ink-100">Usage &amp; knowledge reuse</h2>
        <Tooltip>
          <TooltipTrigger asChild>
            <Info className="size-3.5 cursor-help text-ink-500" />
          </TooltipTrigger>
          <TooltipContent className="max-w-xs">
            Tokens/cost are synced from local Claude Code session transcripts (`agentops usage sync`). &quot;Estimated saved&quot; is a rough aggregate — hit count × an assumed average research-turn cost — not a measured counterfactual.
          </TooltipContent>
        </Tooltip>
      </div>

      <div className="grid grid-cols-2 gap-x-8 gap-y-4 sm:grid-cols-4">
        <Stat label="Tokens (30d)" value={formatTokens(totalTokens)} />
        <Stat label="Cost (30d)" value={formatUsd(usage.tokens.cost_usd)} />
        <Stat label="Knowledge hits" value={String(usage.hit_count)} />
        <Stat label="Est. saved" value={`${formatTokens(usage.estimated_tokens_saved)} / ${formatUsd(usage.estimated_cost_saved_usd)}`} />
      </div>

      <div className="mt-3 flex items-center gap-2">
        <code className="flex-1 truncate rounded-md border border-border-strong bg-panel px-3 py-2 text-mono-code text-ink-200">{usageSyncCommand}</code>
        <CopyButton value={usageSyncCommand} />
      </div>
    </div>
  );
}

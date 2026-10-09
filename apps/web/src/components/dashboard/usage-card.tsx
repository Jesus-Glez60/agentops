import { Info } from "lucide-react";
import type { LlmSpend, SessionBreakdown, UsageSummary } from "@/lib/api/repos-api";
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

function sumTotals(t: UsageSummary["tokens"]): number {
  return t.input_tokens + t.output_tokens + t.cache_read_tokens + t.cache_write_tokens;
}

/**
 * Per-session view -- what one working session (one improvement) cost.
 * Totals include every cached re-read of the conversation, which is where
 * almost all of a long session's tokens go; "Peak context" is how big the
 * conversation got, shown separately because it's a size, not a sum.
 */
function RecentSessionsSection({ sessions }: { sessions: SessionBreakdown[] }) {
  if (sessions.length === 0) return null;
  return (
    <div className="mt-4">
      <p className="mb-2 text-mono-code uppercase text-ink-500">Recent sessions</p>
      <div className="overflow-x-auto">
        <table className="w-full text-body">
          <thead>
            <tr className="text-left text-mono-code uppercase text-ink-500">
              <th className="py-1 pr-4 font-normal">Started</th>
              <th className="py-1 pr-4 text-right font-normal">Main</th>
              <th className="py-1 pr-4 text-right font-normal">Subagents</th>
              <th className="py-1 pr-4 text-right font-normal">Peak context</th>
              <th className="py-1 text-right font-normal">Cost (est.)</th>
            </tr>
          </thead>
          <tbody>
            {sessions.map((s) => (
              <tr key={s.session_id} className="border-t border-border-strong text-ink-200">
                <td className="py-1 pr-4 text-ink-400" title={s.session_id}>
                  {new Date(s.started_at).toLocaleString()}
                </td>
                <td className="py-1 pr-4 text-right">{formatTokens(s.main_tokens)}</td>
                <td className="py-1 pr-4 text-right">{s.subagent_tokens > 0 ? formatTokens(s.subagent_tokens) : "—"}</td>
                <td className="py-1 pr-4 text-right">{s.peak_context_tokens > 0 ? formatTokens(s.peak_context_tokens) : "—"}</td>
                <td className="py-1 text-right">{formatUsd(s.cost_usd)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}

function formatSpendCost(row: LlmSpend): string {
  if (row.cost_estimate_usd === null) return "—";
  return row.cost_partial ? `≥ ${formatUsd(row.cost_estimate_usd)} (partial)` : formatUsd(row.cost_estimate_usd);
}

/**
 * AgentOps' own LLM spend (`llm_usage`) -- separate from the session totals
 * above, which come from the coding agent's own transcripts. A group with
 * any unpriced call (free tier, unknown model) renders as partial or "—",
 * never as a complete dollar figure.
 */
function ModelSpendSection({ spend }: { spend: LlmSpend[] }) {
  if (spend.length === 0) return null;
  return (
    <div className="mt-4">
      <p className="mb-2 text-mono-code uppercase text-ink-500">AgentOps model spend (estimated)</p>
      <div className="overflow-x-auto">
        <table className="w-full text-body">
          <thead>
            <tr className="text-left text-mono-code uppercase text-ink-500">
              <th className="py-1 pr-4 font-normal">Operation</th>
              <th className="py-1 pr-4 font-normal">Model</th>
              <th className="py-1 pr-4 text-right font-normal">Calls</th>
              <th className="py-1 pr-4 text-right font-normal">Tokens in / out</th>
              <th className="py-1 text-right font-normal">Cost</th>
            </tr>
          </thead>
          <tbody>
            {spend.map((row) => (
              <tr key={`${row.operation}|${row.provider}|${row.model}`} className="border-t border-border-strong text-ink-200">
                <td className="py-1 pr-4 font-mono text-mono-code">{row.operation}</td>
                <td className="py-1 pr-4 text-ink-400">
                  {row.provider}/{row.model}
                </td>
                <td className="py-1 pr-4 text-right">
                  {row.calls}
                  {row.failures > 0 && <span className="text-ink-500"> ({row.failures} failed)</span>}
                </td>
                <td className="py-1 pr-4 text-right">
                  {formatTokens(row.input_tokens)} / {formatTokens(row.output_tokens)}
                </td>
                <td className="py-1 text-right">{formatSpendCost(row)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
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
        <ModelSpendSection spend={usage?.llm_spend ?? []} />
      </div>
    );
  }

  // Every token category, cache reads included -- they're most of what a
  // long session processes, and leaving them out understated it ~100x.
  const totalTokens = sumTotals(usage.tokens);
  const subagentTokens = usage.subagent_tokens ? sumTotals(usage.subagent_tokens) : 0;

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
        <Stat label="Tokens (all sessions)" value={`${formatTokens(totalTokens)}${subagentTokens > 0 ? ` (${formatTokens(subagentTokens)} subagents)` : ""}`} />
        <Stat label="Cost (est.)" value={formatUsd(usage.tokens.cost_usd)} />
        <Stat label="Knowledge hits" value={String(usage.hit_count)} />
        <Stat label="Est. saved" value={`${formatTokens(usage.estimated_tokens_saved)} / ${formatUsd(usage.estimated_cost_saved_usd)}`} />
      </div>

      <div className="mt-3 flex items-center gap-2">
        <code className="flex-1 truncate rounded-md border border-border-strong bg-panel px-3 py-2 text-mono-code text-ink-200">{usageSyncCommand}</code>
        <CopyButton value={usageSyncCommand} />
      </div>

      <RecentSessionsSection sessions={usage.sessions ?? []} />
      <ModelSpendSection spend={usage.llm_spend ?? []} />
    </div>
  );
}

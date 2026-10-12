import Link from "next/link";
import type { UsageSummary } from "@/lib/api/repos-api";
import { displayRepoName } from "@/lib/utils";

function formatTokens(n: number): string {
  if (n >= 1_000_000_000) return `${(n / 1_000_000_000).toFixed(2)}B`;
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`;
  return String(n);
}

function totalOf(t: UsageSummary["tokens"]): number {
  return t.input_tokens + t.output_tokens + t.cache_read_tokens + t.cache_write_tokens;
}

/** The reset threshold the dashboard headlines; `usage report` simulates 100k/150k/200k. */
const HEADLINE_RESET = 150_000;

/**
 * Org-wide coding-session token tally across every repo's synced usage
 * (`agentops usage sync`). Totals and session counts are exact; "biggest
 * sinks" and the reset figure come from the recent sessions' transcript
 * breakdowns (`reports`, transcript rows only -- agent estimates are left
 * out of an aggregate presented as measured). The reset figure is an upper
 * bound: re-exploration after a reset isn't modelled.
 */
export function TokenTallyCard({ usage }: { usage: { connectionId: string; summary: UsageSummary }[] }) {
  const summaries = usage.map((u) => u.summary);
  const total = summaries.reduce((sum, s) => sum + totalOf(s.tokens), 0);
  if (total === 0) return null;
  const subagents = summaries.reduce((sum, s) => sum + (s.subagent_tokens ? totalOf(s.subagent_tokens) : 0), 0);
  const sessions = summaries.reduce((sum, s) => sum + (s.session_count ?? 0), 0);
  const cost = summaries.reduce((sum, s) => sum + s.tokens.cost_usd, 0);

  const measured = summaries.flatMap((s) => s.reports ?? []).filter((r) => r.source === "transcript");
  const measuredTotal = measured.reduce((sum, r) => sum + r.total_context_tokens, 0);
  const sinks = new Map<string, number>();
  for (const r of measured) for (const [name, tokens] of r.top_categories) sinks.set(name, (sinks.get(name) ?? 0) + tokens);
  const topSinks = [...sinks.entries()].sort((a, b) => b[1] - a[1]).slice(0, 4);
  const simulated = measured.reduce((sum, r) => sum + (r.resets.find((x) => x.threshold === HEADLINE_RESET)?.simulated_tokens ?? r.total_context_tokens), 0);
  const resetSavings = measuredTotal > 0 ? 1 - simulated / measuredTotal : null;

  // Linked by connection id: `summary.repo` is the internal store name.
  const byRepo = usage.map((u) => ({ id: u.connectionId, tokens: totalOf(u.summary.tokens) })).filter((r) => r.tokens > 0).sort((a, b) => b.tokens - a.tokens);

  return (
    <div className="rounded-lg border border-border-strong p-5">
      <div className="mb-4 flex items-baseline justify-between gap-4">
        <h2 className="text-body font-medium text-ink-100">Coding-session tokens</h2>
        <span className="text-mono-code text-ink-500">all repositories · all synced sessions</span>
      </div>

      <div className="grid grid-cols-2 gap-x-8 gap-y-4 sm:grid-cols-4">
        <div>
          <p className="text-mono-code uppercase text-ink-500">Tokens</p>
          <p className="text-display-card font-bold text-ink-100">{formatTokens(total)}</p>
          {subagents > 0 && <p className="text-mono-code text-ink-500">{formatTokens(subagents)} in subagents</p>}
        </div>
        <div>
          <p className="text-mono-code uppercase text-ink-500">Sessions</p>
          <p className="text-display-card font-bold text-ink-100">{sessions.toLocaleString()}</p>
          {sessions > 0 && <p className="text-mono-code text-ink-500">{formatTokens(Math.round(total / sessions))} per session</p>}
        </div>
        <div>
          <p className="text-mono-code uppercase text-ink-500">Cost (API-equivalent)</p>
          <p className="text-display-card font-bold text-ink-100">${cost.toFixed(2)}</p>
        </div>
        <div>
          <p className="text-mono-code uppercase text-ink-500">Reset @150K (max)</p>
          <p className="text-display-card font-bold text-health-healthy">{resetSavings === null ? "—" : `−${Math.round(resetSavings * 100)}%`}</p>
          {measured.length > 0 && <p className="text-mono-code text-ink-500">over {measured.length} recent sessions</p>}
        </div>
      </div>

      {(topSinks.length > 0 || byRepo.length > 1) && (
        <div className="mt-4 grid grid-cols-1 gap-4 border-t border-border-strong pt-4 sm:grid-cols-2">
          {topSinks.length > 0 && (
            <div>
              <p className="mb-1 text-mono-code uppercase text-ink-500">Biggest sinks (recent sessions)</p>
              <ul className="space-y-0.5 text-body text-ink-300">
                {topSinks.map(([name, tokens]) => (
                  <li key={name} className="flex justify-between gap-4">
                    <span className="font-mono text-mono-code">{name}</span>
                    <span>{Math.round((tokens / Math.max(measuredTotal, 1)) * 100)}%</span>
                  </li>
                ))}
              </ul>
            </div>
          )}
          {byRepo.length > 1 && (
            <div>
              <p className="mb-1 text-mono-code uppercase text-ink-500">By repository</p>
              <ul className="space-y-0.5 text-body text-ink-300">
                {byRepo.map((r) => (
                  <li key={r.id} className="flex justify-between gap-4">
                    <Link href={`/repositories/${encodeURIComponent(r.id)}`} className="truncate hover:text-ink-100">
                      {displayRepoName(r.id)}
                    </Link>
                    <span>{formatTokens(r.tokens)}</span>
                  </li>
                ))}
              </ul>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

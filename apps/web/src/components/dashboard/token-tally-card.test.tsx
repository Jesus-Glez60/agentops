import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { TokenTallyCard } from "@/components/dashboard/token-tally-card";
import type { SessionReportSummary, UsageSummary } from "@/lib/api/repos-api";

const zero = { input_tokens: 0, output_tokens: 0, cache_read_tokens: 0, cache_write_tokens: 0, cost_usd: 0 };

function report(overrides: Partial<SessionReportSummary>): SessionReportSummary {
  return { session_id: "s", source: "transcript", agent: "claude-code", calls: 1, tasks: 1, peak_context: 0, total_context_tokens: 0, compactions: 0, top_categories: [], resets: [], ...overrides };
}

function summary(repo: string, cacheRead: number, overrides: Partial<UsageSummary> = {}): UsageSummary {
  return {
    repo,
    tokens: { ...zero, cache_read_tokens: cacheRead, cost_usd: cacheRead / 1_000_000 },
    subagent_tokens: zero,
    sessions: [],
    session_count: 0,
    hit_count: 0,
    estimated_tokens_saved: 0,
    estimated_cost_saved_usd: 0,
    llm_spend: [],
    ...overrides,
  };
}

describe("TokenTallyCard", () => {
  it("sums tokens and sessions across repos, links by connection id, and aggregates measured sinks", () => {
    render(
      <TokenTallyCard
        usage={[
          {
            connectionId: "Jesus-Glez60--agentops",
            summary: summary("Jesus-Glez60--agentops--49c2f2401c26a680cead915491c1b933", 2_000_000_000, {
              session_count: 18,
              reports: [
                report({ total_context_tokens: 100, top_categories: [["reminders", 40], ["tool:Bash", 30]], resets: [{ threshold: 150_000, resets: 1, simulated_tokens: 40, savings: 0.6 }] }),
                // Agent estimates never feed the measured aggregate.
                report({ source: "agent", total_context_tokens: 1_000, top_categories: [["tool:Bash", 1_000]] }),
              ],
            }),
          },
          { connectionId: "Jesus-Glez60--job-hunter", summary: summary("x", 500_000_000, { session_count: 2 }) },
        ]}
      />,
    );
    expect(screen.getByText("2.50B")).toBeInTheDocument();
    expect(screen.getByText("20")).toBeInTheDocument();
    expect(screen.getByText("125.0M per session")).toBeInTheDocument();
    expect(screen.getByText("−60%")).toBeInTheDocument();
    expect(screen.getByText("40%")).toBeInTheDocument(); // reminders, not inflated by the agent row
    expect(screen.getByRole("link", { name: "Jesus-Glez60--agentops" })).toHaveAttribute("href", "/repositories/Jesus-Glez60--agentops");
  });

  it("renders nothing until any usage has been synced", () => {
    const { container } = render(<TokenTallyCard usage={[{ connectionId: "a", summary: summary("a", 0) }]} />);
    expect(container).toBeEmptyDOMElement();
  });
});

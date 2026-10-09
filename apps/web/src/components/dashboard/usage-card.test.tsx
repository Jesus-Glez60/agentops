import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { TooltipProvider } from "@/components/ui/tooltip";
import { UsageCard } from "@/components/dashboard/usage-card";
import type { LlmSpend, UsageSummary } from "@/lib/api/repos-api";

function spend(overrides: Partial<LlmSpend>): LlmSpend {
  return {
    operation: "explain_symbol",
    provider: "anthropic",
    model: "claude-sonnet-5",
    calls: 3,
    failures: 0,
    input_tokens: 1200,
    output_tokens: 300,
    cost_estimate_usd: 0.42,
    cost_partial: false,
    ...overrides,
  };
}

function summary(llm_spend: LlmSpend[], withSessions: boolean): UsageSummary {
  return {
    repo: "demo",
    tokens: { input_tokens: withSessions ? 1000 : 0, output_tokens: withSessions ? 500 : 0, cache_read_tokens: 0, cache_write_tokens: 0, cost_usd: withSessions ? 1 : 0 },
    subagent_tokens: { input_tokens: 0, output_tokens: 0, cache_read_tokens: 0, cache_write_tokens: 0, cost_usd: 0 },
    sessions: [],
    hit_count: 0,
    estimated_tokens_saved: 0,
    estimated_cost_saved_usd: 0,
    llm_spend,
  };
}

function renderCard(usage: UsageSummary | null) {
  return render(
    <TooltipProvider>
      <UsageCard usage={usage} apiUrl="https://api.example.test" />
    </TooltipProvider>,
  );
}

describe("UsageCard model spend", () => {
  it("shows AgentOps' own model spend even when no session usage has been synced", () => {
    renderCard(summary([spend({})], false));
    expect(screen.getByText(/No usage data synced yet/)).toBeInTheDocument();
    expect(screen.getByText("explain_symbol")).toBeInTheDocument();
    expect(screen.getByText("$0.42")).toBeInTheDocument();
  });

  it("never renders an unpriced or partially priced group as a complete dollar figure", () => {
    renderCard(
      summary(
        [
          spend({ operation: "librarian_classify", provider: "groq", model: "llama", cost_estimate_usd: null, cost_partial: true }),
          spend({ operation: "classify_note", cost_estimate_usd: 0.1, cost_partial: true, failures: 2 }),
        ],
        true,
      ),
    );
    expect(screen.getByText("—")).toBeInTheDocument();
    expect(screen.getByText("≥ $0.10 (partial)")).toBeInTheDocument();
    expect(screen.getByText(/2 failed/)).toBeInTheDocument();
  });

  it("omits the section entirely when there's no recorded model spend", () => {
    renderCard(summary([], true));
    expect(screen.queryByText(/AgentOps model spend/)).not.toBeInTheDocument();
  });

  it("lists recent sessions with subagent tokens and peak context kept apart from the totals", () => {
    const usage = summary([], true);
    usage.subagent_tokens = { input_tokens: 0, output_tokens: 0, cache_read_tokens: 4_087_042, cache_write_tokens: 0, cost_usd: 0 };
    usage.sessions = [
      {
        session_id: "sess-new",
        started_at: "2026-10-09T00:00:00Z",
        ended_at: "2026-10-09T05:00:00Z",
        main_tokens: 40_735_961,
        subagent_tokens: 4_087_042,
        peak_context_tokens: 390_189,
        cost_usd: 31.5,
      },
    ];
    renderCard(usage);
    expect(screen.getByText("Recent sessions")).toBeInTheDocument();
    expect(screen.getByText("40.7M")).toBeInTheDocument();
    expect(screen.getByText("4.1M")).toBeInTheDocument();
    expect(screen.getByText("390.2K")).toBeInTheDocument();
    expect(screen.getByText(/4\.1M subagents/)).toBeInTheDocument();
  });
});

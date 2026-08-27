import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { NodeDetailSections } from "@/components/shared/node-detail-sections";
import type { NodeDetail } from "@/lib/api/repos-api";

function detailWithConnections(connected: NodeDetail["connected"]): NodeDetail {
  return {
    id: 1,
    kind: "Symbol",
    repo: "agentops",
    path: "src/auth/session.ts",
    name: "refreshSession",
    container: null,
    start_line: 42,
    end_line: 68,
    content: "function refreshSession() {}",
    connected,
    curated: true,
    prominence: "Full",
    curation_reason: null,
  };
}

const gotcha = { id: 2, kind: "Gotcha" as const, name: "Token pair gotcha", path: null, relation: "affects" };
const decision = { id: 3, kind: "Decision" as const, name: "Rotating refresh tokens", path: null, relation: "← affects" };
const symbolDep = { id: 4, kind: "Symbol" as const, name: "rotateTokenPair", path: "src/auth/rotate.ts", relation: "depends on" };

describe("NodeDetailSections", () => {
  it("defaults to a single flat 'Connected nodes' list when splitKnowledge is omitted", () => {
    render(<NodeDetailSections detail={detailWithConnections([gotcha, decision, symbolDep])} branch="main" onSelectConnected={vi.fn()} />);
    expect(screen.getByText("Connected nodes")).toBeInTheDocument();
    expect(screen.queryByText("Attached knowledge")).not.toBeInTheDocument();
    expect(screen.queryByText("Relationships")).not.toBeInTheDocument();
  });

  it("partitions connected nodes into Attached knowledge and Relationships when splitKnowledge is true", () => {
    render(<NodeDetailSections detail={detailWithConnections([gotcha, decision, symbolDep])} branch="main" onSelectConnected={vi.fn()} splitKnowledge />);
    expect(screen.getByText("Attached knowledge")).toBeInTheDocument();
    expect(screen.getByText("Relationships")).toBeInTheDocument();
    expect(screen.queryByText("Connected nodes")).not.toBeInTheDocument();
  });

  it("omits the Attached knowledge section when there are no Gotcha/Decision connections, even with splitKnowledge", () => {
    render(<NodeDetailSections detail={detailWithConnections([symbolDep])} branch="main" onSelectConnected={vi.fn()} splitKnowledge />);
    expect(screen.queryByText("Attached knowledge")).not.toBeInTheDocument();
    expect(screen.getByText("Relationships")).toBeInTheDocument();
  });

  it("omits the Relationships section when everything is knowledge, even with splitKnowledge", () => {
    render(<NodeDetailSections detail={detailWithConnections([gotcha])} branch="main" onSelectConnected={vi.fn()} splitKnowledge />);
    expect(screen.getByText("Attached knowledge")).toBeInTheDocument();
    expect(screen.queryByText("Relationships")).not.toBeInTheDocument();
  });

  it("renders nothing connection-related when connected is empty, regardless of splitKnowledge", () => {
    render(<NodeDetailSections detail={detailWithConnections([])} branch="main" onSelectConnected={vi.fn()} splitKnowledge />);
    expect(screen.queryByText("Attached knowledge")).not.toBeInTheDocument();
    expect(screen.queryByText("Relationships")).not.toBeInTheDocument();
    expect(screen.queryByText("Connected nodes")).not.toBeInTheDocument();
  });
});

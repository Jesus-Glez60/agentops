import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

const { pushMock } = vi.hoisted(() => ({ pushMock: vi.fn() }));
vi.mock("next/navigation", () => ({ useRouter: () => ({ push: pushMock }) }));

import { GraphDetailPanel } from "@/components/graph/graph-detail-panel";
import type { NodeDetail } from "@/lib/api/agentops-api";

const detail: NodeDetail = {
  id: 1,
  kind: "Symbol",
  repo: "agentops",
  path: "src/auth/session.ts",
  name: "refreshSession",
  container: null,
  start_line: 42,
  end_line: 68,
  content: "function refreshSession() {}",
  connected: [],
  curated: true,
  prominence: "Full",
  curation_reason: null,
};

describe("GraphDetailPanel", () => {
  it("shows a loading state when detail is undefined", () => {
    render(<GraphDetailPanel detail={undefined} branch={null} isSeed onSelectConnected={vi.fn()} onClose={vi.fn()} onCenter={vi.fn()} />);
    expect(screen.getAllByText("Loading…").length).toBeGreaterThan(0);
  });

  it("Search navigates to /search?q=<name>", () => {
    render(<GraphDetailPanel detail={detail} branch="main" isSeed onSelectConnected={vi.fn()} onClose={vi.fn()} onCenter={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: /Search/ }));
    expect(pushMock).toHaveBeenCalledWith("/search?q=refreshSession");
  });

  it("Docs navigates to /docs", () => {
    render(<GraphDetailPanel detail={detail} branch="main" isSeed onSelectConnected={vi.fn()} onClose={vi.fn()} onCenter={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: /Docs/ }));
    expect(pushMock).toHaveBeenCalledWith("/docs");
  });

  it("close button calls onClose", () => {
    const onClose = vi.fn();
    render(<GraphDetailPanel detail={detail} branch="main" isSeed onSelectConnected={vi.fn()} onClose={onClose} onCenter={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(onClose).toHaveBeenCalled();
  });

  it("hides Center here when the node is already the seed", () => {
    render(<GraphDetailPanel detail={detail} branch="main" isSeed onSelectConnected={vi.fn()} onClose={vi.fn()} onCenter={vi.fn()} />);
    expect(screen.queryByTitle("Center graph on this node")).not.toBeInTheDocument();
  });

  it("shows Center here and calls onCenter when the node is not the seed", () => {
    const onCenter = vi.fn();
    render(<GraphDetailPanel detail={detail} branch="main" isSeed={false} onSelectConnected={vi.fn()} onClose={vi.fn()} onCenter={onCenter} />);
    fireEvent.click(screen.getByTitle("Center graph on this node"));
    expect(onCenter).toHaveBeenCalled();
  });
});

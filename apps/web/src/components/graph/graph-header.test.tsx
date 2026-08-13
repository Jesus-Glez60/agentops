import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { GraphHeader } from "@/components/graph/graph-header";

describe("GraphHeader", () => {
  it("renders the seed label when given", () => {
    render(<GraphHeader seedLabel="refreshSession()" mode="local" onModeChange={vi.fn()} repo="agentops" onChangeRepo={vi.fn()} />);
    expect(screen.getByText("refreshSession()")).toBeInTheDocument();
  });

  it("shows 'all nodes' as the breadcrumb when there's no seed yet", () => {
    render(<GraphHeader seedLabel={null} mode={null} onModeChange={vi.fn()} repo="agentops" onChangeRepo={vi.fn()} />);
    expect(screen.getByText("all nodes")).toBeInTheDocument();
  });

  it("hides the mode tabs entirely when mode is null (whole-repo view)", () => {
    render(<GraphHeader seedLabel={null} mode={null} onModeChange={vi.fn()} repo="agentops" onChangeRepo={vi.fn()} />);
    expect(screen.queryByRole("button", { name: "Local graph" })).not.toBeInTheDocument();
  });

  it("marks the active mode tab via aria-pressed", () => {
    render(<GraphHeader seedLabel="x" mode="impact" onModeChange={vi.fn()} repo="agentops" onChangeRepo={vi.fn()} />);
    expect(screen.getByRole("button", { name: "Impact" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("button", { name: "Local graph" })).toHaveAttribute("aria-pressed", "false");
  });

  it("clicking a tab calls onModeChange with its mode", () => {
    const onModeChange = vi.fn();
    render(<GraphHeader seedLabel="x" mode="local" onModeChange={onModeChange} repo="agentops" onChangeRepo={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "Dep. chain" }));
    expect(onModeChange).toHaveBeenCalledWith("dep_chain");
  });

  it("shows the current repo name in the picker trigger", () => {
    render(<GraphHeader seedLabel="x" mode="local" onModeChange={vi.fn()} repo="agentops" onChangeRepo={vi.fn()} />);
    expect(screen.getByRole("button", { name: /agentops/ })).toBeInTheDocument();
  });
});

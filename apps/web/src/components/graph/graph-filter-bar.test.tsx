import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { GraphFilterBar } from "@/components/graph/graph-filter-bar";

describe("GraphFilterBar", () => {
  it("clicking a kind toggle button calls onToggleKind with that kind", () => {
    const onToggleKind = vi.fn();
    render(<GraphFilterBar kinds={[]} onToggleKind={onToggleKind} depth={2} onDepthChange={vi.fn()} showHotspots={false} onToggleHotspots={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: /Gotchas/ }));
    expect(onToggleKind).toHaveBeenCalledWith("Gotcha");
  });

  it("all kind toggles are pressed when kinds is empty (no filter)", () => {
    render(<GraphFilterBar kinds={[]} onToggleKind={vi.fn()} depth={2} onDepthChange={vi.fn()} showHotspots={false} onToggleHotspots={vi.fn()} />);
    ["Symbols", "Files", "Gotchas", "Decisions"].forEach((label) => {
      expect(screen.getByRole("button", { name: new RegExp(label) })).toHaveAttribute("aria-pressed", "true");
    });
  });

  it("only the selected kinds are pressed when kinds is non-empty", () => {
    render(<GraphFilterBar kinds={["Symbol"]} onToggleKind={vi.fn()} depth={2} onDepthChange={vi.fn()} showHotspots={false} onToggleHotspots={vi.fn()} />);
    expect(screen.getByRole("button", { name: /Symbols/ })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("button", { name: /Files/ })).toHaveAttribute("aria-pressed", "false");
  });

  it("shows the current depth value", () => {
    render(<GraphFilterBar kinds={[]} onToggleKind={vi.fn()} depth={3} onDepthChange={vi.fn()} showHotspots={false} onToggleHotspots={vi.fn()} />);
    expect(screen.getByText("3")).toBeInTheDocument();
  });

  it("hides the depth slider when showDepth is false", () => {
    render(<GraphFilterBar kinds={[]} onToggleKind={vi.fn()} depth={2} onDepthChange={vi.fn()} showDepth={false} showHotspots={false} onToggleHotspots={vi.fn()} />);
    expect(screen.queryByText("Depth")).not.toBeInTheDocument();
  });

  it("checking the hotspots checkbox calls onToggleHotspots", () => {
    const onToggleHotspots = vi.fn();
    render(<GraphFilterBar kinds={[]} onToggleKind={vi.fn()} depth={2} onDepthChange={vi.fn()} showHotspots={false} onToggleHotspots={onToggleHotspots} />);
    fireEvent.click(screen.getByText("Hotspots"));
    expect(onToggleHotspots).toHaveBeenCalled();
  });

  it("the hotspots checkbox reflects showHotspots", () => {
    render(<GraphFilterBar kinds={[]} onToggleKind={vi.fn()} depth={2} onDepthChange={vi.fn()} showHotspots={true} onToggleHotspots={vi.fn()} />);
    const row = screen.getByText("Hotspots").closest("label")!;
    expect(row.querySelector('[role="checkbox"]')).toHaveAttribute("data-state", "checked");
  });

  it("shows live counts next to a kind label when provided", () => {
    render(<GraphFilterBar kinds={[]} onToggleKind={vi.fn()} kindCounts={{ Symbol: 42 }} depth={2} onDepthChange={vi.fn()} showHotspots={false} onToggleHotspots={vi.fn()} />);
    expect(screen.getByRole("button", { name: /Symbols.*42/ })).toBeInTheDocument();
  });
});

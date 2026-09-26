import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { GraphFilterPanel } from "@/components/graph/graph-filter-panel";

describe("GraphFilterPanel", () => {
  it("checking a kind checkbox calls onToggleKind with that kind", () => {
    const onToggleKind = vi.fn();
    render(<GraphFilterPanel kinds={[]} onToggleKind={onToggleKind} depth={2} onDepthChange={vi.fn()} showHotspots={false} onToggleHotspots={vi.fn()} />);
    fireEvent.click(screen.getByText("Gotchas"));
    expect(onToggleKind).toHaveBeenCalledWith("Gotcha");
  });

  it("all kind checkboxes are checked when kinds is empty (no filter)", () => {
    render(<GraphFilterPanel kinds={[]} onToggleKind={vi.fn()} depth={2} onDepthChange={vi.fn()} showHotspots={false} onToggleHotspots={vi.fn()} />);
    const kindLabels = ["Symbols", "Files", "Gotchas", "Decisions"];
    kindLabels.forEach((label) => {
      const row = screen.getByText(label).closest("label")!;
      expect(row.querySelector('[role="checkbox"]')).toHaveAttribute("data-state", "checked");
    });
  });

  it("only the selected kinds are checked when kinds is non-empty", () => {
    render(<GraphFilterPanel kinds={["Symbol"]} onToggleKind={vi.fn()} depth={2} onDepthChange={vi.fn()} showHotspots={false} onToggleHotspots={vi.fn()} />);
    const symbolRow = screen.getByText("Symbols").closest("label")!;
    const fileRow = screen.getByText("Files").closest("label")!;
    expect(symbolRow.querySelector('[role="checkbox"]')).toHaveAttribute("data-state", "checked");
    expect(fileRow.querySelector('[role="checkbox"]')).toHaveAttribute("data-state", "unchecked");
  });

  it("shows the current depth value", () => {
    render(<GraphFilterPanel kinds={[]} onToggleKind={vi.fn()} depth={3} onDepthChange={vi.fn()} showHotspots={false} onToggleHotspots={vi.fn()} />);
    expect(screen.getByText("3")).toBeInTheDocument();
  });

  it("hides the depth slider when showDepth is false", () => {
    render(<GraphFilterPanel kinds={[]} onToggleKind={vi.fn()} depth={2} onDepthChange={vi.fn()} showDepth={false} showHotspots={false} onToggleHotspots={vi.fn()} />);
    expect(screen.queryByText("Depth")).not.toBeInTheDocument();
  });

  it("checking the hotspots checkbox calls onToggleHotspots", () => {
    const onToggleHotspots = vi.fn();
    render(<GraphFilterPanel kinds={[]} onToggleKind={vi.fn()} depth={2} onDepthChange={vi.fn()} showHotspots={false} onToggleHotspots={onToggleHotspots} />);
    fireEvent.click(screen.getByText("Hotspots"));
    expect(onToggleHotspots).toHaveBeenCalled();
  });

  it("the hotspots checkbox reflects showHotspots", () => {
    render(<GraphFilterPanel kinds={[]} onToggleKind={vi.fn()} depth={2} onDepthChange={vi.fn()} showHotspots={true} onToggleHotspots={vi.fn()} />);
    const row = screen.getByText("Hotspots").closest("label")!;
    expect(row.querySelector('[role="checkbox"]')).toHaveAttribute("data-state", "checked");
  });
});

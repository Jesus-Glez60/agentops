import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { GraphFilterPanel } from "@/components/graph/graph-filter-panel";

describe("GraphFilterPanel", () => {
  it("checking a kind checkbox calls onToggleKind with that kind", () => {
    const onToggleKind = vi.fn();
    render(<GraphFilterPanel kinds={[]} onToggleKind={onToggleKind} depth={2} onDepthChange={vi.fn()} />);
    fireEvent.click(screen.getByText("Gotchas"));
    expect(onToggleKind).toHaveBeenCalledWith("Gotcha");
  });

  it("all checkboxes are checked when kinds is empty (no filter)", () => {
    render(<GraphFilterPanel kinds={[]} onToggleKind={vi.fn()} depth={2} onDepthChange={vi.fn()} />);
    const checkboxes = screen.getAllByRole("checkbox");
    checkboxes.forEach((cb) => expect(cb).toHaveAttribute("data-state", "checked"));
  });

  it("only the selected kinds are checked when kinds is non-empty", () => {
    render(<GraphFilterPanel kinds={["Symbol"]} onToggleKind={vi.fn()} depth={2} onDepthChange={vi.fn()} />);
    const symbolRow = screen.getByText("Symbols").closest("label")!;
    const fileRow = screen.getByText("Files").closest("label")!;
    expect(symbolRow.querySelector('[role="checkbox"]')).toHaveAttribute("data-state", "checked");
    expect(fileRow.querySelector('[role="checkbox"]')).toHaveAttribute("data-state", "unchecked");
  });

  it("shows the current depth value", () => {
    render(<GraphFilterPanel kinds={[]} onToggleKind={vi.fn()} depth={3} onDepthChange={vi.fn()} />);
    expect(screen.getByText("3")).toBeInTheDocument();
  });

  it("hides the depth slider when showDepth is false", () => {
    render(<GraphFilterPanel kinds={[]} onToggleKind={vi.fn()} depth={2} onDepthChange={vi.fn()} showDepth={false} />);
    expect(screen.queryByText("Depth")).not.toBeInTheDocument();
  });
});

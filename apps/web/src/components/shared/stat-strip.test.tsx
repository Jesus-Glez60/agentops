import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { StatStrip } from "@/components/shared/stat-strip";

describe("StatStrip", () => {
  it("renders every item's label and value", () => {
    render(<StatStrip items={[{ label: "Repositories", value: 4 }, { label: "Knowledge nodes", value: "2,549" }]} />);
    expect(screen.getByText("Repositories")).toBeInTheDocument();
    expect(screen.getByText("4")).toBeInTheDocument();
    expect(screen.getByText("Knowledge nodes")).toBeInTheDocument();
    expect(screen.getByText("2,549")).toBeInTheDocument();
  });

  it("renders an optional note line", () => {
    render(<StatStrip items={[{ label: "Knowledge nodes", value: 10, note: "symbols, files, gotchas, decisions" }]} />);
    expect(screen.getByText("symbols, files, gotchas, decisions")).toBeInTheDocument();
  });

  it("makes a cell a real link when href is set", () => {
    render(<StatStrip items={[{ label: "Gotchas needing curation", value: 3, href: "/gotchas" }]} />);
    expect(screen.getByRole("link", { name: /Gotchas needing curation/ })).toHaveAttribute("href", "/gotchas");
  });
});

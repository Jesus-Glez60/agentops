import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { NodeCountBar } from "@/components/dashboard/node-count-bar";

describe("NodeCountBar", () => {
  it("renders one segment per non-zero count", () => {
    const { container } = render(<NodeCountBar counts={{ symbols: 3, files: 1, gotchas: 0, gotchas_needing_curation: 0, decisions: 2 }} />);
    // 3 non-zero segments (gotchas is 0, skipped) as direct children of the bar.
    expect(container.firstElementChild?.children.length).toBe(3);
  });

  it("renders a flat empty bar (no segments, no divide-by-zero) when total is 0", () => {
    const { container } = render(<NodeCountBar counts={{ symbols: 0, files: 0, gotchas: 0, gotchas_needing_curation: 0, decisions: 0 }} />);
    expect(container.firstElementChild?.children.length).toBe(0);
  });
});

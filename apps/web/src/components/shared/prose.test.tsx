import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { Prose } from "./prose";

describe("Prose", () => {
  it("renders **bold** as a strong element, not literal asterisks", () => {
    render(<Prose text="**Not fixed here** -- out of scope." />);
    expect(screen.getByText("Not fixed here").tagName).toBe("STRONG");
    expect(screen.queryByText(/\*\*/)).not.toBeInTheDocument();
  });

  it("renders `code` spans as code elements, not literal backticks", () => {
    render(<Prose text="Calls `agentops_mcp::open_store` directly." />);
    expect(screen.getByText("agentops_mcp::open_store").tagName).toBe("CODE");
  });

  it("splits double newlines into separate paragraphs", () => {
    const { container } = render(<Prose text={"First paragraph.\n\nSecond paragraph."} />);
    expect(container.querySelectorAll("p")).toHaveLength(2);
  });
});

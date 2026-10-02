import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { SearchResultCard } from "./search-result-card";

describe("SearchResultCard", () => {
  it("renders the kind, title, snippet, and a relevance label for a strong score", () => {
    render(<SearchResultCard kind="Symbol" kindLabel="Symbol" title="verify_token" snippet="Validates a JWT" score={0.94} selected={false} onClick={vi.fn()} />);
    expect(screen.getByText("Symbol")).toBeInTheDocument();
    expect(screen.getByText("verify_token")).toBeInTheDocument();
    expect(screen.getByText("Validates a JWT")).toBeInTheDocument();
    expect(screen.getByText("Strong match")).toBeInTheDocument();
  });

  it("shows a supporting-context label for a low score", () => {
    render(<SearchResultCard kind="File" kindLabel="File" title="auth.py" snippet="..." score={0.1} selected={false} onClick={vi.fn()} />);
    expect(screen.getByText("Supporting context")).toBeInTheDocument();
  });

  it("calls onClick when clicked", () => {
    const onClick = vi.fn();
    render(<SearchResultCard kind="Gotcha" kindLabel="Gotcha" title="Token bug" snippet="..." score={0.5} selected={false} onClick={onClick} />);
    fireEvent.click(screen.getByRole("button"));
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  it("colors the kind label using the gotcha token, not a generic grey", () => {
    render(<SearchResultCard kind="Gotcha" kindLabel="Gotcha" title="Token bug" snippet="..." score={0.5} selected={false} onClick={vi.fn()} />);
    expect(screen.getByText("Gotcha")).toHaveStyle({ color: "var(--node-gotcha)" });
  });

  it("colors the kind label using the decision token", () => {
    render(<SearchResultCard kind="Decision" kindLabel="Decision" title="Use rotating refresh tokens" snippet="..." score={0.5} selected={false} onClick={vi.fn()} />);
    expect(screen.getByText("Decision")).toHaveStyle({ color: "var(--node-decision)" });
  });

  it("colors the kind label using the note/docs token when the label is remapped to Docs", () => {
    render(<SearchResultCard kind="Note" kindLabel="Docs" title="README" snippet="..." score={0.5} selected={false} onClick={vi.fn()} />);
    expect(screen.getByText("Docs")).toHaveStyle({ color: "var(--node-note)" });
  });

  it("renders a location caption when given", () => {
    render(<SearchResultCard kind="Symbol" kindLabel="Symbol" title="verify_token" snippet="..." score={0.5} location="L42-58" selected={false} onClick={vi.fn()} />);
    expect(screen.getByText("L42-58")).toBeInTheDocument();
  });

  it("omits the location caption when not given", () => {
    render(<SearchResultCard kind="Symbol" kindLabel="Symbol" title="verify_token" snippet="..." score={0.5} selected={false} onClick={vi.fn()} />);
    expect(screen.queryByText(/^L\d/)).not.toBeInTheDocument();
  });
});

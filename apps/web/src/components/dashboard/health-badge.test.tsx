import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { HealthBadge } from "@/components/dashboard/health-badge";

describe("HealthBadge", () => {
  it.each([
    ["healthy", "Healthy"],
    ["warning", "Warning"],
    ["stale", "Stale"],
    ["not-indexed", "Not yet scanned"],
  ] as const)("renders the right label for %s", (status, label) => {
    render(<HealthBadge status={status} />);
    expect(screen.getByText(label)).toBeInTheDocument();
  });
});

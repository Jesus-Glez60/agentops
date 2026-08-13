import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { Database } from "lucide-react";
import { StatCard } from "@/components/dashboard/stat-card";

describe("StatCard", () => {
  it("renders the label and value", () => {
    render(<StatCard label="Repositories" value={4} icon={Database} />);
    expect(screen.getByText("Repositories")).toBeInTheDocument();
    expect(screen.getByText("4")).toBeInTheDocument();
  });
});

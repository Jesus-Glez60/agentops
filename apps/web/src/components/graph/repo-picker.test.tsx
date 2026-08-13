import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { SWRConfig } from "swr";

const { getRepos } = vi.hoisted(() => ({ getRepos: vi.fn() }));
vi.mock("@/lib/api/agentops-api", async () => {
  const actual = await vi.importActual<typeof import("@/lib/api/agentops-api")>("@/lib/api/agentops-api");
  return { ...actual, getRepos };
});

import { RepoPicker } from "@/components/graph/repo-picker";

function renderPicker(onSelect: (repo: string) => void) {
  return render(
    <SWRConfig value={{ provider: () => new Map() }}>
      <RepoPicker onSelect={onSelect} />
    </SWRConfig>,
  );
}

describe("RepoPicker", () => {
  it("lists scanned repos and calls onSelect when clicked", async () => {
    getRepos.mockResolvedValue([{ name: "agentops", path: "/x", branch: "main", last_scanned_at: 0, counts: null, path_missing: false }]);
    const onSelect = vi.fn();
    renderPicker(onSelect);

    fireEvent.click(await screen.findByText("agentops"));
    expect(onSelect).toHaveBeenCalledWith("agentops");
  });

  it("shows an empty message when no repos are scanned", async () => {
    getRepos.mockResolvedValue([]);
    renderPicker(vi.fn());
    expect(await screen.findByText("No repositories scanned yet.")).toBeInTheDocument();
  });
});

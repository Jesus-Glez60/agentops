import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { SWRConfig } from "swr";

const { getRepos } = vi.hoisted(() => ({ getRepos: vi.fn() }));
vi.mock("@/lib/api/repos-api", async () => {
  const actual = await vi.importActual<typeof import("@/lib/api/repos-api")>("@/lib/api/repos-api");
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
    getRepos.mockResolvedValue({ connections: [{ id: "agentops", tenant: "t", repo_url: "git@github.com:acme/agentops.git", method: "ssh", public_key_openssh: null, status: "active", created_at: "", branch: "main", counts: null, path_missing: false }], can_connect: true });
    const onSelect = vi.fn();
    renderPicker(onSelect);

    fireEvent.click(await screen.findByText("agentops"));
    expect(onSelect).toHaveBeenCalledWith("agentops");
  });

  it("shows an empty message when no repos are scanned", async () => {
    getRepos.mockResolvedValue({ connections: [], can_connect: true });
    renderPicker(vi.fn());
    expect(await screen.findByText("No repositories scanned yet.")).toBeInTheDocument();
  });
});

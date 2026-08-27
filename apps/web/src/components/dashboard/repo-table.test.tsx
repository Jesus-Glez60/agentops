import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { SWRConfig } from "swr";

const { getRepos, startIndexing, toastError, toastSuccess } = vi.hoisted(() => ({
  getRepos: vi.fn(),
  startIndexing: vi.fn(),
  toastError: vi.fn(),
  toastSuccess: vi.fn(),
}));

vi.mock("@/lib/api/repos-api", async () => {
  const actual = await vi.importActual<typeof import("@/lib/api/repos-api")>("@/lib/api/repos-api");
  return { ...actual, getRepos, startIndexing };
});

vi.mock("sonner", () => ({
  toast: { error: toastError, success: toastSuccess },
}));

import { TooltipProvider } from "@/components/ui/tooltip";
import { RepoTable } from "@/components/dashboard/repo-table";

function renderTable() {
  return render(
    // A fresh SWRConfig per test resets the cache -- without it, repos
    // fetched in one test would leak into the next via SWR's module-level
    // default cache.
    <SWRConfig value={{ provider: () => new Map() }}>
      <TooltipProvider>
        <RepoTable />
      </TooltipProvider>
    </SWRConfig>,
  );
}

const scannedRepo = {
  id: "agentops",
  tenant: "acme",
  repo_url: "git@github.com:acme/agentops.git",
  method: "ssh",
  public_key_openssh: null,
  status: "active",
  created_at: "2026-01-01T00:00:00Z",
  branch: "main",
  counts: { symbols: 10, files: 5, gotchas: 1, gotchas_needing_curation: 1, decisions: 2 },
  path_missing: false,
};

const unscannedRepo = {
  id: "fresh-clone",
  tenant: "acme",
  repo_url: "git@github.com:acme/fresh-clone.git",
  method: "ssh",
  public_key_openssh: null,
  status: "active",
  created_at: "2026-01-01T00:00:00Z",
  branch: "main",
  counts: null,
  path_missing: false,
};

describe("RepoTable", () => {
  beforeEach(() => {
    getRepos.mockReset();
    startIndexing.mockReset();
    toastError.mockClear();
    toastSuccess.mockClear();
  });

  it("renders a row per repo with health/branch/status", async () => {
    getRepos.mockResolvedValue({ connections: [scannedRepo], can_connect: true });
    renderTable();

    expect(await screen.findByText("agentops")).toBeInTheDocument();
    expect(screen.getByText("main")).toBeInTheDocument();
    expect(screen.getByText("Warning")).toBeInTheDocument(); // has 1 gotcha, recently scanned
  });

  it("renders 'not yet scanned' instead of fabricated zeros when counts is null", async () => {
    getRepos.mockResolvedValue({ connections: [unscannedRepo], can_connect: true });
    renderTable();

    expect(await screen.findByText("fresh-clone")).toBeInTheDocument();
    expect(screen.getByText("not yet scanned")).toBeInTheDocument();
    expect(screen.getByText("Not yet scanned")).toBeInTheDocument(); // health badge
  });

  it("shows an empty state when there are no connected repos", async () => {
    getRepos.mockResolvedValue({ connections: [], can_connect: true });
    renderTable();
    expect(await screen.findByText(/No repositories connected yet/)).toBeInTheDocument();
  });

  it("rescan button starts a reindex job and shows a scanning state while in flight", async () => {
    getRepos.mockResolvedValue({ connections: [scannedRepo], can_connect: true });
    let resolveReindex!: (value: { job_id: string }) => void;
    startIndexing.mockReturnValue(new Promise((resolve) => (resolveReindex = resolve)));
    renderTable();

    await screen.findByText("agentops");
    fireEvent.click(screen.getByRole("button", { name: "Rescan repository" }));

    expect(await screen.findByText("Scanning…")).toBeInTheDocument();
    resolveReindex({ job_id: "job-1" });

    await waitFor(() => expect(screen.queryByText("Scanning…")).not.toBeInTheDocument());
    expect(startIndexing).toHaveBeenCalledWith("agentops", "reindex");
  });

  it("the 'view details' action is present but disabled -- no repo-detail page exists yet", async () => {
    getRepos.mockResolvedValue({ connections: [scannedRepo], can_connect: true });
    renderTable();
    await screen.findByText("agentops");
    expect(screen.getByRole("button", { name: "View details" })).toBeDisabled();
  });
});

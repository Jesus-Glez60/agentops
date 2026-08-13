import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { SWRConfig } from "swr";

const { getRepos, rescanRepo, toastError } = vi.hoisted(() => ({
  getRepos: vi.fn(),
  rescanRepo: vi.fn(),
  toastError: vi.fn(),
}));

vi.mock("@/lib/api/agentops-api", async () => {
  const actual = await vi.importActual<typeof import("@/lib/api/agentops-api")>("@/lib/api/agentops-api");
  return { ...actual, getRepos, rescanRepo };
});

vi.mock("sonner", () => ({
  toast: { error: toastError },
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
  name: "agentops",
  path: "/repos/agentops",
  branch: "main",
  last_scanned_at: Math.floor(Date.now() / 1000),
  counts: { symbols: 10, files: 5, gotchas: 1, gotchas_needing_curation: 1, decisions: 2 },
  path_missing: false,
};

const unscannedRepo = {
  name: "fresh-clone",
  path: "/repos/fresh-clone",
  branch: "main",
  last_scanned_at: Math.floor(Date.now() / 1000),
  counts: null,
  path_missing: false,
};

describe("RepoTable", () => {
  beforeEach(() => {
    getRepos.mockReset();
    rescanRepo.mockReset();
    toastError.mockClear();
  });

  it("renders a row per repo with health/branch/last-scan", async () => {
    getRepos.mockResolvedValue([scannedRepo]);
    renderTable();

    expect(await screen.findByText("agentops")).toBeInTheDocument();
    expect(screen.getByText("main")).toBeInTheDocument();
    expect(screen.getByText("Warning")).toBeInTheDocument(); // has 1 gotcha, recently scanned
  });

  it("renders 'not yet scanned' instead of fabricated zeros when counts is null", async () => {
    getRepos.mockResolvedValue([unscannedRepo]);
    renderTable();

    expect(await screen.findByText("fresh-clone")).toBeInTheDocument();
    expect(screen.getByText("not yet scanned")).toBeInTheDocument();
    expect(screen.getByText("Not yet scanned")).toBeInTheDocument(); // health badge
  });

  it("shows an empty state when there are no scanned repos", async () => {
    getRepos.mockResolvedValue([]);
    renderTable();
    expect(await screen.findByText(/No repositories scanned yet/)).toBeInTheDocument();
  });

  it("rescan button calls rescanRepo and shows a scanning state while in flight", async () => {
    getRepos.mockResolvedValue([scannedRepo]);
    let resolveRescan!: (value: typeof scannedRepo) => void;
    rescanRepo.mockReturnValue(new Promise((resolve) => (resolveRescan = resolve)));
    renderTable();

    await screen.findByText("agentops");
    fireEvent.click(screen.getByRole("button", { name: "Rescan repository" }));

    expect(await screen.findByText("Scanning…")).toBeInTheDocument();
    resolveRescan({ ...scannedRepo, counts: { ...scannedRepo.counts, files: 6 } });

    await waitFor(() => expect(screen.queryByText("Scanning…")).not.toBeInTheDocument());
    expect(rescanRepo).toHaveBeenCalledWith("agentops");
  });

  it("the 'view details' action is present but disabled -- no repo-detail page exists yet", async () => {
    getRepos.mockResolvedValue([scannedRepo]);
    renderTable();
    await screen.findByText("agentops");
    expect(screen.getByRole("button", { name: "View details" })).toBeDisabled();
  });
});

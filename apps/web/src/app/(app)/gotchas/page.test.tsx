import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { SWRConfig } from "swr";

const { getGotchas, getRepos, setCuration, getNodeDetail, toastError } = vi.hoisted(() => ({
  getGotchas: vi.fn(),
  getRepos: vi.fn(),
  setCuration: vi.fn(),
  getNodeDetail: vi.fn(),
  toastError: vi.fn(),
}));

vi.mock("@/lib/api/agentops-api", async () => {
  const actual = await vi.importActual<typeof import("@/lib/api/agentops-api")>("@/lib/api/agentops-api");
  return { ...actual, getGotchas, getRepos, setCuration, getNodeDetail };
});

vi.mock("sonner", () => ({
  toast: { error: toastError },
}));

import GotchasPage from "./page";

function renderPage() {
  return render(
    <SWRConfig value={{ provider: () => new Map() }}>
      <GotchasPage />
    </SWRConfig>,
  );
}

const needsCurationGotcha = {
  repo: "agentops",
  id: 1,
  name: "Unbounded retry loop",
  path: "src/retry.py",
  container: null,
  start_line: null,
  end_line: null,
  snippet: "Retries forever on 5xx.",
  curated: false,
  prominence: "Full" as const,
  curation_reason: null,
};

const reducedGotcha = {
  ...needsCurationGotcha,
  id: 2,
  name: "Niche old-Linux workaround",
  curated: true,
  prominence: "Reduced" as const,
  curation_reason: "only affects old Linux envs",
};

function nodeDetailFor(gotcha: typeof needsCurationGotcha | typeof reducedGotcha) {
  return {
    id: gotcha.id,
    kind: "Gotcha",
    repo: gotcha.repo,
    path: gotcha.path,
    name: gotcha.name,
    container: null,
    start_line: null,
    end_line: null,
    content: gotcha.snippet,
    connected: [],
    curated: gotcha.curated,
    prominence: gotcha.prominence,
    curation_reason: gotcha.curation_reason,
  };
}

describe("GotchasPage", () => {
  beforeEach(() => {
    getGotchas.mockReset();
    getRepos.mockReset();
    setCuration.mockReset();
    getNodeDetail.mockReset();
    toastError.mockClear();
    getRepos.mockResolvedValue([]);
  });

  it("renders a card per gotcha with its curation bucket badge", async () => {
    getGotchas.mockResolvedValue([needsCurationGotcha, reducedGotcha]);
    renderPage();

    expect(await screen.findByText("Unbounded retry loop")).toBeInTheDocument();
    expect(screen.getByText("Niche old-Linux workaround")).toBeInTheDocument();
  });

  it("the Reduced tab filters out gotchas that still need curation", async () => {
    getGotchas.mockResolvedValue([needsCurationGotcha, reducedGotcha]);
    renderPage();
    await screen.findByText("Unbounded retry loop");

    fireEvent.click(screen.getByRole("button", { name: "Reduced" }));
    expect(screen.getByText("Niche old-Linux workaround")).toBeInTheDocument();
    expect(screen.queryByText("Unbounded retry loop")).not.toBeInTheDocument();
  });

  it("shows an empty state when nothing matches", async () => {
    getGotchas.mockResolvedValue([]);
    renderPage();
    expect(await screen.findByText("No gotchas")).toBeInTheDocument();
  });

  it("selecting a gotcha and clicking Keep calls setCuration with Full/null", async () => {
    getGotchas.mockResolvedValue([needsCurationGotcha]);
    getNodeDetail.mockResolvedValue(nodeDetailFor(needsCurationGotcha));
    setCuration.mockResolvedValue({ id: 1, prominence: "Full" });
    renderPage();

    fireEvent.click(await screen.findByText("Unbounded retry loop"));
    fireEvent.click(await screen.findByRole("button", { name: /Keep as permanent knowledge/ }));

    await waitFor(() => expect(setCuration).toHaveBeenCalledWith("agentops", 1, "Full", null));
  });

  it("Reduce prominence opens the reason dialog and requires non-empty text before submitting", async () => {
    getGotchas.mockResolvedValue([needsCurationGotcha]);
    getNodeDetail.mockResolvedValue(nodeDetailFor(needsCurationGotcha));
    setCuration.mockResolvedValue({ id: 1, prominence: "Reduced" });
    renderPage();

    fireEvent.click(await screen.findByText("Unbounded retry loop"));
    fireEvent.click(await screen.findByRole("button", { name: /Reduce prominence/ }));

    const saveButton = await screen.findByRole("button", { name: "Save" });
    expect(saveButton).toBeDisabled();

    fireEvent.change(await screen.findByPlaceholderText(/Only affects old Linux envs/), { target: { value: "niche edge case" } });
    expect(saveButton).not.toBeDisabled();
    fireEvent.click(saveButton);

    await waitFor(() => expect(setCuration).toHaveBeenCalledWith("agentops", 1, "Reduced", "niche edge case"));
  });

  it("a Reduced gotcha shows Restore full prominence and Edit reason instead of Reduce", async () => {
    getGotchas.mockResolvedValue([reducedGotcha]);
    getNodeDetail.mockResolvedValue(nodeDetailFor(reducedGotcha));
    renderPage();

    fireEvent.click(await screen.findByText("Niche old-Linux workaround"));
    expect(await screen.findByRole("button", { name: /Restore full prominence/ })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Edit reason/ })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^Reduce prominence/ })).not.toBeInTheDocument();
  });

  it("Keep all applies Full/null to every currently-filtered gotcha", async () => {
    const secondNeedsCuration = { ...needsCurationGotcha, id: 3, name: "Second gotcha" };
    getGotchas.mockResolvedValue([needsCurationGotcha, secondNeedsCuration, reducedGotcha]);
    setCuration.mockResolvedValue({ id: 1, prominence: "Full" });
    renderPage();
    await screen.findByText("Unbounded retry loop");

    // Scope to the two needing curation -- the bulk action must not touch
    // the already-Reduced one that's filtered out of view.
    fireEvent.click(screen.getByRole("button", { name: "Needs curation" }));
    fireEvent.click(await screen.findByRole("button", { name: "Keep all" }));

    await waitFor(() => {
      expect(setCuration).toHaveBeenCalledWith("agentops", 1, "Full", null);
      expect(setCuration).toHaveBeenCalledWith("agentops", 3, "Full", null);
    });
    expect(setCuration).not.toHaveBeenCalledWith("agentops", 2, "Full", null);
  });

  it("Reduce all opens the reason dialog and applies the same reason to every currently-filtered gotcha", async () => {
    const secondNeedsCuration = { ...needsCurationGotcha, id: 3, name: "Second gotcha" };
    getGotchas.mockResolvedValue([needsCurationGotcha, secondNeedsCuration]);
    setCuration.mockResolvedValue({ id: 1, prominence: "Reduced" });
    renderPage();
    await screen.findByText("Unbounded retry loop");

    fireEvent.click(await screen.findByRole("button", { name: "Reduce all" }));
    fireEvent.change(await screen.findByPlaceholderText(/Only affects old Linux envs/), { target: { value: "bulk niche reason" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => {
      expect(setCuration).toHaveBeenCalledWith("agentops", 1, "Reduced", "bulk niche reason");
      expect(setCuration).toHaveBeenCalledWith("agentops", 3, "Reduced", "bulk niche reason");
    });
  });

  it("Keep all and Reduce all are disabled when nothing matches the filter", async () => {
    getGotchas.mockResolvedValue([]);
    renderPage();
    expect(await screen.findByRole("button", { name: "Keep all" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Reduce all" })).toBeDisabled();
  });

  it("surfaces an error via toast if the curation change fails", async () => {
    getGotchas.mockResolvedValue([needsCurationGotcha]);
    getNodeDetail.mockResolvedValue(nodeDetailFor(needsCurationGotcha));
    setCuration.mockRejectedValue(new Error("network down"));
    renderPage();

    fireEvent.click(await screen.findByText("Unbounded retry loop"));
    fireEvent.click(await screen.findByRole("button", { name: /Keep as permanent knowledge/ }));

    await waitFor(() => expect(toastError).toHaveBeenCalledWith("network down"));
  });
});

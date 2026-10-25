import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { SWRConfig } from "swr";

const { getGotchas, getRepos, setCuration, setNodeKind, getNodeDetail, getProposals, decideProposal, toastError, toastSuccess } = vi.hoisted(() => ({
  getProposals: vi.fn(),
  decideProposal: vi.fn(),
  toastSuccess: vi.fn(),
  getGotchas: vi.fn(),
  getRepos: vi.fn(),
  setCuration: vi.fn(),
  setNodeKind: vi.fn(),
  getNodeDetail: vi.fn(),
  toastError: vi.fn(),
}));

vi.mock("@/lib/api/repos-api", async () => {
  const actual = await vi.importActual<typeof import("@/lib/api/repos-api")>("@/lib/api/repos-api");
  // The page loads `getCurationQueue`; here it is just the mocked list.
  return { ...actual, getGotchas, getCurationQueue: () => getGotchas(), getRepos, setCuration, setNodeKind, getNodeDetail, getProposals, decideProposal };
});

vi.mock("sonner", () => ({
  toast: { error: toastError, success: toastSuccess },
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
  kind: "Gotcha" as const,
};

const reducedGotcha = {
  ...needsCurationGotcha,
  id: 2,
  name: "Niche old-Linux workaround",
  curated: true,
  prominence: "Reduced" as const,
  curation_reason: "only affects old Linux envs",
};

// An imported memory nobody could classify: a note, waiting in Needs review.
const needsReviewNote = {
  ...needsCurationGotcha,
  id: 3,
  name: "Imported deploy memory",
  path: "vault:deploy.md",
  prominence: "NeedsReview" as const,
  curation_reason: "unclassified on import",
  kind: "Note" as const,
};

function nodeDetailFor(gotcha: { id: number; repo: string; path: string | null; name: string | null; snippet: string | null; curated: boolean; prominence: string; curation_reason: string | null; kind: string }) {
  return {
    id: gotcha.id,
    kind: gotcha.kind,
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
    setNodeKind.mockReset();
    getNodeDetail.mockReset();
    toastError.mockClear();
    getRepos.mockResolvedValue({ connections: [], can_connect: true });
    getProposals.mockReset();
    getProposals.mockResolvedValue([]);
    decideProposal.mockReset();
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

  it("a needs-review memory gets its own tab, badge and count, not Needs curation", async () => {
    getGotchas.mockResolvedValue([needsCurationGotcha, needsReviewNote]);
    renderPage();
    await screen.findByText("Imported deploy memory");

    fireEvent.click(screen.getByRole("button", { name: "Needs review" }));
    expect(screen.getByText("Imported deploy memory")).toBeInTheDocument();
    expect(screen.queryByText("Unbounded retry loop")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Needs curation" }));
    expect(screen.queryByText("Imported deploy memory")).not.toBeInTheDocument();
  });

  it("a needs-review memory shows its reason and can have its kind set and be accepted", async () => {
    getGotchas.mockResolvedValue([needsReviewNote]);
    getNodeDetail.mockResolvedValue(nodeDetailFor(needsReviewNote));
    setNodeKind.mockResolvedValue({ id: 3, kind: "gotcha" });
    setCuration.mockResolvedValue({ id: 3, prominence: "Full" });
    renderPage();

    fireEvent.click(await screen.findByText("Imported deploy memory"));
    expect(await screen.findByText("unclassified on import")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Knowledge" })).toHaveAttribute("aria-pressed", "true");
    fireEvent.click(screen.getByRole("button", { name: "Gotcha" }));
    await waitFor(() => expect(setNodeKind).toHaveBeenCalledWith("agentops", 3, "gotcha"));
    await waitFor(() => expect(screen.getByRole("button", { name: "Gotcha" })).toHaveAttribute("aria-pressed", "true"));

    fireEvent.click(screen.getByRole("button", { name: /Accept/ }));
    await waitFor(() => expect(setCuration).toHaveBeenCalledWith("agentops", 3, "Full", null));
  });

  it("a consolidation proposal shows its sources and is decided only by Accept proposal or Reject", async () => {
    const proposal = { ...needsReviewNote, id: 9, name: "The pool runs out under load", kind: "Gotcha" as const, curation_reason: "proposed by consolidation from 4, 5" };
    getGotchas.mockResolvedValue([proposal]);
    getNodeDetail.mockResolvedValue(nodeDetailFor(proposal));
    getProposals.mockResolvedValue([{ proposal_id: 9, created_at: "2026-10-24", sources: [{ id: 4, kind: "gotcha", title: "Pool exhaustion under load" }, { id: 5, kind: "gotcha", title: "Connection pool runs out" }] }]);
    decideProposal.mockResolvedValue({ proposal_id: 9, state: "accepted" });
    renderPage();

    fireEvent.click(await screen.findByText("The pool runs out under load"));
    expect(await screen.findByText(/Pool exhaustion under load/)).toBeInTheDocument();
    expect(screen.getByText(/Connection pool runs out/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Accept" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Pin" })).not.toBeInTheDocument();

    fireEvent.keyDown(document, { key: "k" });
    await waitFor(() => expect(toastError).toHaveBeenCalled());
    expect(setCuration).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Accept proposal" }));
    await waitFor(() => expect(decideProposal).toHaveBeenCalledWith("agentops", 9, "accept"));
  });

  it("Keep all skips consolidation proposals and says so, curating the rest", async () => {
    const proposal = { ...needsReviewNote, id: 9, name: "The pool runs out under load", curation_reason: "proposed by consolidation from 4, 5" };
    getGotchas.mockResolvedValue([proposal, needsCurationGotcha]);
    setCuration.mockResolvedValue({ id: 1, prominence: "Full" });
    renderPage();

    await screen.findByText("The pool runs out under load");
    fireEvent.click(screen.getByRole("button", { name: /Keep all/ }));
    await waitFor(() => expect(setCuration).toHaveBeenCalledTimes(1));
    expect(setCuration).toHaveBeenCalledWith("agentops", 1, "Full", null);
    expect(toastError).toHaveBeenCalledWith(expect.stringContaining("Skipped 1 consolidation proposal"));
  });

  it("rejecting a proposal removes it from the list", async () => {
    const proposal = { ...needsReviewNote, id: 9, name: "The pool runs out under load", kind: "Gotcha" as const };
    getGotchas.mockResolvedValue([proposal]);
    getNodeDetail.mockResolvedValue(nodeDetailFor(proposal));
    getProposals.mockResolvedValue([{ proposal_id: 9, created_at: "2026-10-24", sources: [{ id: 4, kind: "gotcha", title: "Pool exhaustion under load" }] }]);
    decideProposal.mockResolvedValue({ proposal_id: 9, state: "rejected" });
    renderPage();

    fireEvent.click(await screen.findByText("The pool runs out under load"));
    fireEvent.click(await screen.findByRole("button", { name: "Reject" }));
    await waitFor(() => expect(decideProposal).toHaveBeenCalledWith("agentops", 9, "reject"));
    await waitFor(() => expect(screen.queryByText("The pool runs out under load")).not.toBeInTheDocument());
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

  it("pressing K keeps the selected gotcha", async () => {
    getGotchas.mockResolvedValue([needsCurationGotcha]);
    getNodeDetail.mockResolvedValue(nodeDetailFor(needsCurationGotcha));
    setCuration.mockResolvedValue({ id: 1, prominence: "Full" });
    renderPage();

    fireEvent.click(await screen.findByText("Unbounded retry loop"));
    await screen.findByRole("button", { name: /Keep as permanent knowledge/ });
    fireEvent.keyDown(document, { key: "k" });

    await waitFor(() => expect(setCuration).toHaveBeenCalledWith("agentops", 1, "Full", null));
  });

  it("pressing P pins the selected gotcha", async () => {
    getGotchas.mockResolvedValue([needsCurationGotcha]);
    getNodeDetail.mockResolvedValue(nodeDetailFor(needsCurationGotcha));
    setCuration.mockResolvedValue({ id: 1, prominence: "Pinned" });
    renderPage();

    fireEvent.click(await screen.findByText("Unbounded retry loop"));
    await screen.findByRole("button", { name: /Keep as permanent knowledge/ });
    fireEvent.keyDown(document, { key: "p" });

    await waitFor(() => expect(setCuration).toHaveBeenCalledWith("agentops", 1, "Pinned", null));
  });

  it("pressing R opens the reduce-reason dialog for the selected gotcha", async () => {
    getGotchas.mockResolvedValue([needsCurationGotcha]);
    getNodeDetail.mockResolvedValue(nodeDetailFor(needsCurationGotcha));
    renderPage();

    fireEvent.click(await screen.findByText("Unbounded retry loop"));
    await screen.findByRole("button", { name: /Keep as permanent knowledge/ });
    fireEvent.keyDown(document, { key: "r" });

    expect(await screen.findByRole("button", { name: "Save" })).toBeInTheDocument();
  });

  it("pressing ArrowDown/ArrowUp moves the selection in the filtered list", async () => {
    const secondNeedsCuration = { ...needsCurationGotcha, id: 3, name: "Second gotcha" };
    getGotchas.mockResolvedValue([needsCurationGotcha, secondNeedsCuration]);
    getNodeDetail.mockImplementation((repo: string, id: number) => Promise.resolve(nodeDetailFor(id === 1 ? needsCurationGotcha : secondNeedsCuration)));
    renderPage();

    fireEvent.click(await screen.findByText("Unbounded retry loop"));
    await screen.findByRole("button", { name: /Keep as permanent knowledge/ });

    fireEvent.keyDown(document, { key: "ArrowDown" });
    await waitFor(() => expect(getNodeDetail).toHaveBeenCalledWith("agentops", 3));

    fireEvent.keyDown(document, { key: "ArrowUp" });
    await waitFor(() => expect(getNodeDetail).toHaveBeenCalledWith("agentops", 1));
  });

  it("ignores keyboard shortcuts while the reduce-reason textarea has focus", async () => {
    getGotchas.mockResolvedValue([needsCurationGotcha]);
    getNodeDetail.mockResolvedValue(nodeDetailFor(needsCurationGotcha));
    renderPage();

    fireEvent.click(await screen.findByText("Unbounded retry loop"));
    fireEvent.click(await screen.findByRole("button", { name: /Reduce prominence/ }));
    const textarea = await screen.findByPlaceholderText(/Only affects old Linux envs/);

    fireEvent.keyDown(textarea, { key: "k" });

    expect(setCuration).not.toHaveBeenCalled();
  });

  it("shows an error with a retry button when the detail fetch fails, instead of loading forever", async () => {
    getGotchas.mockResolvedValue([needsCurationGotcha]);
    getNodeDetail.mockRejectedValue(new Error("request failed with 500"));
    renderPage();

    fireEvent.click(await screen.findByText("Unbounded retry loop"));

    expect(await screen.findByText("Couldn't load this item's details.")).toBeInTheDocument();
    expect(screen.getByText("request failed with 500")).toBeInTheDocument();
    expect(screen.queryByText("Loading details…")).not.toBeInTheDocument();

    getNodeDetail.mockResolvedValue(nodeDetailFor(needsCurationGotcha));
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));

    expect(await screen.findByRole("button", { name: /Keep as permanent knowledge/ })).toBeInTheDocument();
  });
});

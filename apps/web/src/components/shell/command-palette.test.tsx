import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const { push } = vi.hoisted(() => ({ push: vi.fn() }));

vi.mock("next/navigation", () => ({
  useRouter: () => ({ push }),
}));

import { TooltipProvider } from "@/components/ui/tooltip";
import { CommandPalette } from "@/components/shell/command-palette";

function renderPalette() {
  return render(
    <TooltipProvider>
      <CommandPalette />
    </TooltipProvider>,
  );
}

describe("CommandPalette", () => {
  beforeEach(() => {
    push.mockClear();
  });

  it("opens on Ctrl/Cmd+K and navigates + closes on item selection", async () => {
    renderPalette();

    expect(screen.queryByPlaceholderText("Jump to a page...")).not.toBeInTheDocument();

    fireEvent.keyDown(document, { key: "k", ctrlKey: true });

    expect(await screen.findByPlaceholderText("Jump to a page...")).toBeInTheDocument();

    fireEvent.click(screen.getByText("Repositories"));

    await waitFor(() => expect(push).toHaveBeenCalledWith("/repositories"));
    await waitFor(() => expect(screen.queryByPlaceholderText("Jump to a page...")).not.toBeInTheDocument());
  });

  it("also opens via the visible trigger button", async () => {
    renderPalette();

    fireEvent.click(screen.getByRole("button", { name: "Open command palette" }));

    expect(await screen.findByPlaceholderText("Jump to a page...")).toBeInTheDocument();
  });
});

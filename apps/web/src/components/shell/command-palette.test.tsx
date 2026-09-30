import { useState } from "react";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const { push } = vi.hoisted(() => ({ push: vi.fn() }));

vi.mock("next/navigation", () => ({
  useRouter: () => ({ push }),
}));

import { TooltipProvider } from "@/components/ui/tooltip";
import { CommandPaletteDialog, CommandPaletteTrigger } from "@/components/shell/command-palette";

// CommandPaletteDialog/Trigger are controlled -- something above them owns
// `open` state (AppShell, in the real app). This harness mirrors that shape
// so the dialog+trigger pairing is tested the way it's actually wired.
function Harness() {
  const [open, setOpen] = useState(false);
  return (
    <TooltipProvider>
      <CommandPaletteTrigger onOpen={() => setOpen(true)} />
      <CommandPaletteDialog open={open} onOpenChange={setOpen} />
    </TooltipProvider>
  );
}

describe("CommandPaletteDialog / CommandPaletteTrigger", () => {
  beforeEach(() => {
    push.mockClear();
  });

  it("opens via the trigger and navigates + closes on item selection", async () => {
    render(<Harness />);

    expect(screen.queryByPlaceholderText("Jump to a page...")).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Open command palette" }));

    expect(await screen.findByPlaceholderText("Jump to a page...")).toBeInTheDocument();

    fireEvent.click(screen.getByText("Repositories"));

    await waitFor(() => expect(push).toHaveBeenCalledWith("/repositories"));
    await waitFor(() => expect(screen.queryByPlaceholderText("Jump to a page...")).not.toBeInTheDocument());
  });

  it("stays closed until its open prop flips true", () => {
    render(
      <TooltipProvider>
        <CommandPaletteDialog open={false} onOpenChange={() => {}} />
      </TooltipProvider>,
    );

    expect(screen.queryByPlaceholderText("Jump to a page...")).not.toBeInTheDocument();
  });
});

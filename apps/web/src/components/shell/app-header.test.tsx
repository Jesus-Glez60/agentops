import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: vi.fn() }),
  usePathname: () => "/",
}));

import { TooltipProvider } from "@/components/ui/tooltip";
import { AppHeader } from "@/components/shell/app-header";

describe("AppHeader", () => {
  it("renders the notification bell as disabled, matching the no-fake-unread-count precedent", () => {
    render(
      <TooltipProvider>
        <AppHeader />
      </TooltipProvider>,
    );

    expect(screen.getByRole("button", { name: "Notifications" })).toBeDisabled();
  });
});

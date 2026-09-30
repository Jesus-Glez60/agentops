import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { SWRConfig } from "swr";

vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: vi.fn() }),
  usePathname: () => "/",
}));

const { getMyMemberships } = vi.hoisted(() => ({ getMyMemberships: vi.fn() }));
vi.mock("@/lib/api/team-api", async () => {
  const actual = await vi.importActual<typeof import("@/lib/api/team-api")>("@/lib/api/team-api");
  return { ...actual, getMyMemberships };
});

const { getRepos } = vi.hoisted(() => ({ getRepos: vi.fn() }));
vi.mock("@/lib/api/repos-api", async () => {
  const actual = await vi.importActual<typeof import("@/lib/api/repos-api")>("@/lib/api/repos-api");
  return { ...actual, getRepos };
});

import { TooltipProvider } from "@/components/ui/tooltip";
import { AppHeader } from "@/components/shell/app-header";
import type { SessionUser } from "@/lib/auth/types";

const user: SessionUser = {
  id: 1,
  email: "jesus@agentops.dev",
  first_name: "Jesus",
  last_name: "Gonzalez",
  tenant: "acme",
  avatar_url: null,
  handle: null,
  bio: "",
  location: "",
  theme_pref: "dark",
  default_search_scope: "all",
  show_gotcha_callouts: true,
  graph_layout_algorithm: "force",
  two_factor_enabled: false,
  onboarding_completed: true,
};

describe("AppHeader", () => {
  it("renders the notification bell as disabled, matching the no-fake-unread-count precedent", () => {
    getMyMemberships.mockResolvedValue({ memberships: [] });
    getRepos.mockResolvedValue({ connections: [] });
    render(
      <SWRConfig value={{ provider: () => new Map() }}>
        <TooltipProvider>
          <AppHeader user={user} onOpenPalette={() => {}} />
        </TooltipProvider>
      </SWRConfig>,
    );

    expect(screen.getByRole("button", { name: "Notifications" })).toBeDisabled();
  });
});

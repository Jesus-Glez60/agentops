import { render, screen } from "@testing-library/react";
import { beforeAll, describe, expect, it, vi } from "vitest";
import { SWRConfig } from "swr";

// SidebarProvider's useIsMobile reads window.matchMedia, which jsdom doesn't
// implement -- a minimal stub is enough since no test here resizes the window.
beforeAll(() => {
  window.matchMedia =
    window.matchMedia ||
    ((query: string) => ({
      matches: false,
      media: query,
      onchange: null,
      addEventListener: () => {},
      removeEventListener: () => {},
      addListener: () => {},
      removeListener: () => {},
      dispatchEvent: () => false,
    })) as unknown as typeof window.matchMedia;
});

const { push } = vi.hoisted(() => ({ push: vi.fn() }));

vi.mock("next/navigation", () => ({
  usePathname: () => "/",
  useRouter: () => ({ push }),
}));

const { getRepos } = vi.hoisted(() => ({ getRepos: vi.fn() }));
vi.mock("@/lib/api/repos-api", async () => {
  const actual = await vi.importActual<typeof import("@/lib/api/repos-api")>("@/lib/api/repos-api");
  return { ...actual, getRepos };
});

const { getMyMemberships } = vi.hoisted(() => ({ getMyMemberships: vi.fn() }));
vi.mock("@/lib/api/team-api", async () => {
  const actual = await vi.importActual<typeof import("@/lib/api/team-api")>("@/lib/api/team-api");
  return { ...actual, getMyMemberships };
});

import { TooltipProvider } from "@/components/ui/tooltip";
import { SidebarProvider } from "@/components/ui/sidebar";
import { AppSidebar } from "@/components/shell/app-sidebar";
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

function renderSidebar(defaultOpen = true) {
  return render(
    <SWRConfig value={{ provider: () => new Map() }}>
      <TooltipProvider>
        <SidebarProvider defaultOpen={defaultOpen}>
          <AppSidebar user={user} />
        </SidebarProvider>
      </TooltipProvider>
    </SWRConfig>,
  );
}

describe("AppSidebar", () => {
  it("renders every nav item under its grouped section, plus Settings ungrouped", () => {
    getRepos.mockResolvedValue({ connections: [] });
    renderSidebar();

    expect(screen.getByText("Workspace")).toBeInTheDocument();
    expect(screen.getByText("Sources")).toBeInTheDocument();
    expect(screen.getByText("Knowledge")).toBeInTheDocument();

    expect(screen.getByText("Overview")).toBeInTheDocument();
    expect(screen.getByText("Search")).toBeInTheDocument();
    expect(screen.getByText("Knowledge Graph")).toBeInTheDocument();
    expect(screen.getByText("Libraries")).toBeInTheDocument();
    expect(screen.getByText("Repositories")).toBeInTheDocument();
    expect(screen.getByText("Documentation")).toBeInTheDocument();
    expect(screen.getByText("Gotchas")).toBeInTheDocument();
    expect(screen.getByText("Settings")).toBeInTheDocument();
  });

  it("renders the org scope switcher with the current org's initial", () => {
    getRepos.mockResolvedValue({ connections: [] });
    renderSidebar();

    expect(screen.getByText("acme")).toBeInTheDocument();
  });

  it("marks itself collapsible=icon and keeps rendering nav items when collapsed", () => {
    getRepos.mockResolvedValue({ connections: [] });
    const { container } = renderSidebar(false);

    const sidebarRoot = container.querySelector('[data-slot="sidebar"][data-state]');
    expect(sidebarRoot).toHaveAttribute("data-state", "collapsed");
    expect(sidebarRoot).toHaveAttribute("data-collapsible", "icon");

    // Nav items stay in the DOM (icon-only, not unmounted) in collapsed mode.
    expect(screen.getByText("Overview")).toBeInTheDocument();
    expect(screen.getByText("Gotchas")).toBeInTheDocument();
  });
});

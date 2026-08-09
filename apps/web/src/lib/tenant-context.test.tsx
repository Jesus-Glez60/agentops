import { act, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { TenantProvider, useTenant } from "./tenant-context";

const replaceMock = vi.fn();

vi.mock("next/navigation", () => ({
  useRouter: () => ({ replace: replaceMock }),
  useSearchParams: () => new URLSearchParams(currentSearch),
}));

vi.mock("@/lib/api/heavy-api", () => ({
  // Never resolves in these tests -- heavyTierAvailable stays null, which
  // is fine since these tests are about tenant/localStorage sync, not the
  // probe itself.
  getGithubAppInstallUrl: () => new Promise(() => {}),
}));

let currentSearch = "";

function Probe() {
  const { tenant, hasTenant, setTenant } = useTenant();
  return (
    <div>
      <span data-testid="tenant">{tenant ?? "(none)"}</span>
      <span data-testid="has-tenant">{String(hasTenant)}</span>
      <button onClick={() => setTenant("acme")}>set acme</button>
      <button onClick={() => setTenant(null)}>clear</button>
    </div>
  );
}

describe("TenantProvider", () => {
  beforeEach(() => {
    window.localStorage.clear();
    currentSearch = "";
    replaceMock.mockClear();
  });

  afterEach(() => {
    vi.clearAllMocks();
  });

  it("defaults to no tenant when neither the URL nor localStorage has one", () => {
    render(
      <TenantProvider>
        <Probe />
      </TenantProvider>,
    );
    expect(screen.getByTestId("tenant").textContent).toBe("(none)");
    expect(screen.getByTestId("has-tenant").textContent).toBe("false");
  });

  it("reads the tenant from the URL and persists it to localStorage", () => {
    currentSearch = "tenant=acme";
    render(
      <TenantProvider>
        <Probe />
      </TenantProvider>,
    );
    expect(screen.getByTestId("tenant").textContent).toBe("acme");
    expect(window.localStorage.getItem("agentops.tenant")).toBe("acme");
  });

  it("falls back to localStorage when the URL has no tenant param", () => {
    window.localStorage.setItem("agentops.tenant", "globex");
    render(
      <TenantProvider>
        <Probe />
      </TenantProvider>,
    );
    expect(screen.getByTestId("tenant").textContent).toBe("globex");
  });

  it("setTenant updates state, localStorage, and calls router.replace with the param", () => {
    render(
      <TenantProvider>
        <Probe />
      </TenantProvider>,
    );

    act(() => {
      screen.getByText("set acme").click();
    });

    expect(screen.getByTestId("tenant").textContent).toBe("acme");
    expect(window.localStorage.getItem("agentops.tenant")).toBe("acme");
    expect(replaceMock).toHaveBeenCalledWith("?tenant=acme");
  });

  it("setTenant(null) clears state, localStorage, and the URL param", () => {
    currentSearch = "tenant=acme";
    render(
      <TenantProvider>
        <Probe />
      </TenantProvider>,
    );

    act(() => {
      screen.getByText("clear").click();
    });

    expect(screen.getByTestId("tenant").textContent).toBe("(none)");
    expect(window.localStorage.getItem("agentops.tenant")).toBeNull();
    expect(replaceMock).toHaveBeenCalledWith("?");
  });
});

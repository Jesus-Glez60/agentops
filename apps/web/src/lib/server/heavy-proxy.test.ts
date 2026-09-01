import { describe, expect, it, vi, beforeEach } from "vitest";
import { NextRequest } from "next/server";

const { heavyApiFetch, getSessionToken } = vi.hoisted(() => ({ heavyApiFetch: vi.fn(), getSessionToken: vi.fn() }));
vi.mock("@/lib/server/heavy-api", async () => {
  const actual = await vi.importActual<typeof import("@/lib/server/heavy-api")>("@/lib/server/heavy-api");
  return { ...actual, heavyApiFetch };
});
vi.mock("@/lib/auth/session", () => ({ getSessionToken }));

import { HeavyApiError } from "@/lib/server/heavy-api";
import { proxyHeavyApi, isAllowedPath } from "@/lib/server/heavy-proxy";

function request(path: string, init: ConstructorParameters<typeof NextRequest>[1] = {}) {
  return new NextRequest(`http://localhost/api/heavy/${path}`, init);
}

describe("isAllowedPath", () => {
  it("allows exact and nested matches of allowlisted prefixes", () => {
    expect(isAllowedPath("auth/me")).toBe(true);
    expect(isAllowedPath("auth/me/preferences")).toBe(true);
    expect(isAllowedPath("team")).toBe(true);
    expect(isAllowedPath("team/members/5")).toBe(true);
    expect(isAllowedPath("repos")).toBe(true);
    expect(isAllowedPath("repos/connect")).toBe(true);
    expect(isAllowedPath("integrations")).toBe(true);
    expect(isAllowedPath("integrations/me")).toBe(true);
    expect(isAllowedPath("integrations/me/linear")).toBe(true);
    // Regression: these three were genuinely called by the frontend
    // (getGotchas/search/getActivity in repos-api.ts) but missing from
    // this allowlist -- every request 404'd from the proxy itself before
    // ever reaching the backend, which looked like an empty/broken data
    // bug (the Gotchas page silently showing 0 of everything) rather than
    // the routing gap it actually was. Caught live against the deployed
    // Gotchas page.
    expect(isAllowedPath("gotchas")).toBe(true);
    expect(isAllowedPath("local-search")).toBe(true);
    expect(isAllowedPath("activity")).toBe(true);
  });

  it("rejects anything not on the allowlist, including prefix look-alikes", () => {
    expect(isAllowedPath("search")).toBe(false);
    expect(isAllowedPath("docs/search")).toBe(false);
    expect(isAllowedPath("auth-me-lookalike")).toBe(false);
    expect(isAllowedPath("repos-lookalike")).toBe(false);
    expect(isAllowedPath("../../etc/passwd")).toBe(false);
  });
});

describe("proxyHeavyApi", () => {
  beforeEach(() => {
    heavyApiFetch.mockReset();
    getSessionToken.mockReset();
  });

  it("rejects a non-allowlisted path with 404 before ever checking the session or calling the backend", async () => {
    const response = await proxyHeavyApi(request("search"), ["search"]);
    expect(response.status).toBe(404);
    expect(getSessionToken).not.toHaveBeenCalled();
    expect(heavyApiFetch).not.toHaveBeenCalled();
  });

  it("rejects an allowlisted path with 401 when there's no session cookie", async () => {
    getSessionToken.mockResolvedValue(undefined);
    const response = await proxyHeavyApi(request("auth/me"), ["auth", "me"]);
    expect(response.status).toBe(401);
    expect(heavyApiFetch).not.toHaveBeenCalled();
  });

  it("forwards an allowlisted request to the backend with the bearer token and method/body", async () => {
    getSessionToken.mockResolvedValue("raw-session-token");
    heavyApiFetch.mockResolvedValue({ id: 1, bio: "Staff engineer" });

    const response = await proxyHeavyApi(request("auth/me", { method: "PATCH", body: JSON.stringify({ bio: "Staff engineer" }) }), ["auth", "me"]);

    expect(response.status).toBe(200);
    expect(await response.json()).toEqual({ id: 1, bio: "Staff engineer" });
    expect(heavyApiFetch).toHaveBeenCalledWith("/auth/me", expect.objectContaining({ method: "PATCH", token: "raw-session-token", body: JSON.stringify({ bio: "Staff engineer" }) }));
  });

  it("passes the backend's error message and status straight through", async () => {
    getSessionToken.mockResolvedValue("raw-session-token");
    heavyApiFetch.mockRejectedValue(new HeavyApiError("forbidden", 403));

    const response = await proxyHeavyApi(request("team/members/5", { method: "DELETE" }), ["team", "members", "5"]);

    expect(response.status).toBe(403);
    expect(await response.json()).toEqual({ error: "forbidden" });
  });
});

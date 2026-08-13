import { describe, expect, it, vi, beforeEach } from "vitest";
import { NextRequest } from "next/server";

const { heavyApiFetch } = vi.hoisted(() => ({ heavyApiFetch: vi.fn() }));
vi.mock("@/lib/server/heavy-api", async () => {
  const actual = await vi.importActual<typeof import("@/lib/server/heavy-api")>("@/lib/server/heavy-api");
  return { ...actual, heavyApiFetch };
});

import { HeavyApiError } from "@/lib/server/heavy-api";
import { proxyCredentialsAuth } from "@/lib/server/auth-proxy";

function jsonRequest(body: unknown) {
  return new NextRequest("http://localhost/api/auth/login", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
}

describe("proxyCredentialsAuth", () => {
  beforeEach(() => {
    heavyApiFetch.mockReset();
  });

  it("rejects a request missing email or password before calling the backend", async () => {
    const response = await proxyCredentialsAuth(jsonRequest({ email: "dev@example.com" }), "/auth/login", 200);
    expect(response.status).toBe(400);
    expect(heavyApiFetch).not.toHaveBeenCalled();
  });

  it("on success, sets an httpOnly/secure/sameSite=lax session cookie and returns just the user", async () => {
    heavyApiFetch.mockResolvedValue({ user: { id: 1, email: "dev@example.com", tenant: "abc123" }, session_token: "raw-token-value" });

    const response = await proxyCredentialsAuth(jsonRequest({ email: "dev@example.com", password: "pw" }), "/auth/login", 200);

    expect(response.status).toBe(200);
    const body = await response.json();
    expect(body).toEqual({ user: { id: 1, email: "dev@example.com", tenant: "abc123" } });
    expect(JSON.stringify(body)).not.toContain("raw-token-value");

    const cookie = response.cookies.get("agentops_session");
    expect(cookie?.value).toBe("raw-token-value");
    expect(cookie?.httpOnly).toBe(true);
    expect(cookie?.sameSite).toBe("lax");
  });

  it("uses the given success status (201 for signup)", async () => {
    heavyApiFetch.mockResolvedValue({ user: { id: 2, email: "new@example.com", tenant: "def456" }, session_token: "raw-token" });
    const response = await proxyCredentialsAuth(jsonRequest({ email: "new@example.com", password: "pw" }), "/auth/signup", 201);
    expect(response.status).toBe(201);
  });

  it("passes the backend's error message and status straight through", async () => {
    heavyApiFetch.mockRejectedValue(new HeavyApiError("invalid email or password", 401));

    const response = await proxyCredentialsAuth(jsonRequest({ email: "dev@example.com", password: "wrong" }), "/auth/login", 200);

    expect(response.status).toBe(401);
    expect(await response.json()).toEqual({ error: "invalid email or password" });
  });

  it("returns 502 when the backend is unreachable", async () => {
    heavyApiFetch.mockRejectedValue(new Error("fetch failed"));

    const response = await proxyCredentialsAuth(jsonRequest({ email: "dev@example.com", password: "pw" }), "/auth/login", 200);

    expect(response.status).toBe(502);
  });
});

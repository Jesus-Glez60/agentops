import { describe, expect, it, vi, beforeEach } from "vitest";
import { NextRequest } from "next/server";

const { heavyApiFetch } = vi.hoisted(() => ({ heavyApiFetch: vi.fn() }));
vi.mock("@/lib/server/heavy-api", async () => {
  const actual = await vi.importActual<typeof import("@/lib/server/heavy-api")>("@/lib/server/heavy-api");
  return { ...actual, heavyApiFetch };
});

import { HeavyApiError } from "@/lib/server/heavy-api";
import { proxyCredentialsAuth, proxyLogin2fa } from "@/lib/server/auth-proxy";

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

  it("passes a 202 two-factor challenge straight through without setting a session cookie", async () => {
    heavyApiFetch.mockResolvedValue({ two_factor_required: true, challenge_token: "raw-challenge-token" });

    const response = await proxyCredentialsAuth(jsonRequest({ email: "dev@example.com", password: "pw" }), "/auth/login", 200);

    expect(response.status).toBe(202);
    expect(await response.json()).toEqual({ two_factor_required: true, challenge_token: "raw-challenge-token" });
    expect(response.cookies.get("agentops_session")).toBeUndefined();
  });
});

describe("proxyLogin2fa", () => {
  beforeEach(() => {
    heavyApiFetch.mockReset();
  });

  function challengeRequest(body: unknown) {
    return new NextRequest("http://localhost/api/auth/login/2fa", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) });
  }

  it("rejects a request missing challenge_token or code before calling the backend", async () => {
    const response = await proxyLogin2fa(challengeRequest({ code: "123456" }));
    expect(response.status).toBe(400);
    expect(heavyApiFetch).not.toHaveBeenCalled();
  });

  it("on success, sets the session cookie and returns just the user", async () => {
    heavyApiFetch.mockResolvedValue({ user: { id: 1, email: "dev@example.com", tenant: "abc123" }, session_token: "raw-token-value" });

    const response = await proxyLogin2fa(challengeRequest({ challenge_token: "raw-challenge-token", code: "123456" }));

    expect(response.status).toBe(200);
    expect(await response.json()).toEqual({ user: { id: 1, email: "dev@example.com", tenant: "abc123" } });
    expect(response.cookies.get("agentops_session")?.value).toBe("raw-token-value");
  });

  it("passes the backend's error message and status straight through on a wrong code", async () => {
    heavyApiFetch.mockRejectedValue(new HeavyApiError("invalid verification code", 401));

    const response = await proxyLogin2fa(challengeRequest({ challenge_token: "raw-challenge-token", code: "000000" }));

    expect(response.status).toBe(401);
    expect(await response.json()).toEqual({ error: "invalid verification code" });
  });
});

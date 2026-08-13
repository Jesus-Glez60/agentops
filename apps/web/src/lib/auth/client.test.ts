import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AuthClientError, loginWithPassword, logout, signupWithPassword } from "@/lib/auth/client";

describe("auth client", () => {
  const fetchMock = vi.fn();

  beforeEach(() => {
    fetchMock.mockReset();
    vi.stubGlobal("fetch", fetchMock);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("loginWithPassword posts to /api/auth/login with the right body and returns the user", async () => {
    fetchMock.mockResolvedValue({ ok: true, json: async () => ({ user: { id: 1, email: "dev@example.com", first_name: "Ada", last_name: "Lovelace", tenant: "abc" } }) });

    const user = await loginWithPassword("dev@example.com", "pw");

    expect(fetchMock).toHaveBeenCalledWith(
      "/api/auth/login",
      expect.objectContaining({ method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ email: "dev@example.com", password: "pw" }) }),
    );
    expect(user).toEqual({ id: 1, email: "dev@example.com", first_name: "Ada", last_name: "Lovelace", tenant: "abc" });
  });

  it("signupWithPassword posts to /api/auth/signup with first/last name included", async () => {
    fetchMock.mockResolvedValue({ ok: true, json: async () => ({ user: { id: 2, email: "new@example.com", first_name: "Ada", last_name: "Lovelace", tenant: "def" } }) });

    await signupWithPassword("Ada", "Lovelace", "new@example.com", "pw");

    expect(fetchMock).toHaveBeenCalledWith(
      "/api/auth/signup",
      expect.objectContaining({ method: "POST", body: JSON.stringify({ first_name: "Ada", last_name: "Lovelace", email: "new@example.com", password: "pw" }) }),
    );
  });

  it("surfaces the backend's error message via AuthClientError", async () => {
    fetchMock.mockResolvedValue({ ok: false, json: async () => ({ error: "invalid email or password" }) });

    await expect(loginWithPassword("dev@example.com", "wrong")).rejects.toThrow(AuthClientError);
    await expect(loginWithPassword("dev@example.com", "wrong")).rejects.toThrow("invalid email or password");
  });

  it("falls back to a generic message when the error body isn't shaped as expected", async () => {
    fetchMock.mockResolvedValue({ ok: false, json: async () => null });

    await expect(loginWithPassword("dev@example.com", "wrong")).rejects.toThrow("Something went wrong. Please try again.");
  });

  it("logout posts to /api/auth/logout", async () => {
    fetchMock.mockResolvedValue({ ok: true, json: async () => null });

    await logout();

    expect(fetchMock).toHaveBeenCalledWith("/api/auth/logout", { method: "POST" });
  });
});

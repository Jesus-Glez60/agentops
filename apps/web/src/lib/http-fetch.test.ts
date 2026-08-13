import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { HttpError, httpFetchJson } from "@/lib/http-fetch";

describe("httpFetchJson", () => {
  const fetchMock = vi.fn();

  beforeEach(() => {
    fetchMock.mockReset();
    vi.stubGlobal("fetch", fetchMock);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("returns the parsed JSON body on success", async () => {
    fetchMock.mockResolvedValue({ ok: true, json: async () => ({ hello: "world" }) });
    await expect(httpFetchJson("http://example.test/x")).resolves.toEqual({ hello: "world" });
  });

  it("throws HttpError with the backend's error message and status on failure", async () => {
    fetchMock.mockResolvedValue({ ok: false, status: 400, json: async () => ({ error: "bad request" }) });
    await expect(httpFetchJson("http://example.test/x")).rejects.toThrow(HttpError);
    await expect(httpFetchJson("http://example.test/x")).rejects.toThrow("bad request");
  });

  it("falls back to a generic message when the error body isn't shaped as expected", async () => {
    fetchMock.mockResolvedValue({ ok: false, status: 500, json: async () => null });
    await expect(httpFetchJson("http://example.test/x")).rejects.toThrow(/failed with 500/);
  });

  it("always requests with cache: no-store", async () => {
    fetchMock.mockResolvedValue({ ok: true, json: async () => ({}) });
    await httpFetchJson("http://example.test/x", { method: "POST" });
    expect(fetchMock).toHaveBeenCalledWith("http://example.test/x", expect.objectContaining({ method: "POST", cache: "no-store" }));
  });
});

import { afterEach, describe, expect, it, vi } from "vitest";
import { MISSING_MANAGE_MESSAGE, scrapeLibrary } from "@/lib/api/libraries-api";

function mockFetch(status: number, body: unknown) {
  vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } })));
}

describe("scrapeLibrary", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("turns a missing libraries.manage capability into a readable message", async () => {
    mockFetch(403, { error: "missing required capability: libraries.manage" });
    await expect(scrapeLibrary("react", "19", true)).resolves.toEqual({ isError: true, text: MISSING_MANAGE_MESSAGE });
  });

  it("still throws other failures", async () => {
    mockFetch(500, { error: "boom" });
    await expect(scrapeLibrary("react", "19", true)).rejects.toThrow("boom");
  });

  it("returns the tool result on success", async () => {
    mockFetch(200, { content: [{ type: "text", text: "queued job 7" }], isError: false });
    await expect(scrapeLibrary("react", "19", true)).resolves.toEqual({ isError: false, text: "queued job 7" });
  });
});

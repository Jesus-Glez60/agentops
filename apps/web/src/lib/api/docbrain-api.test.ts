import { describe, expect, it, vi } from "vitest";

const apiFetchMock = vi.fn();
vi.mock("./fetcher", () => ({
  DOCBRAIN_API_BASE: "http://test",
  apiFetch: (...args: unknown[]) => apiFetchMock(...args),
}));

import { probeDocVersions } from "./docbrain-api";

describe("probeDocVersions (parses get_docs' 'Available versions: [...]' text)", () => {
  it("parses a real multi-version list", async () => {
    apiFetchMock.mockResolvedValueOnce({
      content: [{ type: "text", text: 'No docs found for next@__version_probe__. Available versions: ["15.1.2", "15.1.3", "16.0.0"]' }],
      isError: false,
    });
    expect(await probeDocVersions("next")).toEqual(["15.1.2", "15.1.3", "16.0.0"]);
  });

  it("returns an empty array when the library has no doc snapshots yet", async () => {
    apiFetchMock.mockResolvedValueOnce({
      content: [{ type: "text", text: "No docs found for ghost@__version_probe__. Available versions: []" }],
      isError: false,
    });
    expect(await probeDocVersions("ghost")).toEqual([]);
  });

  it("returns an empty array (not a throw) when the response text doesn't match the expected shape at all", async () => {
    apiFetchMock.mockResolvedValueOnce({ content: [{ type: "text", text: "no library 'x' visible to this caller" }], isError: true });
    expect(await probeDocVersions("x")).toEqual([]);
  });
});

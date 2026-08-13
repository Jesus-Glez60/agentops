import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { getActivity, getNodeDetail, getRepos, rescanRepo, search } from "@/lib/api/agentops-api";

describe("agentops-api client", () => {
  const fetchMock = vi.fn();

  beforeEach(() => {
    fetchMock.mockReset();
    vi.stubGlobal("fetch", fetchMock);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("getRepos hits GET /repos and unwraps the repos array", async () => {
    fetchMock.mockResolvedValue({ ok: true, json: async () => ({ repos: [{ name: "agentops" }] }) });
    const repos = await getRepos();
    expect(repos).toEqual([{ name: "agentops" }]);
    const [url] = fetchMock.mock.calls[0];
    expect(String(url)).toContain("/repos");
  });

  it("rescanRepo POSTs to /repos/{name}/rescan and URL-encodes the name", async () => {
    fetchMock.mockResolvedValue({ ok: true, json: async () => ({ name: "my repo" }) });
    await rescanRepo("my repo");
    const [url, init] = fetchMock.mock.calls[0];
    expect(String(url)).toContain("/repos/my%20repo/rescan");
    expect(init).toEqual(expect.objectContaining({ method: "POST" }));
  });

  it("getActivity hits GET /activity and unwraps the activity array", async () => {
    fetchMock.mockResolvedValue({ ok: true, json: async () => ({ activity: [{ repo: "agentops" }] }) });
    const activity = await getActivity();
    expect(activity).toEqual([{ repo: "agentops" }]);
  });

  it("surfaces backend errors via ApiError", async () => {
    fetchMock.mockResolvedValue({ ok: false, status: 500, json: async () => ({ error: "boom" }) });
    await expect(getRepos()).rejects.toThrow("boom");
  });

  it("search hits GET /search with the query and unwraps the results array", async () => {
    fetchMock.mockResolvedValue({ ok: true, json: async () => ({ results: [{ repo: "agentops", id: 1 }] }) });
    const results = await search("token refresh");
    expect(results).toEqual([{ repo: "agentops", id: 1 }]);
    const [url] = fetchMock.mock.calls[0];
    expect(String(url)).toContain("/search?q=token+refresh");
  });

  it("search encodes repos/kinds/topK into the query string", async () => {
    fetchMock.mockResolvedValue({ ok: true, json: async () => ({ results: [] }) });
    await search("auth", { repos: ["agentops", "other"], kinds: ["Gotcha", "Decision"], topK: 5 });
    const [url] = fetchMock.mock.calls[0];
    const parsed = new URL(String(url));
    expect(parsed.searchParams.get("repos")).toBe("agentops,other");
    expect(parsed.searchParams.get("kind")).toBe("gotcha,decision");
    expect(parsed.searchParams.get("top_k")).toBe("5");
  });

  it("search omits repos/kind/top_k params when options are empty", async () => {
    fetchMock.mockResolvedValue({ ok: true, json: async () => ({ results: [] }) });
    await search("auth");
    const [url] = fetchMock.mock.calls[0];
    const parsed = new URL(String(url));
    expect(parsed.searchParams.has("repos")).toBe(false);
    expect(parsed.searchParams.has("kind")).toBe(false);
    expect(parsed.searchParams.has("top_k")).toBe(false);
  });

  it("getNodeDetail hits GET /repos/{name}/nodes/{id} and URL-encodes the repo name", async () => {
    fetchMock.mockResolvedValue({ ok: true, json: async () => ({ id: 42, repo: "my repo" }) });
    const detail = await getNodeDetail("my repo", 42);
    expect(detail).toEqual({ id: 42, repo: "my repo" });
    const [url] = fetchMock.mock.calls[0];
    expect(String(url)).toContain("/repos/my%20repo/nodes/42");
  });

  it("search surfaces backend errors via ApiError", async () => {
    fetchMock.mockResolvedValue({ ok: false, status: 500, json: async () => ({ error: "search boom" }) });
    await expect(search("auth")).rejects.toThrow("search boom");
  });

  it("getNodeDetail surfaces a 404 via ApiError", async () => {
    fetchMock.mockResolvedValue({ ok: false, status: 404, json: async () => ({ error: "no such node" }) });
    await expect(getNodeDetail("agentops", 999)).rejects.toThrow("no such node");
  });
});

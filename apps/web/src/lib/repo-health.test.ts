import { describe, expect, it } from "vitest";
import { repoHealth, repoHealthWithReason } from "./repo-health";

const NOW = 1_700_000_000; // arbitrary fixed "now" in unix seconds

describe("repoHealth", () => {
  it("is not-indexed when counts is null", () => {
    expect(repoHealth({ last_scanned_at: NOW, counts: null }, NOW)).toBe("not-indexed");
  });

  it("is stale when last_scanned_at is older than 7 days", () => {
    const eightDaysAgo = NOW - 60 * 60 * 24 * 8;
    expect(repoHealth({ last_scanned_at: eightDaysAgo, counts: { files: 1, symbols: 1, gotchas: 0, gotchas_needing_curation: 0, decisions: 0 } }, NOW)).toBe("stale");
  });

  it("is warning when there are gotchas needing curation and the scan is recent", () => {
    expect(repoHealth({ last_scanned_at: NOW, counts: { files: 1, symbols: 1, gotchas: 2, gotchas_needing_curation: 2, decisions: 0 } }, NOW)).toBe("warning");
  });

  it("is healthy when every gotcha has been curated, even though the total is nonzero", () => {
    expect(repoHealth({ last_scanned_at: NOW, counts: { files: 1, symbols: 1, gotchas: 2, gotchas_needing_curation: 0, decisions: 0 } }, NOW)).toBe("healthy");
  });

  it("is healthy when recently scanned with no gotchas", () => {
    expect(repoHealth({ last_scanned_at: NOW, counts: { files: 1, symbols: 1, gotchas: 0, gotchas_needing_curation: 0, decisions: 0 } }, NOW)).toBe("healthy");
  });

  it("defaults nowSeconds to the current time when omitted", () => {
    expect(repoHealth({ last_scanned_at: Date.now() / 1000, counts: { files: 1, symbols: 1, gotchas: 0, gotchas_needing_curation: 0, decisions: 0 } })).toBe("healthy");
  });
});

describe("repoHealthWithReason", () => {
  const healthyCounts = { files: 1, symbols: 1, gotchas: 0, gotchas_needing_curation: 0, decisions: 0 };

  it("reports path_missing as the reason, taking priority over a simultaneous failed status", () => {
    const result = repoHealthWithReason({ last_scanned_at: NOW, counts: healthyCounts, path_missing: true, status: "failed: clone error" }, NOW);
    expect(result.reason).toBe("Repo path no longer exists");
  });

  it("reports a failed job's own reason when the status is failed and the path exists", () => {
    const result = repoHealthWithReason({ last_scanned_at: NOW, counts: healthyCounts, path_missing: false, status: "failed: authentication error" }, NOW);
    expect(result.reason).toBe("authentication error");
  });

  it("reports a stale reason when last scanned over 7 days ago", () => {
    const eightDaysAgo = NOW - 60 * 60 * 24 * 8;
    const result = repoHealthWithReason({ last_scanned_at: eightDaysAgo, counts: healthyCounts, status: "active" }, NOW);
    expect(result.status).toBe("stale");
    expect(result.reason).toBe("Not scanned in over 7 days");
  });

  it("reports a gotcha-count reason when warning", () => {
    const result = repoHealthWithReason({ last_scanned_at: NOW, counts: { ...healthyCounts, gotchas: 3, gotchas_needing_curation: 3 }, status: "active" }, NOW);
    expect(result.status).toBe("warning");
    expect(result.reason).toBe("3 gotchas need curation");
  });

  it("reports a not-yet-scanned reason when counts is null", () => {
    const result = repoHealthWithReason({ last_scanned_at: NOW, counts: null, status: "pending" }, NOW);
    expect(result.status).toBe("not-indexed");
    expect(result.reason).toBe("Not yet scanned");
  });

  it("reports no reason when healthy", () => {
    const result = repoHealthWithReason({ last_scanned_at: NOW, counts: healthyCounts, status: "active" }, NOW);
    expect(result.status).toBe("healthy");
    expect(result.reason).toBeNull();
  });
});

import { describe, expect, it } from "vitest";
import { repoHealth } from "./repo-health";

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

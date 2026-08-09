import { describe, expect, it } from "vitest";
import { connectionHealth, repoHealth } from "./repo-health";
import type { RepoSummary } from "./api/types";

const NOW = 1_700_000_000; // arbitrary fixed "now" in unix seconds

describe("repoHealth", () => {
  it("is not-indexed when counts is null", () => {
    const repo: RepoSummary = { path: "/x", last_scanned_at: NOW, counts: null };
    expect(repoHealth(repo, NOW)).toBe("not-indexed");
  });

  it("is stale when last_scanned_at is older than 7 days", () => {
    const eightDaysAgo = NOW - 60 * 60 * 24 * 8;
    const repo: RepoSummary = { path: "/x", last_scanned_at: eightDaysAgo, counts: { files: 1, symbols: 1, gotchas: 0, decisions: 0 } };
    expect(repoHealth(repo, NOW)).toBe("stale");
  });

  it("is warning when there are unresolved gotchas and the scan is recent", () => {
    const repo: RepoSummary = { path: "/x", last_scanned_at: NOW, counts: { files: 1, symbols: 1, gotchas: 2, decisions: 0 } };
    expect(repoHealth(repo, NOW)).toBe("warning");
  });

  it("is healthy when recently scanned with no gotchas", () => {
    const repo: RepoSummary = { path: "/x", last_scanned_at: NOW, counts: { files: 1, symbols: 1, gotchas: 0, decisions: 0 } };
    expect(repoHealth(repo, NOW)).toBe("healthy");
  });
});

describe("connectionHealth", () => {
  it("maps active/pending/failed/unknown statuses correctly", () => {
    expect(connectionHealth("active")).toBe("healthy");
    expect(connectionHealth("pending")).toBe("scanning");
    expect(connectionHealth("failed: SSH_AUTH_FAILURE")).toBe("failed");
    expect(connectionHealth("something-unexpected")).toBe("not-indexed");
  });
});

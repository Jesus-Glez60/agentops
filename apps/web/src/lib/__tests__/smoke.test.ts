import { describe, expect, it } from "vitest";

// Confirms the Vitest harness itself (config, jsdom env, path alias) works
// before real suites (api client, graph traversal, tenant-context) land in
// Phase 2 — delete once those exist and this becomes redundant.
describe("vitest harness", () => {
  it("runs", () => {
    expect(1 + 1).toBe(2);
  });
});

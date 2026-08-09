import { describe, expect, it } from "vitest";
import { failureReason, isFailedStatus, isPrivateVisibility, visibilityLabel } from "./types";

describe("Visibility (confirmed wire shape: 'Public' | { Private: string })", () => {
  it("recognizes the public unit variant", () => {
    expect(isPrivateVisibility("Public")).toBe(false);
    expect(visibilityLabel("Public")).toBe("public");
  });

  it("recognizes the private tuple variant and extracts the org id", () => {
    const v = { Private: "acme" };
    expect(isPrivateVisibility(v)).toBe(true);
    expect(visibilityLabel(v)).toBe("private (acme)");
  });
});

describe("ConnectionStatus ('pending' | 'active' | 'failed: {reason}')", () => {
  it("does not treat pending/active as failed", () => {
    expect(isFailedStatus("pending")).toBe(false);
    expect(isFailedStatus("active")).toBe(false);
    expect(failureReason("pending")).toBeNull();
  });

  it("recognizes a failed status and extracts the reason", () => {
    expect(isFailedStatus("failed: SSH_AUTH_FAILURE")).toBe(true);
    expect(failureReason("failed: SSH_AUTH_FAILURE")).toBe("SSH_AUTH_FAILURE");
  });

  it("handles a failed status with no reason text gracefully", () => {
    expect(failureReason("failed")).toBeNull();
  });
});

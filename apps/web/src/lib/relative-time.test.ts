import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { relativeTimeFromIsoString, relativeTimeFromMs, relativeTimeFromUnixSeconds } from "./relative-time";

const FIXED_NOW = new Date("2026-01-01T12:00:00Z").getTime();

describe("relative-time", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(FIXED_NOW);
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("renders 'just now' for sub-minute differences", () => {
    expect(relativeTimeFromMs(FIXED_NOW - 30_000)).toBe("just now");
  });

  it("renders minutes for under an hour", () => {
    expect(relativeTimeFromMs(FIXED_NOW - 12 * 60_000)).toBe("12m ago");
  });

  it("renders hours for under a day", () => {
    expect(relativeTimeFromMs(FIXED_NOW - 5 * 60 * 60_000)).toBe("5h ago");
  });

  it("renders days beyond that", () => {
    expect(relativeTimeFromMs(FIXED_NOW - 3 * 24 * 60 * 60_000)).toBe("3d ago");
  });

  it("relativeTimeFromUnixSeconds converts seconds to ms correctly", () => {
    const unixSeconds = (FIXED_NOW - 10 * 60_000) / 1000;
    expect(relativeTimeFromUnixSeconds(unixSeconds)).toBe("10m ago");
  });

  it("relativeTimeFromIsoString parses an ISO timestamp correctly", () => {
    const iso = new Date(FIXED_NOW - 2 * 60 * 60_000).toISOString();
    expect(relativeTimeFromIsoString(iso)).toBe("2h ago");
  });
});

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { relativeTimeFromIsoString, relativeTimeFromMs, relativeTimeFromUnixSeconds } from "./relative-time";

const NOW_MS = 1_786_500_000_000;

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(NOW_MS);
});

afterEach(() => {
  vi.useRealTimers();
});

describe("relativeTimeFromMs", () => {
  it("shows 'just now' for under a minute", () => {
    expect(relativeTimeFromMs(NOW_MS - 30_000)).toBe("just now");
  });

  it("floors, not rounds -- 90s ago is 1m ago, not 2m", () => {
    expect(relativeTimeFromMs(NOW_MS - 90_000)).toBe("1m ago");
  });

  it("shows hours once past 60 minutes", () => {
    expect(relativeTimeFromMs(NOW_MS - 60 * 60_000 * 2)).toBe("2h ago");
  });

  it("shows days once past 24 hours", () => {
    expect(relativeTimeFromMs(NOW_MS - 60 * 60_000 * 24 * 3)).toBe("3d ago");
  });
});

describe("relativeTimeFromUnixSeconds", () => {
  it("converts seconds to ms before formatting", () => {
    expect(relativeTimeFromUnixSeconds(NOW_MS / 1000 - 90)).toBe("1m ago");
  });
});

describe("relativeTimeFromIsoString", () => {
  it("treats a SQLite CURRENT_TIMESTAMP-shaped string (no T, no Z) as UTC, not local time", () => {
    const nowUtcSqliteShaped = new Date(NOW_MS).toISOString().slice(0, 19).replace("T", " ");
    expect(relativeTimeFromIsoString(nowUtcSqliteShaped)).toBe("just now");
  });

  it("still handles a proper ISO string with T/Z unchanged", () => {
    const nowIso = new Date(NOW_MS - 60_000).toISOString();
    expect(relativeTimeFromIsoString(nowIso)).toBe("1m ago");
  });
});

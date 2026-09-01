import { describe, expect, it } from "vitest";
import { mapWithConcurrency } from "./concurrency";

describe("mapWithConcurrency", () => {
  it("never runs more than `limit` calls at once", async () => {
    let active = 0;
    let maxActive = 0;
    await mapWithConcurrency([1, 2, 3, 4, 5, 6, 7, 8], 3, async (n) => {
      active++;
      maxActive = Math.max(maxActive, active);
      await new Promise((resolve) => setTimeout(resolve, 5));
      active--;
      return n * 2;
    });
    expect(maxActive).toBeLessThanOrEqual(3);
  });

  it("returns results in the same order as the input, not completion order", async () => {
    const delays = [30, 10, 20];
    const results = await mapWithConcurrency(delays, 3, async (delay, i) => {
      await new Promise((resolve) => setTimeout(resolve, delay));
      return i;
    });
    expect(results.map((r) => (r.status === "fulfilled" ? r.value : null))).toEqual([0, 1, 2]);
  });

  it("captures a rejection as a rejected result without failing the whole batch", async () => {
    const results = await mapWithConcurrency([1, 2, 3], 2, async (n) => {
      if (n === 2) throw new Error("boom");
      return n;
    });
    expect(results[0]).toEqual({ status: "fulfilled", value: 1 });
    expect(results[1].status).toBe("rejected");
    expect(results[2]).toEqual({ status: "fulfilled", value: 3 });
  });

  it("handles an empty input", async () => {
    const results = await mapWithConcurrency([], 5, async (n: number) => n);
    expect(results).toEqual([]);
  });
});

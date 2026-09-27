import { describe, expect, it } from "vitest";
import { BASE_PATH, withBasePath } from "@/lib/base-path";

describe("withBasePath", () => {
  it("prefixes a leading-slash path with BASE_PATH", () => {
    expect(withBasePath("/login")).toBe(`${BASE_PATH}/login`);
  });

  it("adds the missing leading slash before prefixing", () => {
    expect(withBasePath("login")).toBe(`${BASE_PATH}/login`);
  });

  it("stays consistent with next.config.ts's basePath value", () => {
    // This is the actual bug class recorded in this project's notes
    // (Next.js only auto-prefixes next/link, next/router, and redirect() --
    // never a raw fetch()/window.location/new URL string) -- if BASE_PATH
    // ever drifts from next.config.ts's `basePath`, every raw call site
    // wrapped in withBasePath() silently starts 404ing in production while
    // still passing this test, tsc, and build. This test only guards
    // withBasePath's own string logic; keeping the two values in sync is
    // still a manual, human responsibility (see the "single source of
    // truth" gap recorded as a follow-up).
    expect(BASE_PATH).toBe("/suite");
  });
});

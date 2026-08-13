import { describe, expect, it } from "vitest";
import { NAV_ITEMS, navLabelForPath } from "@/lib/nav-config";

describe("navLabelForPath", () => {
  it("resolves a top-level nav route to its label", () => {
    expect(navLabelForPath("/repositories")).toBe("Repositories");
    expect(navLabelForPath("/")).toBe("Overview");
  });

  it("falls back to a title-cased last path segment for sub-routes", () => {
    expect(navLabelForPath("/libraries/react-router")).toBe("React Router");
  });

  it("falls back to Overview for an empty path", () => {
    expect(navLabelForPath("")).toBe("Overview");
  });

  it("has exactly the eight items the design mock specifies, in order", () => {
    expect(NAV_ITEMS.map((item) => item.href)).toEqual(["/", "/search", "/graph", "/docs", "/libraries", "/repositories", "/gotchas", "/settings"]);
  });
});

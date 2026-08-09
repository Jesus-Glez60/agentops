import { beforeEach, describe, expect, it } from "vitest";
import { getRecentSearches, pushRecentSearch } from "./recent-searches";

describe("recent-searches", () => {
  beforeEach(() => {
    window.localStorage.clear();
  });

  it("starts empty", () => {
    expect(getRecentSearches("code")).toEqual([]);
  });

  it("adds a query to the front", () => {
    pushRecentSearch("code", "how does auth work");
    expect(getRecentSearches("code")).toEqual(["how does auth work"]);
  });

  it("de-duplicates and moves a repeated query back to the front", () => {
    pushRecentSearch("code", "a");
    pushRecentSearch("code", "b");
    pushRecentSearch("code", "a");
    expect(getRecentSearches("code")).toEqual(["a", "b"]);
  });

  it("caps at 5 entries", () => {
    for (const q of ["1", "2", "3", "4", "5", "6"]) pushRecentSearch("code", q);
    expect(getRecentSearches("code")).toEqual(["6", "5", "4", "3", "2"]);
  });

  it("keeps 'code' and 'docs' namespaces separate", () => {
    pushRecentSearch("code", "x");
    expect(getRecentSearches("docs")).toEqual([]);
  });

  it("ignores blank queries", () => {
    pushRecentSearch("code", "   ");
    expect(getRecentSearches("code")).toEqual([]);
  });
});

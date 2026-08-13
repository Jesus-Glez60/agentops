import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { getRecentSearches, pushRecentSearch } from "./recent-searches";

describe("recent-searches", () => {
  beforeEach(() => {
    window.localStorage.clear();
  });

  afterEach(() => {
    window.localStorage.clear();
  });

  it("is empty by default", () => {
    expect(getRecentSearches()).toEqual([]);
  });

  it("pushes a query to the front", () => {
    pushRecentSearch("token refresh");
    expect(getRecentSearches()).toEqual(["token refresh"]);
  });

  it("moves a re-searched query back to the front instead of duplicating it", () => {
    pushRecentSearch("a");
    pushRecentSearch("b");
    pushRecentSearch("a");
    expect(getRecentSearches()).toEqual(["a", "b"]);
  });

  it("caps at 5 entries, dropping the oldest", () => {
    for (const q of ["a", "b", "c", "d", "e", "f"]) pushRecentSearch(q);
    expect(getRecentSearches()).toEqual(["f", "e", "d", "c", "b"]);
  });

  it("ignores blank queries", () => {
    pushRecentSearch("   ");
    expect(getRecentSearches()).toEqual([]);
  });

  it("trims whitespace before storing", () => {
    pushRecentSearch("  auth flow  ");
    expect(getRecentSearches()).toEqual(["auth flow"]);
  });
});

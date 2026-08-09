// No server-side search-history endpoint exists anywhere -- recent
// searches are purely a client-side convenience, localStorage-only.
const KEY_PREFIX = "agentops.recent-searches.";
const MAX_ENTRIES = 5;

export function getRecentSearches(namespace: "code" | "docs"): string[] {
  if (typeof window === "undefined") return [];
  try {
    const raw = window.localStorage.getItem(KEY_PREFIX + namespace);
    return raw ? (JSON.parse(raw) as string[]) : [];
  } catch {
    return [];
  }
}

export function pushRecentSearch(namespace: "code" | "docs", query: string): string[] {
  const trimmed = query.trim();
  if (!trimmed) return getRecentSearches(namespace);
  const existing = getRecentSearches(namespace).filter((q) => q !== trimmed);
  const next = [trimmed, ...existing].slice(0, MAX_ENTRIES);
  window.localStorage.setItem(KEY_PREFIX + namespace, JSON.stringify(next));
  return next;
}

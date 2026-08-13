// No server-side search-history endpoint exists anywhere -- recent
// searches are purely a client-side convenience, localStorage-only.
const KEY = "agentops.recent-searches";
const MAX_ENTRIES = 5;

export function getRecentSearches(): string[] {
  if (typeof window === "undefined") return [];
  try {
    const raw = window.localStorage.getItem(KEY);
    return raw ? (JSON.parse(raw) as string[]) : [];
  } catch {
    return [];
  }
}

export function pushRecentSearch(query: string): string[] {
  const trimmed = query.trim();
  if (!trimmed) return getRecentSearches();
  const existing = getRecentSearches().filter((q) => q !== trimmed);
  const next = [trimmed, ...existing].slice(0, MAX_ENTRIES);
  window.localStorage.setItem(KEY, JSON.stringify(next));
  return next;
}

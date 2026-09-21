// Shared `/api/heavy/*` proxy client -- session token stays server-side
// (see profile-api.ts's doc comment for why), this only ever calls the
// same-origin Next.js proxy, never agentops-heavy-api directly. Extracted
// from three identical copies (repos-api.ts, team-api.ts, profile-api.ts)
// once a fourth consumer (libraries-api.ts) needed the exact same thing.
export async function heavyFetch<T>(path: string, init: RequestInit = {}): Promise<T> {
  const res = await fetch(`/api/heavy${path}`, {
    ...init,
    headers: { ...(init.body ? { "Content-Type": "application/json" } : {}), ...init.headers },
    cache: "no-store",
  });
  const data = await res.json().catch(() => null);
  if (!res.ok) {
    const message = data && typeof data === "object" && typeof data.error === "string" ? data.error : `request to ${path} failed with ${res.status}`;
    throw new Error(message);
  }
  return data as T;
}

// Environment-agnostic core shared by `lib/server/heavy-api.ts` (server-only,
// Bearer-token) and `lib/api/fetcher.ts` (client-safe, no auth) -- both are
// otherwise the same fetch -> JSON -> typed-error logic with only the base
// URL and auth-header injection differing. No "server-only"/"use client"
// directive here deliberately: this file takes an explicit URL and has no
// ambient env/token knowledge, so it's safe to import from either side.
export class HttpError extends Error {
  status: number;

  constructor(message: string, status: number) {
    super(message);
    this.name = "HttpError";
    this.status = status;
  }
}

export async function httpFetchJson<T>(url: string | URL, init: RequestInit = {}): Promise<T> {
  const res = await fetch(url, { ...init, cache: "no-store" });
  const data = await res.json().catch(() => null);

  if (!res.ok) {
    const message = (data && typeof data === "object" && "error" in data && typeof data.error === "string" ? data.error : null) ?? `request to ${url} failed with ${res.status}`;
    throw new HttpError(message, res.status);
  }

  return data as T;
}

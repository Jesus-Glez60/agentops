// Client-safe: agentops-api/docbrain-api have no auth to leak (confirmed in
// the frontend kickoff plan's prior-art audit), so unlike heavy-api.ts this
// base URL is deliberately a NEXT_PUBLIC_* var. Thin wrapper over
// `lib/http-fetch.ts`'s shared core.
import { httpFetchJson, HttpError } from "@/lib/http-fetch";

export { HttpError as ApiError };

export async function apiFetch<T>(base: string, path: string, init: RequestInit = {}): Promise<T> {
  const url = new URL(path, base);
  return httpFetchJson<T>(url, {
    ...init,
    headers: {
      ...(init.body ? { "Content-Type": "application/json" } : {}),
      ...init.headers,
    },
  });
}

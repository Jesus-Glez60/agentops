// Server-only: talks to the merged agentops-server process (same port
// agentops-api.ts/libraries-api.ts use) using Bearer-token session auth.
// Only ever imported from Route Handlers / server helpers -- never from a
// "use client" component. Deliberately not a NEXT_PUBLIC_* base URL (see
// the frontend kickoff plan's prior-art audit): main's shelved dashboard
// called all three backends straight from the browser because none of them
// had auth yet. Session tokens must never reach the browser, so this base
// URL and every call through it stay server-side even though the backend
// process itself is the same one the public routes hit directly.
//
// Thin wrapper over `lib/http-fetch.ts`'s shared core -- see that file's
// doc comment for why the fetch/JSON/error logic itself isn't duplicated
// here a second time.
import { httpFetchJson, HttpError } from "@/lib/http-fetch";

const HEAVY_API_URL = process.env.AGENTOPS_HEAVY_API_URL ?? "http://127.0.0.1:8420";

export { HttpError as HeavyApiError };

interface HeavyFetchOptions extends RequestInit {
  /** Raw session token, sent as `Authorization: Bearer <token>`. */
  token?: string;
}

export async function heavyApiFetch<T>(path: string, options: HeavyFetchOptions = {}): Promise<T> {
  const { token, ...init } = options;
  const url = new URL(path, HEAVY_API_URL);

  return httpFetchJson<T>(url, {
    ...init,
    headers: {
      ...(init.body ? { "Content-Type": "application/json" } : {}),
      ...(token ? { Authorization: `Bearer ${token}` } : {}),
      ...init.headers,
    },
  });
}

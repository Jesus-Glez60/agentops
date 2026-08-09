// Base URLs -- same env-var-with-localhost-fallback convention the old
// pages already used (confirmed identical across all 5), just centralized
// instead of copy-pasted per page.
export const AGENTOPS_API_BASE = process.env.NEXT_PUBLIC_AGENTOPS_API_URL || "http://127.0.0.1:8420";
export const DOCBRAIN_API_BASE = process.env.NEXT_PUBLIC_DOCBRAIN_API_URL || "http://127.0.0.1:8421";
export const HEAVY_API_BASE = process.env.NEXT_PUBLIC_HEAVY_API_URL || "http://127.0.0.1:8978";

export class ApiError extends Error {
  status: number;

  constructor(message: string, status: number) {
    super(message);
    this.name = "ApiError";
    this.status = status;
  }
}

interface ApiFetchOptions extends RequestInit {
  query?: Record<string, string | number | boolean | undefined>;
}

/**
 * One fetch wrapper for all three backends. Builds the URL (with optional
 * query params), parses the JSON body, and throws a typed `ApiError`
 * (carrying the HTTP status, so callers can special-case e.g. 402) when the
 * response isn't ok -- this is the exact fetch->json->throw-on-!ok pattern
 * that was duplicated 5+ times across the old pages, with one fix: it
 * always tries `data.error` first (one old page, docbrain's `libraries`
 * fetch, didn't).
 */
export async function apiFetch<T>(base: string, path: string, options: ApiFetchOptions = {}): Promise<T> {
  const { query, ...init } = options;
  const url = new URL(path, base);
  if (query) {
    for (const [key, value] of Object.entries(query)) {
      if (value !== undefined) url.searchParams.set(key, String(value));
    }
  }

  const res = await fetch(url.toString(), {
    ...init,
    headers: {
      ...(init.body ? { "Content-Type": "application/json" } : {}),
      ...init.headers,
    },
  });

  const data = await res.json().catch(() => null);

  if (!res.ok) {
    const message = (data && typeof data === "object" && "error" in data && typeof data.error === "string" ? data.error : null) ?? `request to ${url.pathname} failed with ${res.status}`;
    throw new ApiError(message, res.status);
  }

  return data as T;
}

/** Reads the same `AGENTOPS_WEB_APP_BASE_PATH` env var next.config.ts sets `basePath` from (via its
 * `env` field, so this is inlined at build time even though the var has no `NEXT_PUBLIC_` prefix) --
 * one env var is the source of truth for both, instead of this file hardcoding a second literal that
 * could drift from next.config.ts's `basePath`. Next.js auto-prefixes `next/link`/`next/router`/
 * `redirect()` calls with `basePath` already -- this constant is only for the handful of call sites
 * that build a raw URL/fetch string themselves and so aren't covered (see the project's own recorded
 * gotcha on this: a raw `fetch("/api/...")` resolves against the origin root, not basePath). */
export const BASE_PATH = process.env.AGENTOPS_WEB_APP_BASE_PATH || "/suite";

export function withBasePath(path: string): string {
  return `${BASE_PATH}${path.startsWith("/") ? path : `/${path}`}`;
}

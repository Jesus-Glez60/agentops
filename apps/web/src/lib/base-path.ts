/** Must match `basePath` in next.config.ts. Next.js auto-prefixes `next/link`/`next/router`/`redirect()`
 * calls with this already -- this constant is only for the handful of call sites that build a raw URL
 * string themselves (window.location.href, manually-constructed absolute links) and so aren't covered. */
export const BASE_PATH = "/suite";

export function withBasePath(path: string): string {
  return `${BASE_PATH}${path.startsWith("/") ? path : `/${path}`}`;
}

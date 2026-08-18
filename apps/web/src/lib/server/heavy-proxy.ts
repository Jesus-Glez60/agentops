// Shared by `/api/heavy/[...path]/route.ts` -- forwards a session-authed
// request to agentops-heavy-api with the caller's bearer token attached,
// server-side only (see heavy-api.ts's doc comment for why the token can
// never reach the browser).
//
// Deliberately allowlisted, not a true open catch-all: this is the only
// generic path-forwarding proxy in the app (every other route.ts maps to
// one hardcoded backend path), so unlike those, a typo or a forgotten
// check here would let any authenticated browser request forward an
// arbitrary path straight into the internal Rust API. `isAllowedPath`
// gates that before any backend call is made.
import { NextResponse, type NextRequest } from "next/server";
import { HeavyApiError, heavyApiFetch } from "@/lib/server/heavy-api";
import { getSessionToken } from "@/lib/auth/session";

/** Prefixes (relative to agentops-heavy-api's root) this proxy will forward -- everything else 404s before any backend call is made. */
const ALLOWED_PREFIXES = ["auth/me", "auth/sessions", "auth/2fa", "auth/api-keys", "team", "invites", "repos", "integrations"];

export function isAllowedPath(path: string): boolean {
  return ALLOWED_PREFIXES.some((prefix) => path === prefix || path.startsWith(`${prefix}/`));
}

export async function proxyHeavyApi(req: NextRequest, pathSegments: string[]): Promise<NextResponse> {
  const path = pathSegments.join("/");
  if (!isAllowedPath(path)) {
    return NextResponse.json({ error: "not found" }, { status: 404 });
  }

  const token = await getSessionToken();
  if (!token) {
    return NextResponse.json({ error: "not signed in" }, { status: 401 });
  }

  const hasBody = req.method !== "GET" && req.method !== "HEAD" && req.method !== "DELETE";
  const body = hasBody ? await req.text() : undefined;
  const search = req.nextUrl.search;

  try {
    const data = await heavyApiFetch<unknown>(`/${path}${search}`, {
      method: req.method,
      token,
      body: body && body.length > 0 ? body : undefined,
    });
    return NextResponse.json(data ?? {});
  } catch (err) {
    if (err instanceof HeavyApiError) {
      return NextResponse.json({ error: err.message }, { status: err.status });
    }
    return NextResponse.json({ error: "unable to reach the backend" }, { status: 502 });
  }
}

// Server-only session helpers -- reads the httpOnly session cookie and, if
// present, asks agentops-heavy-api who it belongs to. The browser never
// sees the raw token (see `heavy-api.ts`'s doc comment); this is the only
// place that cookie's value is read.
import { cookies } from "next/headers";
import type { NextRequest, NextResponse } from "next/server";
import { redirect } from "next/navigation";
import { HeavyApiError, heavyApiFetch } from "@/lib/server/heavy-api";
import type { SessionUser } from "@/lib/auth/types";
import { SESSION_COOKIE } from "@/lib/auth/constants";

export { SESSION_COOKIE };
const SESSION_MAX_AGE_SECONDS = 60 * 60 * 24 * 30; // matches agentops-accounts' 30-day session lifetime

/**
 * `secure` must reflect whether *this request* actually arrived over HTTPS,
 * not just whether this is a production build -- `NODE_ENV=production` is
 * set unconditionally by the Docker image regardless of whether the
 * deployer has TLS set up (plain-HTTP self-host on a LAN IP with no reverse
 * proxy is a completely normal way to run this). Getting this wrong doesn't
 * error anywhere visible: the browser just silently refuses to store a
 * `Secure` cookie delivered over `http://`, so login/signup appear to
 * succeed (the API call returns 200/201 with the user) but the session
 * never actually persists -- caught via live testing against a real
 * plain-HTTP deployment, not assumed. `x-forwarded-proto` covers the
 * common case of a TLS-terminating reverse proxy (nginx/Caddy/Traefik) in
 * front of a plain-HTTP origin; falls back to the request's own scheme
 * otherwise.
 */
function requestIsHttps(req: NextRequest): boolean {
  const forwardedProto = req.headers.get("x-forwarded-proto");
  return forwardedProto ? forwardedProto === "https" : req.nextUrl.protocol === "https:";
}

export function setSessionCookie(response: NextResponse, token: string, req: NextRequest): void {
  response.cookies.set(SESSION_COOKIE, token, {
    httpOnly: true,
    secure: requestIsHttps(req),
    sameSite: "lax",
    path: "/",
    maxAge: SESSION_MAX_AGE_SECONDS,
  });
}

export function clearSessionCookie(response: NextResponse): void {
  response.cookies.delete(SESSION_COOKIE);
}

export async function getSessionToken(): Promise<string | undefined> {
  const store = await cookies();
  return store.get(SESSION_COOKIE)?.value;
}

/** Returns `null` for "not signed in" (no cookie, or an expired/invalid one) rather than throwing. */
export async function getCurrentUser(): Promise<SessionUser | null> {
  const token = await getSessionToken();
  if (!token) return null;

  try {
    return await heavyApiFetch<SessionUser>("/auth/me", { token });
  } catch (err) {
    if (err instanceof HeavyApiError && err.status === 401) return null;
    throw err;
  }
}

/** For use in `(app)/layout.tsx` — belt-and-suspenders with `middleware.ts`'s cheap cookie-presence check. */
export async function requireUser(): Promise<SessionUser> {
  const user = await getCurrentUser();
  if (!user) redirect("/login");
  return user;
}

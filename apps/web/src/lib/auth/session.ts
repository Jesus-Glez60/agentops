// Server-only session helpers -- reads the httpOnly session cookie and, if
// present, asks agentops-heavy-api who it belongs to. The browser never
// sees the raw token (see `heavy-api.ts`'s doc comment); this is the only
// place that cookie's value is read.
import { cookies } from "next/headers";
import type { NextResponse } from "next/server";
import { redirect } from "next/navigation";
import { HeavyApiError, heavyApiFetch } from "@/lib/server/heavy-api";
import type { SessionUser } from "@/lib/auth/types";
import { SESSION_COOKIE } from "@/lib/auth/constants";

export { SESSION_COOKIE };
const SESSION_MAX_AGE_SECONDS = 60 * 60 * 24 * 30; // matches agentops-accounts' 30-day session lifetime

export function setSessionCookie(response: NextResponse, token: string): void {
  response.cookies.set(SESSION_COOKIE, token, {
    httpOnly: true,
    secure: process.env.NODE_ENV === "production",
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

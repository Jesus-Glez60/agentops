// Shared by the login/signup route handlers -- both proxy identically to
// agentops-heavy-api (validate → forward → set the session cookie on
// success), differing only in the backend path and success status. Not a
// route.ts itself, so Next.js doesn't treat this as its own route.
import { NextResponse, type NextRequest } from "next/server";
import { HeavyApiError, heavyApiFetch } from "@/lib/server/heavy-api";
import { setSessionCookie } from "@/lib/auth/session";
import type { AuthResponse, TwoFactorChallenge } from "@/lib/auth/types";

export async function proxyCredentialsAuth(req: NextRequest, backendPath: "/auth/signup" | "/auth/login", successStatus: 200 | 201): Promise<NextResponse> {
  const body = await req.json().catch(() => null);
  if (!body || typeof body.email !== "string" || typeof body.password !== "string") {
    return NextResponse.json({ error: "email and password are required" }, { status: 400 });
  }

  try {
    // The rest of the body (first_name/last_name for signup) is forwarded
    // as-is -- agentops-heavy-api validates its own required fields per
    // route (e.g. LoginRequest ignores extras, SignupRequest 400s on a
    // blank name), so this proxy doesn't need to know each route's exact
    // shape.
    const data = await heavyApiFetch<AuthResponse | TwoFactorChallenge>(backendPath, {
      method: "POST",
      body: JSON.stringify(body),
    });
    // `fetch`'s `res.ok` is true for any 2xx, so a 202 two-factor
    // challenge doesn't throw -- it just comes back shaped differently
    // than `AuthResponse`. Only `/auth/login` can produce this; signup
    // never does, since 2FA can't be enabled before an account exists.
    if ("two_factor_required" in data) {
      return NextResponse.json(data, { status: 202 });
    }
    const response = NextResponse.json({ user: data.user }, { status: successStatus });
    setSessionCookie(response, data.session_token, req);
    return response;
  } catch (err) {
    if (err instanceof HeavyApiError) {
      return NextResponse.json({ error: err.message }, { status: err.status });
    }
    return NextResponse.json({ error: "unable to reach the backend" }, { status: 502 });
  }
}

/** Completes a 2FA login challenge -- same shape as `proxyCredentialsAuth`'s success path (cookie-setting), but a different request/response contract, so kept separate rather than overloading that function with a third mode. */
export async function proxyLogin2fa(req: NextRequest): Promise<NextResponse> {
  const body = await req.json().catch(() => null);
  if (!body || typeof body.challenge_token !== "string" || typeof body.code !== "string") {
    return NextResponse.json({ error: "challenge_token and code are required" }, { status: 400 });
  }

  try {
    const data = await heavyApiFetch<AuthResponse>("/auth/login/2fa", { method: "POST", body: JSON.stringify(body) });
    const response = NextResponse.json({ user: data.user }, { status: 200 });
    setSessionCookie(response, data.session_token, req);
    return response;
  } catch (err) {
    if (err instanceof HeavyApiError) {
      return NextResponse.json({ error: err.message }, { status: err.status });
    }
    return NextResponse.json({ error: "unable to reach the backend" }, { status: 502 });
  }
}

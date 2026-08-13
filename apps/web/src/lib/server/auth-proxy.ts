// Shared by the login/signup route handlers -- both proxy identically to
// agentops-heavy-api (validate → forward → set the session cookie on
// success), differing only in the backend path and success status. Not a
// route.ts itself, so Next.js doesn't treat this as its own route.
import { NextResponse, type NextRequest } from "next/server";
import { HeavyApiError, heavyApiFetch } from "@/lib/server/heavy-api";
import { setSessionCookie } from "@/lib/auth/session";
import type { AuthResponse } from "@/lib/auth/types";

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
    const data = await heavyApiFetch<AuthResponse>(backendPath, {
      method: "POST",
      body: JSON.stringify(body),
    });
    const response = NextResponse.json({ user: data.user }, { status: successStatus });
    setSessionCookie(response, data.session_token);
    return response;
  } catch (err) {
    if (err instanceof HeavyApiError) {
      return NextResponse.json({ error: err.message }, { status: err.status });
    }
    return NextResponse.json({ error: "unable to reach the backend" }, { status: 502 });
  }
}

import { NextResponse, type NextRequest } from "next/server";
import { SESSION_COOKIE } from "@/lib/auth/constants";
import { BASE_PATH } from "@/lib/base-path";

// Cheap presence-only check -- doesn't validate the token against
// agentops-heavy-api (that happens once, server-side, in (app)/layout.tsx
// via requireUser()). This just keeps a signed-out visitor from ever
// seeing the authenticated shell flash before that check runs.
//
// Named `proxy.ts` (not `middleware.ts`) -- Next.js 16 renamed the file
// convention; `middleware.ts` still works but is deprecated and slated for
// removal.
//
// Verified empirically (Next.js docs don't state this): `request.nextUrl`
// has `basePath` already stripped, and `NextResponse.redirect(new URL(...))`
// does NOT re-add it -- a bare `new URL("/login", request.url)` here 307s to
// the un-prefixed `/login`, which 404s once everything lives under
// `/suite`. Both the redirect target and the `from` param must be built
// with BASE_PATH by hand.
export function proxy(request: NextRequest) {
  const hasSession = request.cookies.has(SESSION_COOKIE);
  if (!hasSession) {
    const loginUrl = new URL(`${BASE_PATH}/login`, request.url);
    loginUrl.searchParams.set("from", BASE_PATH + request.nextUrl.pathname + request.nextUrl.search);
    return NextResponse.redirect(loginUrl);
  }
  return NextResponse.next();
}

export const config = {
  // Everything except: the login page itself, the auth API routes, the
  // public invite-preview page and its API route (a visitor previewing an
  // invite link isn't signed in yet -- that's the whole point), the PM2
  // deployment's infra-config wizard (/setup) and its API route (runs
  // before any account exists on the instance, so there's no session to
  // have yet), and Next.js internals/static assets.
  matcher: ["/((?!login|api/auth|invite/|api/invites|setup|api/bootstrap|_next/static|_next/image|favicon.ico).*)"],
};

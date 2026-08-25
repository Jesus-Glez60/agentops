import { NextResponse, type NextRequest } from "next/server";
import { SESSION_COOKIE } from "@/lib/auth/constants";

// Cheap presence-only check -- doesn't validate the token against
// agentops-heavy-api (that happens once, server-side, in (app)/layout.tsx
// via requireUser()). This just keeps a signed-out visitor from ever
// seeing the authenticated shell flash before that check runs.
//
// Named `proxy.ts` (not `middleware.ts`) -- Next.js 16 renamed the file
// convention; `middleware.ts` still works but is deprecated and slated for
// removal.
export function proxy(request: NextRequest) {
  const hasSession = request.cookies.has(SESSION_COOKIE);
  if (!hasSession) {
    const loginUrl = new URL("/login", request.url);
    loginUrl.searchParams.set("from", request.nextUrl.pathname);
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

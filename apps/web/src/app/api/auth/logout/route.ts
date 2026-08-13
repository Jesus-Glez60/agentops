import { NextResponse } from "next/server";
import { heavyApiFetch } from "@/lib/server/heavy-api";
import { clearSessionCookie, getSessionToken } from "@/lib/auth/session";

export async function POST() {
  const token = await getSessionToken();
  if (token) {
    // Best-effort server-side revoke -- even if this fails (network blip,
    // already-expired token), still clear the cookie below so the user
    // ends up logged out locally either way.
    await heavyApiFetch("/auth/logout", { method: "POST", token }).catch(() => undefined);
  }

  const response = NextResponse.json({ loggedOut: true });
  clearSessionCookie(response);
  return response;
}

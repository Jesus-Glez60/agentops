// Public proxy for GET /invites/{token} -- deliberately NOT under
// /api/heavy/* (that proxy always requires a session cookie; see
// heavy-proxy.ts). A visitor previewing an invite link may not be signed
// in yet, so this route forwards to the backend with no bearer token at
// all, matching the backend's own "no session required" contract for this
// one read.
import { NextResponse, type NextRequest } from "next/server";
import { HeavyApiError, heavyApiFetch } from "@/lib/server/heavy-api";

export async function GET(_req: NextRequest, { params }: { params: Promise<{ token: string }> }) {
  const { token } = await params;
  try {
    const data = await heavyApiFetch<unknown>(`/invites/${encodeURIComponent(token)}`);
    return NextResponse.json(data);
  } catch (err) {
    if (err instanceof HeavyApiError) {
      return NextResponse.json({ error: err.message }, { status: err.status });
    }
    return NextResponse.json({ error: "unable to reach the backend" }, { status: 502 });
  }
}

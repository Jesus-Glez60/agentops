// Public proxy for POST /bootstrap/config -- same "no session required"
// shape as invites/[token]/route.ts: a brand-new instance has no account
// (and thus no session) to authenticate with yet. The backend itself is
// what enforces "first-run only" (403s once any account exists), not this
// route -- see accounts_integrations.rs's `bootstrap_config` doc comment.
//
// Deliberately a raw `fetch`, not `heavyApiFetch`: a validation failure
// comes back as `{ "errors": ["...", "..."] }` (see `BootstrapConfig`'s
// doc comment on `validate()`), and `heavyApiFetch`'s generic error path
// only surfaces a single `error` string -- passing the body through as-is
// keeps every validation message the `/setup` page can show.
import { NextResponse, type NextRequest } from "next/server";
import { HEAVY_API_URL } from "@/lib/server/heavy-api";

export async function POST(req: NextRequest) {
  const body = await req.json().catch(() => null);
  if (!body || typeof body !== "object") {
    return NextResponse.json({ error: "invalid request body" }, { status: 400 });
  }

  try {
    const res = await fetch(new URL("/bootstrap/config", HEAVY_API_URL), {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
      cache: "no-store",
    });
    const data = await res.json().catch(() => null);
    return NextResponse.json(data ?? { error: `request failed with ${res.status}` }, { status: res.status });
  } catch {
    return NextResponse.json({ error: "unable to reach the backend" }, { status: 502 });
  }
}

import { headers } from "next/headers";
import { requireUser } from "@/lib/auth/session";
import { OnboardingChecklist } from "@/components/onboarding/onboarding-checklist";

/**
 * Server-derived so the `agentops connect --remote <origin>` command shown
 * in the checklist is correct on first paint with no client/server
 * mismatch -- computing it from `window.location.origin` client-side would
 * either need a post-hydration effect (a real value flash after paint) or
 * risk a hydration mismatch (server has no `window` to render the same
 * value with). Same `x-forwarded-proto` reasoning as `session.ts`'s
 * `requestIsHttps` -- this deployment is commonly plain HTTP with a
 * TLS-terminating reverse proxy in front, not assumed to always be HTTPS.
 */
async function requestOrigin(): Promise<string> {
  const h = await headers();
  const proto = h.get("x-forwarded-proto") ?? "http";
  const host = h.get("x-forwarded-host") ?? h.get("host") ?? "localhost";
  return `${proto}://${host}`;
}

// Top-level, deliberately outside (app)/ -- that layout redirects here
// whenever `!user.onboarding_completed`, so living inside it would loop.
// Session-authed like every (app)/* route (requireUser redirects to
// /login if there's no session), but rendered full-bleed like /login and
// /setup rather than inside the app shell/sidebar.
export default async function WelcomePage() {
  const user = await requireUser();
  const origin = await requestOrigin();
  return <OnboardingChecklist user={user} origin={origin} />;
}

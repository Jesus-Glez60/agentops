import { LoginSignupPanel } from "@/components/auth/login-signup-panel";
import { heavyApiFetch } from "@/lib/server/heavy-api";

// Next.js 16 Server Component searchParams is Promise-wrapped -- must
// await it (day-one-bug-checklist.md #6). `from` is set by middleware.ts
// when it redirects a signed-out visitor here; only trust it as a
// same-origin relative path to avoid it being used as an open redirect.
function sanitizeRedirect(from: string | undefined): string {
  if (from && from.startsWith("/") && !from.startsWith("//")) return from;
  return "/";
}

// `from=/invite/{token}` is how a signed-out visitor lands here after
// clicking a team invite link (`InviteLandingClient`) -- pull the token
// back out so signup can send it along (see `signupWithPassword`'s doc
// comment for why: it's the only thing that lets signup through once this
// instance is gated). The actual join still happens when the redirect
// lands back on `/invite/{token}` and calls `POST /invites/accept`.
function inviteTokenFrom(redirectTo: string): string | undefined {
  return redirectTo.match(/^\/invite\/([^/?#]+)/)?.[1];
}

interface BootstrapStatus {
  has_accounts: boolean;
  signup_open: boolean;
}

async function getBootstrapStatus(): Promise<BootstrapStatus> {
  try {
    return await heavyApiFetch<BootstrapStatus>("/auth/bootstrap-status");
  } catch {
    // Backend unreachable or the route somehow errors -- fail toward the
    // pre-existing behavior (both tabs shown, login-first) rather than
    // locking a visitor out of a page whose whole job is letting them in.
    return { has_accounts: true, signup_open: true };
  }
}

export default async function LoginPage({ searchParams }: { searchParams: Promise<{ from?: string }> }) {
  const params = await searchParams;
  const redirectTo = sanitizeRedirect(params.from);
  const inviteToken = inviteTokenFrom(redirectTo);
  const { has_accounts, signup_open } = await getBootstrapStatus();

  // First-run UX: an empty instance defaults straight to Signup (that's
  // the setup step). Once gated (`signup_open` false) and there's no
  // invite in hand, signup is a dead end -- don't offer the tab at all.
  const showSignupTab = signup_open || !!inviteToken;
  const defaultTab = !has_accounts ? "signup" : "login";

  return <LoginSignupPanel hasAccounts={has_accounts} showSignupTab={showSignupTab} defaultTab={defaultTab} redirectTo={redirectTo} inviteToken={inviteToken} />;
}

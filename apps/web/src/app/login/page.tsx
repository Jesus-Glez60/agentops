import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { LoginForm } from "@/components/auth/login-form";
import { SignupForm } from "@/components/auth/signup-form";
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

  return (
    <main className="flex min-h-screen items-center justify-center bg-canvas p-4">
      <Card className="w-full max-w-sm">
        <CardHeader>
          <CardTitle className="text-page-title">AgentOps</CardTitle>
          <CardDescription>{!has_accounts ? "Set up your AgentOps instance." : "Sign in to your account, or create a new one."}</CardDescription>
        </CardHeader>
        <CardContent>
          {showSignupTab ? (
            <Tabs defaultValue={defaultTab}>
              <TabsList className="w-full">
                <TabsTrigger value="login" className="flex-1">
                  Log in
                </TabsTrigger>
                <TabsTrigger value="signup" className="flex-1">
                  Sign up
                </TabsTrigger>
              </TabsList>
              <TabsContent value="login" className="pt-4">
                <LoginForm redirectTo={redirectTo} />
              </TabsContent>
              <TabsContent value="signup" className="pt-4">
                <SignupForm redirectTo={redirectTo} inviteToken={inviteToken} />
              </TabsContent>
            </Tabs>
          ) : (
            <LoginForm redirectTo={redirectTo} />
          )}
        </CardContent>
      </Card>
    </main>
  );
}

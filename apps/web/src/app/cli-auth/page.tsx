import { requireUser } from "@/lib/auth/session";
import { DeviceApprovalCard } from "@/components/cli-auth/device-approval-card";

// Top-level, deliberately outside (app)/ -- same reasoning as /welcome:
// this must work for a user who just logged in, before onboarding
// completes. `proxy.ts` already redirects a signed-out visitor to
// `/login?from=/cli-auth?code=...` (preserving the query string -- see
// its own comment), so by the time this component runs a session cookie
// is guaranteed to exist; `requireUser()` here is defense-in-depth, not
// the primary gate.
export default async function CliAuthPage({ searchParams }: { searchParams: Promise<{ code?: string }> }) {
  const user = await requireUser();
  const { code } = await searchParams;
  return <DeviceApprovalCard initialCode={code} userName={user.first_name} />;
}

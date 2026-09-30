import { redirect } from "next/navigation";
import { requireUser } from "@/lib/auth/session";
import { AppShell } from "@/components/shell/app-shell";
import { SwrProvider } from "@/components/providers/swr-provider";

export default async function AppLayout({ children }: { children: React.ReactNode }) {
  // Belt-and-suspenders with middleware.ts's cheap cookie-presence check --
  // this call actually validates the session against agentops-heavy-api.
  const user = await requireUser();

  // Mandatory but not blocking -- every session-authed entry into the app
  // shell funnels through here, so onboarding can't be silently skipped
  // the way it was before this existed (see the onboarding plan's Context).
  // `/welcome` itself lives outside this route group, so this never loops.
  // Its only job is making sure the page gets shown once; the checklist
  // itself is entirely skippable once there.
  if (!user.onboarding_completed) {
    redirect("/welcome");
  }

  return (
    <SwrProvider>
      <AppShell user={user}>{children}</AppShell>
    </SwrProvider>
  );
}
